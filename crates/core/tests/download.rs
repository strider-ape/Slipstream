//! The single-connection engine against the local test server (TEST-PLAN §A).

use std::future::pending;
use std::path::{Path, PathBuf};
use std::time::Duration;

use slipstream_core::download::{Error, Outcome, Request, download};
use slipstream_core::storage::{part_path, sidecar_path};
use testserver::content;

async fn server() -> String {
    format!("http://{}", testserver::spawn().await.unwrap())
}

/// A fresh, empty folder per test.
fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("slipstream-dl-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn request(url: String, dir: &Path) -> Request {
    Request {
        url,
        dir: dir.to_owned(),
        connections: 1,
        ..Default::default()
    }
}

async fn run(req: &Request) -> Result<Outcome, Error> {
    download(req, pending(), |_| {}).await
}

/// Run until `after`, then stop cleanly (like pressing Ctrl+C).
async fn run_and_stop(req: &Request, after: Duration) -> Error {
    download(req, tokio::time::sleep(after), |_| {})
        .await
        .unwrap_err()
}

fn expected(name: &str, version: u64, size: usize) -> Vec<u8> {
    content::bytes(content::file_seed(name, None, version), 0, size)
}

fn assert_finished(out: &Outcome, want: &[u8]) {
    assert!(std::fs::read(&out.path).unwrap() == want, "content differs");
    assert!(!part_path(&out.path).exists(), "part file left behind");
    assert!(!sidecar_path(&out.path).exists(), "sidecar left behind");
}

#[tokio::test]
async fn whole_file() {
    let (base, dir) = (server().await, temp_dir("whole"));
    let out = run(&request(format!("{base}/file/a.bin?size=3000000"), &dir))
        .await
        .unwrap();
    assert_eq!(out.path, dir.join("a.bin"));
    assert_eq!(
        (out.size, out.resumed_from, out.restarted),
        (3_000_000, 0, false)
    );
    assert_finished(&out, &expected("a.bin", 0, 3_000_000));
}

#[tokio::test]
async fn stop_and_resume() {
    let (base, dir) = (server().await, temp_dir("resume"));
    let req = request(
        format!("{base}/file/r.bin?size=2000000&throttle=2000000"),
        &dir,
    );
    assert!(matches!(
        run_and_stop(&req, Duration::from_millis(400)).await,
        Error::Cancelled
    ));
    assert!(part_path(&dir.join("r.bin")).exists());

    let out = run(&req).await.unwrap();
    assert!(out.resumed_from > 0, "should continue, not start over");
    assert!(!out.restarted);
    assert_finished(&out, &expected("r.bin", 0, 2_000_000));
}

#[tokio::test]
async fn crash_and_resume() {
    // Dropping the download mid-way is what a crash looks like: no clean save,
    // only the periodic one (every 2 s).
    let (base, dir) = (server().await, temp_dir("crash"));
    let req = request(
        format!("{base}/file/c.bin?size=3000000&throttle=1000000"),
        &dir,
    );
    let crashed = tokio::time::timeout(Duration::from_millis(2500), run(&req)).await;
    assert!(
        crashed.is_err(),
        "should still be running when it 'crashes'"
    );

    let out = run(&req).await.unwrap();
    assert!(
        out.resumed_from >= 1_000_000,
        "resumed from {}",
        out.resumed_from
    );
    assert_finished(&out, &expected("c.bin", 0, 3_000_000));
}

#[tokio::test]
async fn file_changed_on_server() {
    let (base, dir) = (server().await, temp_dir("changed"));
    // Request 1 is the probe; from request 2 on the server has a new version.
    let req = request(
        format!("{base}/file/v.bin?size=1000000&throttle=2000000&etag_change_after=1"),
        &dir,
    );
    run_and_stop(&req, Duration::from_millis(200)).await;

    let out = run(&req).await.unwrap();
    assert!(out.restarted, "If-Range must notice the change");
    assert_finished(&out, &expected("v.bin", 1, 1_000_000));
}

#[tokio::test]
async fn expired_link() {
    let (base, dir) = (server().await, temp_dir("expired"));
    let req = request(
        format!("{base}/file/e.bin?size=1000000&throttle=2000000&html_after=1"),
        &dir,
    );
    run_and_stop(&req, Duration::from_millis(200)).await;
    assert!(matches!(run(&req).await, Err(Error::LinkExpired)));
}

#[tokio::test]
async fn server_without_ranges() {
    let (base, dir) = (server().await, temp_dir("norange"));
    let req = request(
        format!("{base}/file/n.bin?size=1000000&throttle=2000000&no_range=true"),
        &dir,
    );
    run_and_stop(&req, Duration::from_millis(200)).await;

    let out = run(&req).await.unwrap();
    assert!(
        out.restarted,
        "can't resume without ranges, must start over"
    );
    assert_finished(&out, &expected("n.bin", 0, 1_000_000));
}

#[tokio::test]
async fn unknown_size() {
    let (base, dir) = (server().await, temp_dir("chunked"));
    let out = run(&request(
        format!("{base}/file/u.bin?size=500000&chunked=true"),
        &dir,
    ))
    .await
    .unwrap();
    assert_eq!(out.size, 500_000);
    assert_finished(&out, &expected("u.bin", 0, 500_000));
}

#[tokio::test]
async fn redirects_and_unicode_name() {
    let (base, dir) = (server().await, temp_dir("redirect"));
    let url =
        format!("{base}/redirect/3/%E2%82%AC%20r%C3%A9sum%C3%A9.pdf?size=1000&attachment=true");
    let out = run(&request(url, &dir)).await.unwrap();
    assert_eq!(out.path, dir.join("€ résumé.pdf"));
    assert_finished(&out, &expected("€ résumé.pdf", 0, 1000));
}

#[tokio::test]
async fn name_clash_gets_a_number() {
    let (base, dir) = (server().await, temp_dir("clash"));
    std::fs::write(dir.join("a.bin"), b"mine").unwrap();
    let out = run(&request(format!("{base}/file/a.bin?size=10"), &dir))
        .await
        .unwrap();
    assert_eq!(out.path, dir.join("a (2).bin"));
    assert_eq!(std::fs::read(dir.join("a.bin")).unwrap(), b"mine");
}

#[tokio::test]
async fn cookies_are_replayed() {
    let (base, dir) = (server().await, temp_dir("cookie"));
    let mut req = request(format!("{base}/protected/p.bin?size=5000"), &dir);
    assert!(matches!(run(&req).await, Err(Error::Status(s)) if s == 403));

    req.headers
        .push(("Cookie".into(), testserver::SESSION_COOKIE.into()));
    let out = run(&req).await.unwrap();
    assert_finished(&out, &expected("p.bin", 0, 5000));
}

#[tokio::test]
async fn empty_file() {
    let (base, dir) = (server().await, temp_dir("empty"));
    let out = run(&request(format!("{base}/file/z.bin?size=0"), &dir))
        .await
        .unwrap();
    assert_eq!(out.size, 0);
    assert_finished(&out, b"");
}

#[tokio::test]
async fn network_errors_say_why() {
    let dir = temp_dir("dns");
    let req = request("https://no-such-host.invalid/file.bin".into(), &dir);
    let err = run(&req).await.unwrap_err().to_string();
    assert!(err.to_lowercase().contains("dns"), "{err}");
}
