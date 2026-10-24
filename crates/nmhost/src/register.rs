//! Tell browsers where the native messaging host is (SPEC-browser-capture §6).
//! Linux: a JSON manifest in each installed browser's `NativeMessagingHosts` folder.
//! Windows: the manifests live in Slipstream's data folder, and a registry value
//! under HKCU points each browser at them (Brave and Vivaldi read Chrome's key).

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use slipstream_core::ipc::Paths;

pub const HOST_NAME: &str = "org.slipstream.host";
/// Chromium-family extensions allowed to start the host (see extension/README.md).
pub const CHROME_EXTENSION_IDS: [&str; 1] = ["mlmcfpobghbehfiebmeedgpibmbeoajo"];
pub const FIREFOX_EXTENSION_ID: &str = "slipstream@strider-ape.github.io";

/// `--register` / `--unregister`, run by the user (not a browser): stdout is fine here.
pub fn cli(enable: bool) -> ExitCode {
    let run = || -> io::Result<Vec<String>> {
        let exe = std::env::current_exe()?;
        let paths = Paths::from_env()?;
        let home = home_dir()?;
        if enable {
            register(&exe, &paths.data_dir, &home)
        } else {
            unregister(&paths.data_dir, &home)
        }
    };
    match run() {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn home_dir() -> io::Result<PathBuf> {
    #[cfg(unix)]
    let home = std::env::var_os("HOME");
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    home.map(PathBuf::from)
        .ok_or_else(|| io::ErrorKind::NotFound.into())
}

pub fn chromium_manifest(exe: &Path) -> String {
    let origins: Vec<String> = CHROME_EXTENSION_IDS
        .iter()
        .map(|id| format!("chrome-extension://{id}/"))
        .collect();
    pretty(serde_json::json!({
        "name": HOST_NAME,
        "description": "Slipstream download manager",
        "path": exe,
        "type": "stdio",
        "allowed_origins": origins,
    }))
}

pub fn firefox_manifest(exe: &Path) -> String {
    pretty(serde_json::json!({
        "name": HOST_NAME,
        "description": "Slipstream download manager",
        "path": exe,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
    }))
}

fn pretty(v: serde_json::Value) -> String {
    serde_json::to_string_pretty(&v).expect("valid JSON") + "\n"
}

/// (browser, its config folder, where host manifests go, Firefox-style manifest?)
#[cfg(unix)]
const BROWSERS: [(&str, &str, &str, bool); 6] = [
    (
        "Chrome",
        ".config/google-chrome",
        ".config/google-chrome/NativeMessagingHosts",
        false,
    ),
    (
        "Chromium",
        ".config/chromium",
        ".config/chromium/NativeMessagingHosts",
        false,
    ),
    (
        "Edge",
        ".config/microsoft-edge",
        ".config/microsoft-edge/NativeMessagingHosts",
        false,
    ),
    (
        "Brave",
        ".config/BraveSoftware/Brave-Browser",
        ".config/BraveSoftware/Brave-Browser/NativeMessagingHosts",
        false,
    ),
    (
        "Vivaldi",
        ".config/vivaldi",
        ".config/vivaldi/NativeMessagingHosts",
        false,
    ),
    (
        "Firefox",
        ".mozilla",
        ".mozilla/native-messaging-hosts",
        true,
    ),
];

/// Register for every installed browser; returns one line per browser.
#[cfg(unix)]
pub fn register(exe: &Path, _data_dir: &Path, home: &Path) -> io::Result<Vec<String>> {
    let mut lines = Vec::new();
    for (name, config, hosts, firefox) in BROWSERS {
        if !home.join(config).is_dir() {
            lines.push(format!("skipped {name} (not installed)"));
            continue;
        }
        let dir = home.join(hosts);
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{HOST_NAME}.json"));
        let manifest = if firefox {
            firefox_manifest(exe)
        } else {
            chromium_manifest(exe)
        };
        std::fs::write(&file, manifest)?;
        lines.push(format!("registered {name}: {}", file.display()));
    }
    Ok(lines)
}

#[cfg(unix)]
pub fn unregister(_data_dir: &Path, home: &Path) -> io::Result<Vec<String>> {
    let mut lines = Vec::new();
    for (name, _, hosts, _) in BROWSERS {
        let file = home.join(hosts).join(format!("{HOST_NAME}.json"));
        if std::fs::remove_file(&file).is_ok() {
            lines.push(format!("unregistered {name}"));
        }
    }
    Ok(lines)
}

/// (browser, registry key holding host names, Firefox-style manifest?)
#[cfg(windows)]
const KEYS: [(&str, &str, bool); 4] = [
    (
        "Chrome, Brave, Vivaldi",
        r"HKCU\Software\Google\Chrome\NativeMessagingHosts",
        false,
    ),
    (
        "Chromium",
        r"HKCU\Software\Chromium\NativeMessagingHosts",
        false,
    ),
    (
        "Edge",
        r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts",
        false,
    ),
    (
        "Firefox",
        r"HKCU\Software\Mozilla\NativeMessagingHosts",
        true,
    ),
];

#[cfg(windows)]
fn manifest_paths(data_dir: &Path) -> (PathBuf, PathBuf) {
    let dir = data_dir.join("native-messaging");
    (
        dir.join(format!("{HOST_NAME}.json")),
        dir.join(format!("{HOST_NAME}.firefox.json")),
    )
}

#[cfg(windows)]
pub fn register(exe: &Path, data_dir: &Path, _home: &Path) -> io::Result<Vec<String>> {
    let (chromium, firefox) = manifest_paths(data_dir);
    std::fs::create_dir_all(chromium.parent().unwrap())?;
    std::fs::write(&chromium, chromium_manifest(exe))?;
    std::fs::write(&firefox, firefox_manifest(exe))?;
    let mut lines = Vec::new();
    for (name, key, is_firefox) in KEYS {
        let manifest = if is_firefox { &firefox } else { &chromium };
        let key = format!(r"{key}\{HOST_NAME}");
        let path = manifest.to_string_lossy();
        reg(&["add", &key, "/ve", "/t", "REG_SZ", "/d", &path, "/f"])?;
        lines.push(format!("registered {name}: {key}"));
    }
    Ok(lines)
}

#[cfg(windows)]
pub fn unregister(data_dir: &Path, _home: &Path) -> io::Result<Vec<String>> {
    let mut lines = Vec::new();
    for (name, key, _) in KEYS {
        if reg(&["delete", &format!(r"{key}\{HOST_NAME}"), "/f"]).is_ok() {
            lines.push(format!("unregistered {name}"));
        }
    }
    let (chromium, firefox) = manifest_paths(data_dir);
    let _ = std::fs::remove_file(chromium);
    let _ = std::fs::remove_file(firefox);
    Ok(lines)
}

#[cfg(windows)]
fn reg(args: &[&str]) -> io::Result<String> {
    let out = std::process::Command::new("reg").args(args).output()?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(io::Error::other(format!("reg {}: {why}", args[0])));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ssnm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn manifests_name_our_extensions() {
        let exe = Path::new("/opt/slipstream/slipstream-nmhost");
        let chrome: serde_json::Value = serde_json::from_str(&chromium_manifest(exe)).unwrap();
        assert_eq!(chrome["name"], HOST_NAME);
        assert_eq!(chrome["type"], "stdio");
        assert_eq!(chrome["path"], "/opt/slipstream/slipstream-nmhost");
        assert_eq!(
            chrome["allowed_origins"][0],
            "chrome-extension://mlmcfpobghbehfiebmeedgpibmbeoajo/"
        );
        let firefox: serde_json::Value = serde_json::from_str(&firefox_manifest(exe)).unwrap();
        assert_eq!(firefox["allowed_extensions"][0], FIREFOX_EXTENSION_ID);
        assert!(firefox.get("allowed_origins").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn registers_only_installed_browsers() {
        let home = temp("home");
        std::fs::create_dir_all(home.join(".config/google-chrome")).unwrap();
        std::fs::create_dir_all(home.join(".mozilla")).unwrap();
        let exe = Path::new("/usr/bin/slipstream-nmhost");

        let lines = register(exe, &home, &home).unwrap();
        assert!(
            lines.iter().any(|l| l.starts_with("registered Chrome")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "skipped Chromium (not installed)"),
            "{lines:?}"
        );
        let chrome =
            home.join(".config/google-chrome/NativeMessagingHosts/org.slipstream.host.json");
        let firefox = home.join(".mozilla/native-messaging-hosts/org.slipstream.host.json");
        assert_eq!(
            std::fs::read_to_string(&chrome).unwrap(),
            chromium_manifest(exe)
        );
        assert_eq!(
            std::fs::read_to_string(&firefox).unwrap(),
            firefox_manifest(exe)
        );
        assert!(!home.join(".config/chromium").exists());

        let lines = unregister(&home, &home).unwrap();
        assert_eq!(lines, vec!["unregistered Chrome", "unregistered Firefox"]);
        assert!(!chrome.exists() && !firefox.exists());
    }

    /// Writes the real HKCU, so only on CI runners.
    #[cfg(windows)]
    #[test]
    fn registry_round_trip() {
        if std::env::var_os("CI").is_none() {
            return;
        }
        let data = temp("data");
        let exe = Path::new(r"C:\Program Files\Slipstream\slipstream-nmhost.exe");
        register(exe, &data, &data).unwrap();
        let key = format!(r"{}\{HOST_NAME}", KEYS[0].1);
        let value = reg(&["query", &key, "/ve"]).unwrap();
        assert!(value.contains("native-messaging"), "{value}");
        let (chromium, _) = manifest_paths(&data);
        assert_eq!(
            std::fs::read_to_string(&chromium).unwrap(),
            chromium_manifest(exe)
        );
        unregister(&data, &data).unwrap();
        assert!(reg(&["query", &key, "/ve"]).is_err());
    }
}
