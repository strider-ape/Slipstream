//! Start Slipstream when the user logs in (optional, `slipstream autostart on|off`).
//! Windows: a value under HKCU\...\Run. Linux: an XDG autostart entry, which desktop
//! sessions (GNOME, KDE, XFCE...) start at login.
//!
//! What starts is the app in the tray (`slipstream-gui --tray`, which starts the
//! daemon too), or just the daemon where the app isn't installed next to it.

use std::io;
use std::path::{Path, PathBuf};

/// Turn starting at login on or off; returns a line describing what changed.
pub fn set(enable: bool, daemon: &Path) -> io::Result<String> {
    let command = command(daemon);
    #[cfg(windows)]
    {
        windows(enable, &command)
    }
    #[cfg(unix)]
    {
        linux(enable, &command)
    }
}

/// The command line to run at login, quoted.
fn command(daemon: &Path) -> String {
    let gui = daemon.with_file_name(format!("slipstream-gui{}", std::env::consts::EXE_SUFFIX));
    if gui.is_file() {
        format!("\"{}\" --tray", gui.display())
    } else {
        format!("\"{}\"", daemon.display())
    }
}

#[cfg(windows)]
fn windows(enable: bool, command: &str) -> io::Result<String> {
    use std::process::Command;
    // The same value the installer writes, so the two never start Slipstream twice.
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let status = if enable {
        Command::new("reg")
            .args([
                "add",
                KEY,
                "/v",
                "Slipstream",
                "/t",
                "REG_SZ",
                "/d",
                command,
                "/f",
            ])
            .output()?
            .status
    } else {
        // Deleting a value that isn't there "fails"; that's fine.
        Command::new("reg")
            .args(["delete", KEY, "/v", "Slipstream", "/f"])
            .output()?;
        return Ok(format!(r"removed {KEY}\Slipstream"));
    };
    if !status.success() {
        return Err(io::Error::other("reg add failed"));
    }
    Ok(format!(r"added {KEY}\Slipstream: {command}"))
}

#[cfg(unix)]
fn linux(enable: bool, command: &str) -> io::Result<String> {
    let dir = autostart_dir()?;
    let file = dir.join("slipstream.desktop");
    // Older versions wrote this one (daemon only).
    remove(&dir.join("slipstream-daemon.desktop"))?;
    if !enable {
        remove(&file)?;
        return Ok(format!("removed {}", file.display()));
    }
    std::fs::create_dir_all(&dir)?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Slipstream\n\
         Comment=Download manager, in the tray\nExec={command}\nIcon=slipstream\n\
         Terminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    );
    std::fs::write(&file, entry)?;
    Ok(format!("wrote {}: {command}", file.display()))
}

#[cfg(unix)]
fn autostart_dir() -> io::Result<PathBuf> {
    let config: PathBuf = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(c) => c.into(),
        None => {
            Path::new(&std::env::var_os("HOME").ok_or(io::ErrorKind::NotFound)?).join(".config")
        }
    };
    Ok(config.join("autostart"))
}

#[cfg(unix)]
fn remove(file: &Path) -> io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
