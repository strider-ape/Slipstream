//! The background process that owns every download (ROADMAP M4). Clients (CLI, browser
//! host, GUI) talk to it over [`slipstream_core::ipc`]; the list lives in SQLite.

pub mod autostart;
pub mod catalogue;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use slipstream_core::download::{self, Error as DownloadError, Outcome, Progress};
use slipstream_core::ipc::{
    self, AddRequest, AppSettings, CaptureSettings, DownloadInfo, Listener, Paths, Request,
    Response, Status, Stream,
};
use slipstream_core::storage::{part_path, sidecar_path};
use tokio::io::BufReader;
use tokio::sync::{Notify, oneshot};
use tokio::task::{JoinHandle, JoinSet};

use catalogue::Catalogue;

/// How often `watch` clients get a fresh list.
const WATCH_EVERY: Duration = Duration::from_millis(500);

/// Run the daemon until a client sends `Shutdown` or the process is asked to stop.
/// Unfinished downloads from the last run start again. Fails with `AddrInUse` if a
/// daemon is already running for these paths.
pub async fn run(paths: &Paths) -> io::Result<()> {
    std::fs::create_dir_all(&paths.data_dir)?;
    let mut listener = Listener::bind(paths).await?;
    let db = Catalogue::open(&paths.data_dir.join("catalogue.db")).map_err(io::Error::other)?;
    let daemon = Arc::new(Daemon {
        db: Mutex::new(db),
        running: Mutex::default(),
        default_dir: paths.downloads.clone(),
        shutdown: Notify::new(),
    });

    // Pick up where the last run stopped.
    let unfinished = daemon
        .db()
        .ids_with_status(&[Status::Queued, Status::Downloading]);
    for id in unfinished.map_err(io::Error::other)? {
        daemon.start(id);
    }

    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            conn = listener.accept() => match conn {
                Ok(stream) => {
                    clients.spawn(serve(daemon.clone(), stream));
                }
                Err(e) => eprintln!("slipstream-daemon: accept failed: {e}"),
            },
            // Collect finished client tasks so they don't pile up.
            Some(_) = clients.join_next() => {}
            () = daemon.shutdown.notified() => break,
            () = terminated() => break,
        }
    }
    // Stop every download cleanly (progress saved). Their status stays "downloading",
    // so the next start continues them.
    // Hang up on clients: on Windows a pipe name stays taken while any
    // connection to it is open, which would block the next daemon.
    clients.shutdown().await;
    let all: Vec<i64> = daemon.running.lock().unwrap().keys().copied().collect();
    for id in all {
        daemon.stop(id).await;
    }
    Ok(())
}

/// Ctrl+C, or SIGTERM on Linux (logout, `systemctl stop`, `kill`).
async fn terminated() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

struct Daemon {
    db: Mutex<Catalogue>,
    running: Mutex<HashMap<i64, Running>>,
    default_dir: PathBuf,
    shutdown: Notify,
}

/// A download in progress.
struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
    live: Arc<Mutex<Live>>,
}

/// What a running download last reported; merged into list answers.
#[derive(Clone, Default)]
struct Live {
    name: Option<String>,
    size: Option<u64>,
    downloaded: u64,
    speed: u64,
    connections: usize,
    segments: Vec<[u64; 3]>,
}

