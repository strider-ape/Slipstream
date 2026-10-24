//! Start the daemon when the user logs in (optional, `slipstream autostart on|off`).
//! Windows: a value under HKCU\...\Run. Linux: an XDG autostart entry, which desktop
//! sessions (GNOME, KDE, XFCE...) start at login.

use std::io;
use std::path::Path;

/// Turn starting at login on or off; returns a line describing what changed.
pub fn set(enable: bool, daemon: &Path) -> io::Result<String> {
    #[cfg(windows)]
    {
        windows(enable, daemon)
    }
    #[cfg(unix)]
    {
        linux(enable, daemon)
    }
}

#[cfg(windows)]
fn windows(enable: bool, daemon: &Path) -> io::Result<String> {
    use std::process::Command;
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let status = if enable {
        let value = format!("\"{}\"", daemon.display());
        Command::new("reg")
            .args([
                "add",
                KEY,
                "/v",
                "Slipstream",
                "/t",
                "REG_SZ",
                "/d",
                &value,
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
    Ok(format!(r"added {KEY}\Slipstream"))
}

#[cfg(unix)]
fn linux(enable: bool, daemon: &Path) -> io::Result<String> {
    let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(c) => c.into(),
        None => {
            Path::new(&std::env::var_os("HOME").ok_or(io::ErrorKind::NotFound)?).join(".config")
        }
    };
    let file = config.join("autostart").join("slipstream-daemon.desktop");
    if !enable {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        return Ok(format!("removed {}", file.display()));
    }
    std::fs::create_dir_all(file.parent().unwrap())?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Slipstream download manager\n\
         Exec=\"{}\"\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        daemon.display()
    );
    std::fs::write(&file, entry)?;
    Ok(format!("wrote {}", file.display()))
}
