//! Slipstream command-line client.

use std::io::Write;
use std::process::ExitCode;
use std::time::Instant;

use slipstream_core::download::{self, Error, Progress, Request};

const USAGE: &str = "\
usage: slipstream get URL [options]

options:
  -d, --dir DIR          folder to save into (default: the current folder)
  -o, --output NAME      file name (default: from the server)
  -c, --connections N    most connections per file (default 8)
  -l, --limit RATE       speed limit in bytes/s, e.g. 500K or 2M (default none)
  -H, --header 'K: V'    extra request header, repeatable (e.g. 'Cookie: a=b')

Run the same command again to resume an interrupted download.";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let req = match args.first().map(String::as_str) {
        Some("get") => match parse_get(&args[1..]) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        },
        Some("-V" | "--version") => {
            println!("slipstream {}", slipstream_core::VERSION);
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
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
