//! The connection to the daemon, on its own thread: a `watch` stream feeds the
//! window, and the window's buttons come back here as commands. Reconnects (and
//! starts the daemon) by itself.

use std::collections::HashSet;
use std::io;
use std::time::Duration;

use slint::Weak;
use slipstream_core::ipc::{
    AddRequest, AppSettings, Client, DownloadInfo, Paths, Request, Response,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::{CaughtPopup, MainWindow};

/// What the window asks the daemon to do.
pub enum Command {
    Add(String),
    Pause(i64),
    Resume(i64),
    Remove { id: i64, delete_file: bool },
    SaveSettings(AppSettings),
}

pub fn start(ui: Weak<MainWindow>, popup: Weak<CaughtPopup>) -> UnboundedSender<Command> {
    let (tx, rx) = unbounded_channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        rt.expect("tokio runtime").block_on(run(ui, popup, rx));
    });
    tx
}

async fn run(ui: Weak<MainWindow>, popup: Weak<CaughtPopup>, mut rx: UnboundedReceiver<Command>) {
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => return status(&ui, e.to_string()),
    };
    let mut seen = Seen::default();
    loop {
        match session(&paths, &ui, &popup, &mut rx, &mut seen).await {
            Ok(()) => return, // the window is gone
            Err(e) => status(&ui, format!("Not connected to Slipstream ({e}); retrying…")),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Which downloads the window already knows, to spot ones the browser just handed over.
#[derive(Default)]
struct Seen {
    known: Option<HashSet<i64>>,
    /// URLs added from this window: no popup for those.
    ours: HashSet<String>,
}

impl Seen {
    fn update(&mut self, list: &[DownloadInfo], popup: &Weak<CaughtPopup>) {
        if let Some(known) = &self.known {
            for d in list.iter().filter(|d| !known.contains(&d.id)) {
                if !self.ours.remove(&d.url) {
                    caught(popup, d.clone());
                }
            }
        }
        self.known = Some(list.iter().map(|d| d.id).collect());
    }
}

async fn session(
    paths: &Paths,
    ui: &Weak<MainWindow>,
    popup: &Weak<CaughtPopup>,
    rx: &mut UnboundedReceiver<Command>,
    seen: &mut Seen,
) -> io::Result<()> {
    let mut watch = Client::connect_or_start(paths).await?;
    let mut cmd = Client::connect(paths).await?;
    if let Response::Settings(s) = cmd.call(&Request::GetSettings).await? {
        settings(ui, s);
    }
    status(ui, String::new());
    let first = watch.call(&Request::Watch).await?;
    show(ui, popup, seen, first);
    loop {
        tokio::select! {
            msg = watch.next() => match msg? {
                Some(msg) => show(ui, popup, seen, msg),
                None => return Err(io::ErrorKind::UnexpectedEof.into()),
            },
            c = rx.recv() => {
                let Some(c) = c else { return Ok(()) };
                let saving = matches!(c, Command::SaveSettings(_));
                let req = match c {
                    Command::Add(url) => {
                        seen.ours.insert(url.clone());
                        Request::Add(AddRequest { url, ..Default::default() })
                    }
                    Command::Pause(id) => Request::Pause { id },
                    Command::Resume(id) => Request::Resume { id },
                    Command::Remove { id, delete_file } => Request::Remove { id, delete_file },
                    Command::SaveSettings(s) => Request::SetSettings(s),
                };
                match cmd.call(&req).await? {
                    Response::Error { message } => status(ui, message),
                    Response::Ok if saving => status(ui, "Settings saved".into()),
                    _ => {}
                }
            }
        }
    }
}

fn show(ui: &Weak<MainWindow>, popup: &Weak<CaughtPopup>, seen: &mut Seen, msg: Response) {
    if let Response::Downloads { downloads } = msg {
        seen.update(&downloads, popup);
        let ui = ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                crate::show_list(&ui, downloads);
            }
        });
    }
}

fn status(ui: &Weak<MainWindow>, text: String) {
    let ui = ui.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            ui.set_status_text(text.into());
        }
    });
}

fn settings(ui: &Weak<MainWindow>, s: AppSettings) {
    let ui = ui.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            crate::show_settings(&ui, s);
        }
    });
}

fn caught(popup: &Weak<CaughtPopup>, d: DownloadInfo) {
    let popup = popup.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(popup) = popup.upgrade() {
            crate::show_caught(&popup, d);
        }
    });
}
