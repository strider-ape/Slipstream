//! Slipstream command-line client.

use std::io::Write;

use std::process::ExitCode;
use std::time::Instant;

use slipstream_core::download::{self, Error, Progress, Request};
use slipstream_core::ipc::{self, AddRequest, Client, DownloadInfo, Paths, Response};

const USAGE: &str = "\
usage:
  slipstream get URL [options]      download right here, in this window
  slipstream add URL [options]      hand the download to the background daemon
  slipstream list                   the daemon's downloads
  slipstream watch                  live view of the daemon's downloads (Ctrl+C to quit)
  slipstream pause ID | resume ID
  slipstream remove ID [--delete-file]
  slipstream daemon status | stop
  slipstream autostart on | off     start Slipstream in the tray when you log in
  slipstream register-browsers      let Chrome/Edge/Brave/Vivaldi/Firefox hand downloads over
  slipstream unregister-browsers

options for get and add:
  -d, --dir DIR          folder to save into (get: this folder; add: your Downloads folder)
  -o, --output NAME      file name (default: from the server)
  -c, --connections N    most connections per file (default 8)
  -l, --limit RATE       speed limit in bytes/s, e.g. 500K or 2M (default none)
  -H, --header 'K: V'    extra request header, repeatable (e.g. 'Cookie: a=b')

