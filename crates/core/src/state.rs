//! Resume state, saved next to the part file as `<name>.slipstream.json`
//! (SPEC-engine §10). The bytes live in the part file; this says which are done.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Format version of the sidecar file.
const VERSION: u32 = 1;

/// What identifies "the same file" on the server, for `If-Range`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Validator {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub size: Option<u64>,
}

impl Validator {
    /// Value for `If-Range`: a strong ETag, else Last-Modified. Weak ETags
    /// (`W/"..."`) are not allowed in `If-Range` (RFC 9110 §13.1.5).
    pub fn if_range(&self) -> Option<&str> {
        self.etag
            .as_deref()
            .filter(|e| !e.starts_with("W/"))
            .or(self.last_modified.as_deref())
    }
}

/// A byte range of the file and how much of it is already on disk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: u32,
    pub start: u64,
    /// Inclusive end. `None` = until the server stops (size unknown).
    pub end: Option<u64>,
    /// Bytes written from `start`.
    pub done: u64,
}

impl Segment {
    /// Next byte to fetch.
    pub fn next_offset(&self) -> u64 {
        self.start + self.done
    }

    /// Bytes still missing, if the end is known.
    pub fn remaining(&self) -> Option<u64> {
        self.end
            .map(|end| (end + 1).saturating_sub(self.next_offset()))
    }

    pub fn is_complete(&self) -> bool {
        self.remaining() == Some(0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub v: u32,
    /// The URL as given (re-resolved if the final URL expires).
    pub url: String,
    /// Where redirects ended; segment requests go here.
    pub final_url: String,
    pub method: String,
    /// Request headers to replay (cookies, referer, user agent...).
    pub headers: Vec<(String, String)>,
    pub validator: Validator,
    /// `Content-Type` of the first answer (an HTML page later means "link expired").
    #[serde(default)]
    pub mime: Option<String>,
    /// Whether the server supports byte ranges (can resume / split).
    pub ranged: bool,
    pub segments: Vec<Segment>,
    /// Unix seconds when the download was added.
    pub created: u64,
}

impl State {
    pub fn new(url: &str, headers: Vec<(String, String)>) -> Self {
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self {
            v: VERSION,
            url: url.to_owned(),
            final_url: url.to_owned(),
            method: "GET".to_owned(),
            headers,
            validator: Validator::default(),
            mime: None,
            ranged: false,
            segments: Vec::new(),
            created,
        }
    }

    /// Every segment as `[start, end, done]`; an open end shows as what arrived.
    pub fn segment_view(&self) -> Vec<[u64; 3]> {
        let end = |s: &Segment| s.end.unwrap_or((s.start + s.done).saturating_sub(1));
        self.segments
            .iter()
            .map(|s| [s.start, end(s), s.done])
            .collect()
    }

    /// Bytes on disk so far.
    pub fn downloaded(&self) -> u64 {
        self.segments.iter().map(|s| s.done).sum()
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let state: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if state.v != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported state version {}", state.v),
            ));
        }
        Ok(state)
    }

    /// Atomic save: write a temp file, flush it to disk, then rename over the old
    /// one. A crash at any moment leaves either the old or the new state, never half.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_range_prefers_strong_etag() {
        let mut v = Validator {
            etag: Some("\"abc\"".into()),
            last_modified: Some("Wed, 21 Oct 2026 10:00:00 GMT".into()),
            size: Some(1),
        };
        assert_eq!(v.if_range(), Some("\"abc\""));
        v.etag = Some("W/\"abc\"".into());
        assert_eq!(v.if_range(), Some("Wed, 21 Oct 2026 10:00:00 GMT"));
        v.last_modified = None;
        assert_eq!(v.if_range(), None);
    }

    #[test]
    fn segment_math() {
        let s = Segment {
            id: 0,
            start: 100,
            end: Some(199),
            done: 40,
        };
        assert_eq!(s.next_offset(), 140);
        assert_eq!(s.remaining(), Some(60));
        assert!(!s.is_complete());
        let open = Segment { end: None, ..s };
        assert_eq!(open.remaining(), None);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("slipstream-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.slipstream.json");
        let mut st = State::new("http://h/x", vec![("Cookie".into(), "a=b".into())]);
        st.segments.push(Segment {
            id: 0,
            start: 0,
            end: Some(9),
            done: 5,
        });
        st.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap(), st);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
