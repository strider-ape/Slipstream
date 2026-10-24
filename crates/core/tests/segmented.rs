//! Many connections per file against the local test server (SPEC-engine §13, TEST-PLAN §A).

use std::future::pending;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use slipstream_core::download::{Error, Outcome, Request, download};
use testserver::content;

async fn server() -> String {
    format!("http://{}", testserver::spawn().await.unwrap())
}

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("slipstream-seg-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn request(url: String, dir: &Path) -> Request {
    Request {
        url,
        dir: dir.to_owned(),
        ..Default::default()
    }
}

/// Run to the end; also return the most connections seen at once.
async fn run(req: &Request) -> (Result<Outcome, Error>, usize) {
    let mut peak = 0;
    let out = download(req, pending(), |p| peak = peak.max(p.connections)).await;
    (out, peak)
}

fn assert_content(out: &Outcome, name: &str, version: u64, size: usize) {
    let want = content::bytes(content::file_seed(name, None, version), 0, size);
    assert!(std::fs::read(&out.path).unwrap() == want, "content differs");
}

#[tokio::test(flavor = "multi_thread")]
async fn eight_connections_beat_a_per_connection_throttle() {
    // 1 MB/s per connection: one connection would need 24 s.
    let (base, dir) = (server().await, temp_dir("speed"));
    let req = request(
        format!("{base}/file/s.bin?size=24000000&throttle=1000000"),
        &dir,
    );
    let began = Instant::now();
    let (out, peak) = run(&req).await;
    let out = out.unwrap();
    let took = began.elapsed();
    assert_eq!(peak, 8, "should ramp up to 8 connections");
    assert!(took < Duration::from_secs(9), "took {took:?}");
    assert_content(&out, "s.bin", 0, 24_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_and_resume_many_segments() {
    let (base, dir) = (server().await, temp_dir("crash"));
    let req = request(
        format!("{base}/file/c.bin?size=16000000&throttle=1000000"),
        &dir,
    );
    let crashed = tokio::time::timeout(Duration::from_millis(3500), run(&req)).await;
    assert!(
        crashed.is_err(),
        "should still be running when it 'crashes'"
    );

    let (out, _) = run(&req).await;
    let out = out.unwrap();
    assert!(out.resumed_from > 0 && !out.restarted);
    assert_content(&out, "c.bin", 0, 16_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn survives_dropped_connections() {
    let (base, dir) = (server().await, temp_dir("drops"));
    let req = request(
        format!("{base}/file/d.bin?size=8000000&throttle=2000000&drop_pct=20"),
        &dir,
    );
    let (out, _) = run(&req).await;
    assert_content(&out.unwrap(), "d.bin", 0, 8_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_changed_mid_download_restarts_cleanly() {
    let (base, dir) = (server().await, temp_dir("changed"));
    let req = request(
        format!("{base}/file/v.bin?size=8000000&throttle=2000000&etag_change_after=2"),
        &dir,
    );
    let (out, _) = run(&req).await;
    let out = out.unwrap();
    assert!(out.restarted);
    assert_content(&out, "v.bin", 1, 8_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn error_page_mid_download_is_reported() {
    let (base, dir) = (server().await, temp_dir("html"));
    let req = request(
        format!("{base}/file/h.bin?size=8000000&throttle=2000000&html_after=1"),
        &dir,
    );
    assert!(matches!(run(&req).await.0, Err(Error::LinkExpired)));
}

#[tokio::test(flavor = "multi_thread")]
async fn too_many_connections_lowers_the_cap() {
    let (base, dir) = (server().await, temp_dir("429"));
    let req = request(
        format!("{base}/file/t.bin?size=12000000&throttle=1000000&conn_limit=4&retry_after=1"),
        &dir,
    );
    let (out, _) = run(&req).await;
    assert_content(&out.unwrap(), "t.bin", 0, 12_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_tail_is_raced() {
    // The first request into the last 2 MB crawls at 50 kB/s (40 s alone);
    // a racing connection gets it at full speed.
    let (base, dir) = (server().await, temp_dir("tail"));
    let req = request(
        format!(
            "{base}/file/w.bin?size=8000000&throttle=4000000\
             &slow_from=6000000&slow_rate=50000&slow_count=1"
        ),
        &dir,
    );
    let began = Instant::now();
    let (out, _) = run(&req).await;
    let took = began.elapsed();
    assert_content(&out.unwrap(), "w.bin", 0, 8_000_000);
    assert!(took < Duration::from_secs(10), "took {took:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn speed_limit_is_respected() {
    let (base, dir) = (server().await, temp_dir("limit"));
    let mut req = request(format!("{base}/file/l.bin?size=4000000"), &dir);
    req.speed_limit = 2_000_000;
    let began = Instant::now();
    let (out, _) = run(&req).await;
    let took = began.elapsed();
    assert_content(&out.unwrap(), "l.bin", 0, 4_000_000);
    assert!(took >= Duration::from_millis(1700), "too fast: {took:?}");
    assert!(took < Duration::from_secs(4), "too slow: {took:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_connection_when_asked() {
    let (base, dir) = (server().await, temp_dir("one"));
    let mut req = request(
        format!("{base}/file/o.bin?size=3000000&throttle=2000000"),
        &dir,
    );
    req.connections = 1;
    let (out, peak) = run(&req).await;
    assert_eq!(peak, 1);
    assert_content(&out.unwrap(), "o.bin", 0, 3_000_000);
}
