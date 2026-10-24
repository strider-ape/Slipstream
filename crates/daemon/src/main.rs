//! Slipstream background daemon.
//!
//!     slipstream-daemon                  run (normally started by `slipstream add`)
//!     slipstream-daemon --autostart on   start at login (off to undo)
//!
//! `SLIPSTREAM_HOME=DIR` keeps the catalogue and control socket in DIR instead of
//! the per-user locations (tests, portable use).

use std::process::ExitCode;

use slipstream_core::ipc::Paths;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => return fail(&e.to_string()),
    };
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => {}
        ["--autostart", on @ ("on" | "off")] => {
            let exe = std::env::current_exe();
            return match exe.and_then(|exe| slipstream_daemon::autostart::set(on == "on", &exe)) {
                Ok(done) => {
                    println!("{done}");
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&e.to_string()),
            };
        }
        ["-V" | "--version"] => {
            println!("slipstream-daemon {}", slipstream_core::VERSION);
            return ExitCode::SUCCESS;
        }
        _ => return fail("usage: slipstream-daemon [--autostart on|off]"),
    }
    match slipstream_daemon::run(&paths).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("slipstream-daemon: already running");
            ExitCode::from(3)
        }
        Err(e) => fail(&e.to_string()),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("slipstream-daemon: {message}");
    ExitCode::FAILURE
}
