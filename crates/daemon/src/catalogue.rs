//! The download list, kept in SQLite (SPEC-engine §11). Byte progress lives in each
//! download's sidecar; this is the list the clients show and what survives restarts.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row, params};
use slipstream_core::ipc::{AddRequest, DownloadInfo, Status};

pub struct Catalogue {
    db: Connection,
}

/// Everything needed to (re)start a download.
pub struct Job {
    pub url: String,
    pub dir: PathBuf,
    pub name: Option<String>,
    pub headers: Vec<(String, String)>,
    pub connections: usize,
    pub speed_limit: u64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Catalogue {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS downloads (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 url         TEXT    NOT NULL,
                 dir         TEXT    NOT NULL,
                 name        TEXT,
                 size        INTEGER,
                 downloaded  INTEGER NOT NULL DEFAULT 0,
                 status      TEXT    NOT NULL,
                 error       TEXT,
                 headers     TEXT    NOT NULL DEFAULT '[]',
                 connections INTEGER NOT NULL DEFAULT 0,
                 speed_limit INTEGER NOT NULL DEFAULT 0,
                 added       INTEGER NOT NULL,
                 finished    INTEGER
             );
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             PRAGMA user_version = 1;",
        )?;
        Ok(Self { db })
    }

    pub fn add(&self, req: &AddRequest, dir: &Path) -> rusqlite::Result<i64> {
        let headers = serde_json::to_string(&req.headers).unwrap_or_else(|_| "[]".into());
        self.db.execute(
            "INSERT INTO downloads (url, dir, name, status, headers, connections, speed_limit, added)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                req.url,
                dir.to_string_lossy(),
                req.name,
                Status::Queued.as_str(),
                headers,
                req.connections as i64,
                req.speed_limit as i64,
                now()
            ],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    pub fn job(&self, id: i64) -> rusqlite::Result<Option<Job>> {
        self.db
            .query_row(
                "SELECT url, dir, name, headers, connections, speed_limit FROM downloads WHERE id = ?1",
                [id],
                |r| {
                    let headers: String = r.get(3)?;
                    Ok(Job {
                        url: r.get(0)?,
                        dir: PathBuf::from(r.get::<_, String>(1)?),
                        name: r.get(2)?,
                        headers: serde_json::from_str(&headers).unwrap_or_default(),
                        connections: r.get::<_, i64>(4)? as usize,
                        speed_limit: r.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()
    }

    pub fn list(&self) -> rusqlite::Result<Vec<DownloadInfo>> {
        let mut stmt = self.db.prepare(
            "SELECT id, url, dir, name, status, size, downloaded, error FROM downloads ORDER BY id",
        )?;
        stmt.query_map([], info)?.collect()
    }

    pub fn ids_with_status(&self, statuses: &[Status]) -> rusqlite::Result<Vec<i64>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|d| statuses.contains(&d.status))
            .map(|d| d.id)
            .collect())
    }

    pub fn status(&self, id: i64) -> rusqlite::Result<Option<Status>> {
        let s: Option<String> = self
            .db
            .query_row("SELECT status FROM downloads WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(s.and_then(|s| Status::parse(&s)))
    }

    pub fn set_status(&self, id: i64, status: Status, error: Option<&str>) -> rusqlite::Result<()> {
        let finished = (status == Status::Complete).then(now);
        self.db.execute(
            "UPDATE downloads SET status = ?2, error = ?3, finished = COALESCE(?4, finished) WHERE id = ?1",
            params![id, status.as_str(), error, finished],
        )?;
        Ok(())
    }

    /// Remember what we know so far (shown while the daemon restarts).
    pub fn set_progress(
        &self,
        id: i64,
        name: Option<&str>,
        size: Option<u64>,
        downloaded: u64,
    ) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE downloads SET name = COALESCE(?2, name), size = COALESCE(?3, size), downloaded = ?4
             WHERE id = ?1",
            params![id, name, size.map(|s| s as i64), downloaded as i64],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn remove(&self, id: i64) -> rusqlite::Result<bool> {
        Ok(self
            .db
            .execute("DELETE FROM downloads WHERE id = ?1", [id])?
            > 0)
    }
}

fn info(r: &Row<'_>) -> rusqlite::Result<DownloadInfo> {
    let status: String = r.get(4)?;
    Ok(DownloadInfo {
        id: r.get(0)?,
        url: r.get(1)?,
        dir: PathBuf::from(r.get::<_, String>(2)?),
        name: r.get(3)?,
        status: Status::parse(&status).unwrap_or(Status::Error),
        size: r.get::<_, Option<i64>>(5)?.map(|s| s as u64),
        downloaded: r.get::<_, i64>(6)? as u64,
        speed: 0,
        connections: 0,
        error: r.get(7)?,
        segments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_update_list_remove() {
        let c = Catalogue::open(Path::new(":memory:")).unwrap();
        let req = AddRequest {
            url: "https://x/a.zip".into(),
            headers: vec![("Cookie".into(), "a=b".into())],
            ..Default::default()
        };
        let id = c.add(&req, Path::new("/tmp")).unwrap();
        assert_eq!(c.status(id).unwrap(), Some(Status::Queued));
        assert_eq!(c.job(id).unwrap().unwrap().headers, req.headers);

        c.set_progress(id, Some("a.zip"), Some(100), 40).unwrap();
        c.set_status(id, Status::Paused, None).unwrap();
        let row = &c.list().unwrap()[0];
        assert_eq!(
            (row.name.as_deref(), row.size, row.downloaded),
            (Some("a.zip"), Some(100), 40)
        );
        assert_eq!(c.ids_with_status(&[Status::Paused]).unwrap(), vec![id]);

        assert_eq!(c.setting("capture").unwrap(), None);
        c.set_setting("capture", "{}").unwrap();
        c.set_setting("capture", "{\"enabled\":false}").unwrap();
        assert_eq!(
            c.setting("capture").unwrap().as_deref(),
            Some("{\"enabled\":false}")
        );

        assert!(c.remove(id).unwrap());
        assert!(c.list().unwrap().is_empty());
    }
}