`get` resumes when you run the same command again; the daemon resumes on its own.";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let id = |s: &str| {
        s.parse::<i64>()
            .map_err(|_| format!("not a download number: {s}"))
    };
    let result = match words[..] {
        ["get", ..] => return get(&args[1..]).await,
        ["add", ..] => add(&args[1..]).await,
        ["list"] => list().await,
        ["watch"] => watch().await,
        ["pause", n] => match id(n) {
            Ok(id) => command(ipc::Request::Pause { id }, &format!("Paused #{id}")).await,
            Err(e) => Err(e),
        },
        ["resume", n] => match id(n) {
            Ok(id) => command(ipc::Request::Resume { id }, &format!("Resumed #{id}")).await,
            Err(e) => Err(e),
        },
        ["remove", n] | ["remove", n, "--delete-file"] => match id(n) {
            Ok(id) => {
                let delete_file = words.len() == 3;
                let req = ipc::Request::Remove { id, delete_file };
                command(req, &format!("Removed #{id}")).await
            }
            Err(e) => Err(e),
        },
        ["daemon", "status"] => daemon_status().await,
        ["daemon", "stop"] => command(ipc::Request::Shutdown, "Daemon stopping").await,
        ["autostart", on @ ("on" | "off")] => autostart(on),
        ["register-browsers" | "--register-browsers"] => nmhost("--register"),
        ["unregister-browsers" | "--unregister-browsers"] => nmhost("--unregister"),
        ["-V" | "--version"] => {
            println!("slipstream {}", slipstream_core::VERSION);
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `get`: download in this process, with a progress line.
async fn get(args: &[String]) -> ExitCode {
    let req = match parse_get(args) {
        Ok(req) => req,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut meter = Meter::new();
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let result = download::download(&req, ctrl_c, |p| meter.show(p)).await;
    eprintln!();
    match result {
        Ok(out) => {
            if out.restarted {
                eprintln!(
                    "note: the earlier partial download couldn't be continued, so it started over"
                );
            }
            println!("Saved {} ({})", out.path.display(), human(out.size));
            ExitCode::SUCCESS
        }
        Err(Error::Cancelled) => {
            eprintln!("Paused. Run the same command again to resume.");
            ExitCode::from(130)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `add`: hand a download to the daemon (starting it if needed).
async fn add(args: &[String]) -> Result<(), String> {
    let explicit_dir = args.iter().any(|a| a == "-d" || a == "--dir");
    let r = parse_get(args)?;
    // The daemon has its own working folder, so send an absolute path.
    let dir = explicit_dir.then(|| match r.dir.is_absolute() {
        true => r.dir.clone(),
        false => std::env::current_dir().unwrap_or_default().join(&r.dir),
    });
    let req = AddRequest {
        url: r.url,
        dir,
        name: r.name,
        headers: r.headers,
        connections: r.connections,
        speed_limit: r.speed_limit,
    };
    let mut client = connect_or_start().await?;
    match call(&mut client, ipc::Request::Add(req)).await? {
        Response::Added { id } => {
            println!("Added #{id}. Follow it with: slipstream watch");
            Ok(())
        }
        other => Err(describe(other)),
    }
}

async fn list() -> Result<(), String> {
    let mut client = connect().await?;
    match call(&mut client, ipc::Request::List).await? {
        Response::Downloads { downloads } => {
            print_table(&downloads);
            Ok(())
        }
        other => Err(describe(other)),
    }
}

/// Redraw the list every time the daemon sends a fresh one.
async fn watch() -> Result<(), String> {
    let mut client = connect().await?;
    let mut msg = call(&mut client, ipc::Request::Watch).await?;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        if let Response::Downloads { downloads } = &msg {
            // Clear the screen and go to the top-left corner.
            print!("\x1b[2J\x1b[H");
            print_table(downloads);
            println!("\n(Ctrl+C to stop watching; downloads keep going)");
        }
        msg = tokio::select! {
            m = client.next() => match m.map_err(|e| e.to_string())? {
                Some(m) => m,
                None => return Err("the daemon stopped".into()),
            },
            _ = &mut ctrl_c => return Ok(()),
        };
    }
}

/// Send one command that answers `Ok`, and print `done`.
async fn command(req: ipc::Request, done: &str) -> Result<(), String> {
    let mut client = connect().await?;
    match call(&mut client, req).await? {
        Response::Ok => {
            println!("{done}");
            Ok(())
        }
        other => Err(describe(other)),
    }
}

async fn daemon_status() -> Result<(), String> {
    let paths = Paths::from_env().map_err(|e| e.to_string())?;
    match Client::connect(&paths).await {
        Ok(mut client) => match call(&mut client, ipc::Request::Ping).await? {
            Response::Pong { version, .. } => {
                println!(
                    "running (version {version}), data in {}",
                    paths.data_dir.display()
                );
                Ok(())
            }
            other => Err(describe(other)),
        },
        Err(_) => {
            println!("not running");
            Ok(())
        }
    }
}

fn autostart(on: &str) -> Result<(), String> {
    let status = std::process::Command::new(ipc::daemon_exe())
        .args(["--autostart", on])
        .status()
        .map_err(|e| format!("could not run {}: {e}", ipc::daemon_exe().display()))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "changing autostart failed".into())
}

/// Run the browser host (next to this program) to (un)register it with browsers.
fn nmhost(flag: &str) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = me.with_file_name(format!("slipstream-nmhost{}", std::env::consts::EXE_SUFFIX));
    let status = std::process::Command::new(&exe)
        .arg(flag)
        .status()
        .map_err(|e| format!("could not run {}: {e}", exe.display()))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "browser registration failed".into())
}

async fn call(client: &mut Client, req: ipc::Request) -> Result<Response, String> {
    client
        .call(&req)
        .await
        .map_err(|e| format!("lost the daemon: {e}"))
}

fn describe(response: Response) -> String {
    match response {
        Response::Error { message } => message,
        other => format!("unexpected answer: {other:?}"),
    }
}

async fn connect() -> Result<Client, String> {
    let paths = Paths::from_env().map_err(|e| e.to_string())?;
    Client::connect(&paths)
        .await
        .map_err(|_| "the daemon is not running (`slipstream add URL` starts it)".into())
}

/// Connect, starting the daemon in the background first if it is not running.
async fn connect_or_start() -> Result<Client, String> {
    let paths = Paths::from_env().map_err(|e| e.to_string())?;
    Client::connect_or_start(&paths)
        .await
        .map_err(|e| e.to_string())
}

fn print_table(downloads: &[DownloadInfo]) {
    if downloads.is_empty() {
        println!("No downloads.");
        return;
    }
    println!(
        "{:>4}  {:<11} {:>5}  {:>10}  {:>11}  NAME",
        "ID", "STATUS", "DONE", "SIZE", "SPEED"
    );
    for d in downloads {
        let done = match d.size {
            Some(size) if size > 0 => format!("{}%", d.downloaded * 100 / size),
            _ => String::new(),
        };
        let speed = if d.speed > 0 {
            format!("{}/s", human(d.speed))
        } else {
            String::new()
        };
        let mut name = d.name.clone().unwrap_or_else(|| d.url.clone());
        if d.connections > 0 {
            name += &format!("  ({} conn)", d.connections);
        }
        let size = d.size.map(human).unwrap_or_default();
        let status = d.status.as_str();
        println!(
            "{:>4}  {status:<11} {done:>5}  {size:>10}  {speed:>11}  {name}",
            d.id
        );
        if let Some(error) = &d.error {
            println!("      {error}");
        }
    }
}

fn parse_get(args: &[String]) -> Result<Request, String> {
    let mut req = Request::default();
    let mut url = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "-d" | "--dir" => req.dir = value()?.into(),
            "-o" | "--output" => req.name = Some(value()?.clone()),
            "-c" | "--connections" => {
                req.connections = value()?
                    .parse()
                    .map_err(|_| "--connections takes a number")?;
            }
            "-l" | "--limit" => req.speed_limit = parse_rate(value()?)?,
            "-H" | "--header" => {
                let h = value()?;
                let (k, v) = h
                    .split_once(':')
                    .ok_or(format!("bad header (want 'Name: value'): {h}"))?;
                req.headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            a if url.is_none() => url = Some(a.to_owned()),
            a => return Err(format!("unexpected argument {a}")),
        }
    }
    req.url = url.ok_or("missing URL")?;
    if req.dir.as_os_str().is_empty() {
        req.dir = std::env::current_dir().map_err(|e| e.to_string())?;
    }
    Ok(req)
}

/// `500K` → 512000, `2M` → 2097152, `1000` → 1000.
fn parse_rate(s: &str) -> Result<u64, String> {
    let (num, mult) = match s.to_ascii_uppercase().chars().last() {
        Some('K') => (&s[..s.len() - 1], 1024),
        Some('M') => (&s[..s.len() - 1], 1024 * 1024),
        _ => (s, 1),
    };
    let n: f64 = num.trim().parse().map_err(|_| format!("bad rate: {s}"))?;
    Ok((n * mult as f64) as u64)
}

/// One-line progress display on stderr: percent, bytes, speed, time left.
struct Meter {
    last: Option<(Instant, u64)>,
    /// Smoothed speed in bytes/s (exponential moving average).
    speed: f64,
}

impl Meter {
    fn new() -> Self {
        Self {
            last: None,
            speed: 0.0,
        }
    }

    fn show(&mut self, p: Progress) {
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

        let mut line = match p.size {
            Some(size) if size > 0 => format!(
                "{:5.1}%  {} of {}",
                p.downloaded as f64 * 100.0 / size as f64,
                human(p.downloaded),
                human(size)
            ),
            _ => human(p.downloaded),
        };
        if self.speed > 0.0 {
            line += &format!("  {}/s", human(self.speed as u64));
            if let Some(size) = p.size {
                let secs = size.saturating_sub(p.downloaded) as f64 / self.speed;
                line += &format!("  ETA {}:{:02}", secs as u64 / 60, secs as u64 % 60);
            }
        }
        if p.connections > 0 {
            line += &format!("  {} conn", p.connections);
        }
        // \r returns to the line start; padding wipes leftovers of a longer line.
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r{line:<60}");
        let _ = err.flush();
    }
}

/// 1536 → "1.5 KiB".
fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn parses_get() {
        let req = parse_get(&args(&[
            "https://x/a.zip",
            "-d",
            "out",
            "-H",
            "Cookie: a=b; c=d",
            "-o",
            "b.zip",
        ]))
        .unwrap();
        assert_eq!(req.url, "https://x/a.zip");
        assert_eq!(req.dir, std::path::Path::new("out"));
        assert_eq!(req.name.as_deref(), Some("b.zip"));
        assert_eq!(
            req.headers,
            vec![("Cookie".to_owned(), "a=b; c=d".to_owned())]
        );
        assert!(parse_get(&args(&["-d"])).is_err());
        assert!(parse_get(&args(&[])).is_err());
        assert!(parse_get(&args(&["u1", "u2"])).is_err());
    }

    #[test]
    fn rates() {
        assert_eq!(parse_rate("1000"), Ok(1000));
        assert_eq!(parse_rate("500K"), Ok(512_000));
        assert_eq!(parse_rate("1.5m"), Ok(1_572_864));
        assert!(parse_rate("fast").is_err());
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