impl Daemon {
    fn db(&self) -> MutexGuard<'_, Catalogue> {
        self.db.lock().unwrap()
    }

    async fn handle(self: &Arc<Self>, req: Request) -> Response {
        let result = match req {
            Request::Ping => Ok(Response::Pong {
                version: slipstream_core::VERSION.into(),
                protocol: ipc::PROTOCOL,
            }),
            Request::Add(add) => self.add(add),
            Request::List | Request::Watch => self.snapshot(),
            Request::Pause { id } => self.pause(id).await,
            Request::Resume { id } => self.resume(id),
            Request::Remove { id, delete_file } => self.remove(id, delete_file).await,
            Request::Shutdown => {
                self.shutdown.notify_one();
                Ok(Response::Ok)
            }
            Request::CaptureSettings => Ok(Response::CaptureSettings(self.capture_settings())),
            Request::GetSettings => Ok(Response::Settings(self.app_settings())),
            Request::SetSettings(s) => self.set_app_settings(s),
        };
        result.unwrap_or_else(|message| Response::Error { message })
    }

    /// Saved capture rules (`settings` key "capture"), or the defaults.
    fn capture_settings(&self) -> CaptureSettings {
        let saved = self.db().setting("capture").ok().flatten();
        let mut rules: CaptureSettings = saved
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        rules.enabled &= self.app_settings().catch_enabled;
        rules
    }

    /// Saved preferences (`settings` key "app"), or the defaults.
    fn app_settings(&self) -> AppSettings {
        let saved = self.db().setting("app").ok().flatten();
        saved
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| AppSettings {
                download_dir: self.default_dir.clone(),
                max_connections: 8,
                speed_limit: 0,
                catch_enabled: true,
            })
    }

    fn set_app_settings(&self, s: AppSettings) -> Result<Response, String> {
        if !(1..=32).contains(&s.max_connections) {
            return Err("connections must be between 1 and 32".into());
        }
        let json = serde_json::to_string(&s).map_err(|e| e.to_string())?;
        self.db()
            .set_setting("app", &json)
            .map_err(|e| e.to_string())?;
        Ok(Response::Ok)
    }

    fn add(self: &Arc<Self>, mut req: AddRequest) -> Result<Response, String> {
        if !(req.url.starts_with("http://") || req.url.starts_with("https://")) {
            return Err(format!("not an http(s) URL: {}", req.url));
        }
        // Whatever the request leaves open comes from the settings.
        let app = self.app_settings();
        let dir = req.dir.clone().unwrap_or_else(|| app.download_dir.clone());
        if req.connections == 0 {
            req.connections = app.max_connections;
        }
        if req.speed_limit == 0 {
            req.speed_limit = app.speed_limit;
        }
        // Two downloads of one URL into one folder would share a part file.
        let list = self.db().list().map_err(|e| e.to_string())?;
        if let Some(d) = list
            .iter()
            .find(|d| d.url == req.url && d.dir == dir && d.status != Status::Complete)
        {
            return Err(format!("already in the list as #{}", d.id));
        }
        let id = self.db().add(&req, &dir).map_err(|e| e.to_string())?;
        self.start(id);
        Ok(Response::Added { id })
    }

    /// Start (or continue) download `id` in the background.
    fn start(self: &Arc<Self>, id: i64) {
        let Ok(Some(job)) = self.db().job(id) else {
            return;
        };
        let _ = self.db().set_status(id, Status::Downloading, None);
        let req = download::Request {
            url: job.url,
            headers: job.headers,
            dir: job.dir,
            name: job.name,
            connections: job.connections,
            speed_limit: job.speed_limit,
        };
        // Hold the map while spawning, so a download that ends at once can't
        // finish before it is registered.
        let mut running = self.running.lock().unwrap();
        if running.contains_key(&id) {
            return;
        }
        let live = Arc::new(Mutex::new(Live::default()));
        let (stop, stopped) = oneshot::channel::<()>();
        let (me, report) = (self.clone(), live.clone());
        let task = tokio::spawn(async move {
            let mut meter = Meter::default();
            let stop = async {
                let _ = stopped.await;
            };
            let result = download::download(&req, stop, |p| meter.update(&report, p)).await;
            me.finished(id, &report, result);
        });
        running.insert(id, Running { stop, task, live });
    }

    /// Record how a download ended.
    fn finished(&self, id: i64, live: &Mutex<Live>, result: Result<Outcome, DownloadError>) {
        let l = live.lock().unwrap().clone();
        {
            let db = self.db();
            match result {
                Ok(out) => {
                    let name = out
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned());
                    let _ = db.set_progress(id, name.as_deref(), Some(out.size), out.size);
                    let _ = db.set_status(id, Status::Complete, None);
                }
                // Paused, removed or shutting down: whoever stopped it set the status.
                Err(DownloadError::Cancelled) => {
                    let _ = db.set_progress(id, l.name.as_deref(), l.size, l.downloaded);
                }
                Err(e) => {
                    let _ = db.set_progress(id, l.name.as_deref(), l.size, l.downloaded);
                    let _ = db.set_status(id, Status::Error, Some(&e.to_string()));
                }
            }
        }
        self.running.lock().unwrap().remove(&id);
    }

    /// Stop a running download and wait until it has saved its progress.
    async fn stop(&self, id: i64) {
        let running = self.running.lock().unwrap().remove(&id);
        if let Some(r) = running {
            let _ = r.stop.send(());
            let _ = r.task.await;
        }
    }

    async fn pause(&self, id: i64) -> Result<Response, String> {
        // Read the status first: a lock taken in a `match` header is held for the
        // whole match, and the arms need the database again.
        let status = self.db().status(id).map_err(|e| e.to_string())?;
        match status {
            None => return Err(format!("no download #{id}")),
            Some(Status::Queued | Status::Downloading) => {}
            Some(s) => return Err(format!("#{id} is {}", s.as_str())),
        }
        self.db()
            .set_status(id, Status::Paused, None)
            .map_err(|e| e.to_string())?;
        self.stop(id).await;
        Ok(Response::Ok)
    }

    fn resume(self: &Arc<Self>, id: i64) -> Result<Response, String> {
        // Read the status first: a lock taken in a `match` header is held for the
        // whole match, and the arms need the database again.
        let status = self.db().status(id).map_err(|e| e.to_string())?;
        match status {
            None => Err(format!("no download #{id}")),
            Some(Status::Paused | Status::Error) => {
                self.start(id);
                Ok(Response::Ok)
            }
            Some(s) => Err(format!("#{id} is {}", s.as_str())),
        }
    }

    async fn remove(&self, id: i64, delete_file: bool) -> Result<Response, String> {
        let job = self.db().job(id).map_err(|e| e.to_string())?;
        let Some(job) = job else {
            return Err(format!("no download #{id}"));
        };
        self.stop(id).await;
        let name = self.snapshot_of(id).and_then(|d| d.name).or(job.name);
        self.db().remove(id).map_err(|e| e.to_string())?;
        if delete_file && let Some(name) = name {
            let target = job.dir.join(name);
            for path in [part_path(&target), sidecar_path(&target), target] {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(Response::Ok)
    }

    fn snapshot_of(&self, id: i64) -> Option<DownloadInfo> {
        let list = self.list().ok()?;
        list.into_iter().find(|d| d.id == id)
    }

    fn snapshot(&self) -> Result<Response, String> {
        Ok(Response::Downloads {
            downloads: self.list().map_err(|e| e.to_string())?,
        })
    }

    /// The catalogue with live numbers of running downloads laid over it.
    fn list(&self) -> rusqlite::Result<Vec<DownloadInfo>> {
        let mut list = self.db().list()?;
        let running = self.running.lock().unwrap();
        for d in &mut list {
            if let Some(r) = running.get(&d.id) {
                let l = r.live.lock().unwrap();
                d.name = l.name.clone().or(d.name.take());
                d.size = l.size.or(d.size);
                d.downloaded = l.downloaded.max(d.downloaded);
                d.speed = l.speed;
                d.connections = l.connections;
                d.segments = l.segments.clone();
            }
        }
        Ok(list)
    }
}

/// Turns progress reports into the numbers clients show.
#[derive(Default)]
struct Meter {
    last: Option<(Instant, u64)>,
    speed: f64,
}

impl Meter {
    fn update(&mut self, live: &Mutex<Live>, p: Progress) {
        let now = Instant::now();
        if let Some((t, bytes)) = self.last {
            let dt = now.duration_since(t).as_secs_f64();
            if dt > 0.0 {
                let instant = p.downloaded.saturating_sub(bytes) as f64 / dt;
                self.speed = if self.speed == 0.0 {
                    instant
                } else {
                    0.7 * self.speed + 0.3 * instant
                };
            }
        }
        self.last = Some((now, p.downloaded));
        let mut l = live.lock().unwrap();
        l.name = p.name.or(l.name.take());
        l.size = p.size;
        l.downloaded = p.downloaded;
        l.speed = self.speed as u64;
        l.connections = p.connections;
        l.segments = p.segments;
    }
}

/// One client connection: answer requests until it hangs up.
async fn serve(daemon: Arc<Daemon>, stream: Stream) {
    let (r, mut w) = tokio::io::split(stream);
    let mut r = BufReader::new(r);
    loop {
        let req = match ipc::recv::<Request>(&mut r).await {
            Ok(Some(req)) => req,
            // A request this daemon doesn't know (e.g. from a newer app): say so, keep going.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                let message =
                    format!("unknown request (is the daemon older than the app? restart it): {e}");
                if ipc::send(&mut w, &Response::Error { message })
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
            _ => return,
        };
        if req == Request::Watch {
            loop {
                let snapshot = daemon.handle(Request::List).await;
                if ipc::send(&mut w, &snapshot).await.is_err() {
                    return;
                }
                tokio::time::sleep(WATCH_EVERY).await;
            }
        }
        let response = daemon.handle(req).await;
        if ipc::send(&mut w, &response).await.is_err() {
            return;
        }
    }
}
