//! The host program end to end, driven like a browser drives it: framed JSON over
//! stdin/stdout, with a real daemon behind it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use slipstream_core::ipc::{Client, DownloadInfo, Paths, Request, Response, Status};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ssh-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A daemon living in `home`, in this process.
async fn daemon(home: &Path) -> Paths {
    let paths = Paths::new(Some(home)).unwrap();
    let p = paths.clone();
    tokio::spawn(async move { slipstream_daemon::run(&p).await });
    for _ in 0..200 {
        if let Ok(mut c) = Client::connect(&paths).await
            && c.call(&Request::Ping).await.is_ok()
        {
            return paths;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon did not start");
}

/// The host program, started the way Chrome starts it.
struct Host {
    _child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Host {
    fn start(home: &Path, env: &[(&str, &str)]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_slipstream-nmhost"))
            .arg("chrome-extension://mlmcfpobghbehfiebmeedgpibmbeoajo/")
            .env("SLIPSTREAM_HOME", home)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Self {
            _child: child,
            stdin,
            stdout,
        }
    }

    async fn send(&mut self, msg: Value) {
        let body = serde_json::to_vec(&msg).unwrap();
        self.stdin
            .write_all(&(body.len() as u32).to_le_bytes())
            .await
            .unwrap();
        self.stdin.write_all(&body).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let read = async {
            let len = self.stdout.read_u32_le().await.unwrap();
            let mut body = vec![0; len as usize];
            self.stdout.read_exact(&mut body).await.unwrap();
            serde_json::from_slice(&body).expect("stdout must only carry framed JSON")
        };
        tokio::time::timeout(Duration::from_secs(10), read)
            .await
            .expect("no reply")
    }

    async fn ask(&mut self, msg: Value) -> Value {
        self.send(msg).await;
        self.recv().await
    }
}

async fn list(paths: &Paths) -> Vec<DownloadInfo> {
    let mut c = Client::connect(paths).await.unwrap();
    match c.call(&Request::List).await.unwrap() {
        Response::Downloads { downloads } => downloads,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_gets_capture_settings() {
    let home = temp_dir("hello");
    daemon(&home).await;
    let mut host = Host::start(&home, &[]);
    let reply = host
        .ask(json!({"v": 1, "type": "hello", "browser": "Chrome/141", "ext": "0.1.0"}))
        .await;
    assert_eq!(reply["v"], 1);
    assert_eq!(reply["type"], "settings");
    assert_eq!(reply["enabled"], true);
    assert!(
        reply["captureExt"]
            .as_array()
            .unwrap()
            .contains(&json!("zip")),
        "{reply}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn captured_download_is_taken_with_its_cookies() {
    let home = temp_dir("capture");
    let paths = daemon(&home).await;
    let base = format!("http://{}", testserver::spawn().await.unwrap());
    let mut host = Host::start(&home, &[]);
    let reply = host
        .ask(json!({
            "v": 1, "type": "capture", "id": "c-1",
            "url": format!("{base}/protected/p.bin?size=5000"),
            "headers": [["Cookie", testserver::SESSION_COOKIE]],
            "filename": "p.bin",
            "modifiers": {"force": false, "prevent": false}
        }))
        .await;
    assert_eq!(
        reply,
        json!({"v": 1, "type": "verdict", "id": "c-1", "action": "take"})
    );

    // The daemon downloads it with the browser's cookie (403 without it).
    for _ in 0..200 {
        let d = list(&paths).await;
        if d.first().is_some_and(|d| d.status == Status::Complete) {
            let want = testserver::content::bytes(
                testserver::content::file_seed("p.bin", None, 0),
                0,
                5000,
            );
            assert!(std::fs::read(paths.downloads.join("p.bin")).unwrap() == want);
            return;
        }
        assert!(d.first().is_none_or(|d| d.status != Status::Error), "{d:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("download never completed: {:?}", list(&paths).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_click_is_released() {
    let home = temp_dir("prevent");
    let paths = daemon(&home).await;
    let mut host = Host::start(&home, &[]);
    let reply = host
        .ask(json!({
            "v": 1, "type": "capture", "id": "c-2", "url": "https://example.com/a.zip",
            "modifiers": {"force": false, "prevent": true}
        }))
        .await;
    assert_eq!(reply["action"], "release");
    assert!(list(&paths).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_protocol_version_is_refused() {
    let home = temp_dir("version");
    daemon(&home).await;
    let mut host = Host::start(&home, &[]);
    let reply = host.ask(json!({"v": 2, "type": "hello"})).await;
    assert_eq!(reply["type"], "error");
    // The host keeps serving after a bad message.
    let reply = host.ask(json!({"v": 1, "type": "hello"})).await;
    assert_eq!(reply["type"], "settings");
}

#[tokio::test(flavor = "multi_thread")]
async fn no_daemon_means_a_quick_error() {
    let home = temp_dir("nodaemon");
    // No daemon here, and starting one is impossible: the extension must hear back
    // fast so it can give the download back to the browser.
    let mut host = Host::start(
        &home,
        &[("SLIPSTREAM_DAEMON", "/nonexistent/slipstream-daemon")],
    );
    let started = std::time::Instant::now();
    let reply = host
        .ask(json!({"v": 1, "type": "capture", "id": "c-3", "url": "https://example.com/a.zip"}))
        .await;
    assert_eq!(reply["type"], "error");
    assert_eq!(reply["id"], "c-3");
    assert!(
        reply["message"]
            .as_str()
            .unwrap()
            .contains("daemon not reachable"),
        "{reply}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}
