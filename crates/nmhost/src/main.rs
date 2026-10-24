//! Slipstream native messaging host: the browser extension's bridge to the daemon
//! (SPEC-browser-capture §1). Browsers start it and talk to it over stdin/stdout.
//! stdout is the browser channel, so this program never prints anything else there
//! while serving a browser; it logs to `nmhost.log` in the data folder.
//!
//!     slipstream-nmhost --register     tell installed browsers where this host is
//!     slipstream-nmhost --unregister   undo that

mod messages;
mod register;

use std::io::{self, Write as _};
use std::process::ExitCode;

use messages::{Action, Capture, Incoming, Outgoing};
use slipstream_core::ipc::{AddRequest, Client, Paths, Request, Response};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--register") => return register::cli(true),
        Some("--unregister") => return register::cli(false),
        Some("-V" | "--version") => {
            println!("slipstream-nmhost {}", slipstream_core::VERSION);
            return ExitCode::SUCCESS;
        }
        // Browsers pass the caller's origin (Chrome) or manifest path (Firefox).
        _ => {}
    }
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("slipstream-nmhost: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut host = Host {
        log: Log::open(&paths),
        paths,
        daemon: None,
    };
    host.log.line(&format!("started, args {args:?}"));

    let (mut stdin, mut stdout) = (tokio::io::stdin(), tokio::io::stdout());
    loop {
        let raw = match messages::read(&mut stdin).await {
            Ok(Some(raw)) => raw,
            Ok(None) => break, // the browser closed the port
            Err(e) => {
                host.log.line(&format!("bad input, closing: {e}"));
                break;
            }
        };
        let reply = host.handle(&raw).await;
        if let Err(e) = messages::write(&mut stdout, &reply).await {
            host.log.line(&format!("can't answer the browser: {e}"));
            break;
        }
    }
    host.log.line("stopped");
    ExitCode::SUCCESS
}

struct Host {
    paths: Paths,
    /// Connection to the daemon, made on first use.
    daemon: Option<Client>,
    log: Log,
}

impl Host {
    async fn handle(&mut self, raw: &[u8]) -> Outgoing {
        let msg = match messages::parse(raw) {
            Ok(msg) => msg,
            Err(message) => {
                self.log.line(&format!("rejected message: {message}"));
                return Outgoing::Error { id: None, message };
            }
        };
        match msg {
            Incoming::Hello { browser, ext } => {
                self.log
                    .line(&format!("hello from {browser}, extension {ext}"));
                match self.ask(&Request::CaptureSettings).await {
                    Ok(Response::CaptureSettings(settings)) => Outgoing::Settings(settings),
                    Ok(other) => error(None, format!("unexpected answer: {other:?}")),
                    Err(e) => error(None, format!("daemon not reachable: {e}")),
                }
            }
            Incoming::Capture(capture) => self.capture(capture).await,
        }
    }

    /// Hand a caught download to the daemon. The verdict tells the extension whether
    /// to cancel the browser's own download (take) or let it continue (release).
    async fn capture(&mut self, c: Capture) -> Outgoing {
        if c.modifiers.prevent {
            return Outgoing::Verdict {
                id: c.id,
                action: Action::Release,
            };
        }
        let add = AddRequest {
            url: c.url.clone(),
            name: c.filename.clone(),
            headers: c.headers.clone(),
            ..Default::default()
        };
        match self.ask(&Request::Add(add)).await {
            Ok(Response::Added { id }) => {
                let forced = if c.modifiers.force {
                    " (forced with Alt)"
                } else {
                    ""
                };
                self.log.line(&format!("caught {} as #{id}{forced}", c.url));
                Outgoing::Verdict {
                    id: c.id,
                    action: Action::Take,
                }
            }
            Ok(other) => {
                // E.g. already in the list: let the browser have it rather than lose it.
                self.log.line(&format!("released {}: {other:?}", c.url));
                Outgoing::Verdict {
                    id: c.id,
                    action: Action::Release,
                }
            }
            Err(e) => error(Some(c.id), format!("daemon not reachable: {e}")),
        }
    }

    /// Ask the daemon, connecting (and starting it if needed) on first use, and
    /// reconnecting once if the daemon restarted since.
    async fn ask(&mut self, req: &Request) -> io::Result<Response> {
        if let Some(daemon) = self.daemon.as_mut()
            && let Ok(response) = daemon.call(req).await
        {
            return Ok(response);
        }
        let mut daemon = Client::connect_or_start(&self.paths).await?;
        let response = daemon.call(req).await?;
        self.daemon = Some(daemon);
        Ok(response)
    }
}

fn error(id: Option<String>, message: String) -> Outgoing {
    Outgoing::Error { id, message }
}

/// Append-only log file; logging must never break the host.
struct Log(Option<std::fs::File>);

impl Log {
    fn open(paths: &Paths) -> Self {
        let _ = std::fs::create_dir_all(&paths.data_dir);
        let file = std::fs::File::options()
            .create(true)
            .append(true)
            .open(paths.data_dir.join("nmhost.log"));
        Self(file.ok())
    }

    fn line(&mut self, text: &str) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Some(f) = &mut self.0 {
            let _ = writeln!(f, "[{secs}] {text}");
        }
    }
}
