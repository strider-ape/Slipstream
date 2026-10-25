//! The daemon end to end: real IPC, real downloads from the local test server.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use slipstream_core::ipc::{AddRequest, Client, DownloadInfo, Paths, Request, Response, Status};
use testserver::content;
use tokio::task::JoinHandle;

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ssd-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Start a daemon living in `home` and wait until it answers.
async fn start(home: &Path) -> (Paths, JoinHandle<io::Result<()>>) {
    let paths = Paths::new(Some(home)).unwrap();
    let p = paths.clone();
    let task = tokio::spawn(async move { slipstream_daemon::run(&p).await });
    for _ in 0..200 {
        if let Ok(mut c) = Client::connect(&paths).await
            && matches!(c.call(&Request::Ping).await, Ok(Response::Pong { .. }))
        {
            return (paths, task);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon did not start");
}

async fn add(c: &mut Client, url: String, dir: &Path, connections: usize) -> i64 {
    let req = AddRequest {
        url,
        dir: Some(dir.to_owned()),
        connections,
        ..Default::default()
    };
    match c.call(&Request::Add(req)).await.unwrap() {
        Response::Added { id } => id,
        other => panic!("add failed: {other:?}"),
    }
}

async fn row(c: &mut Client, id: i64) -> Option<DownloadInfo> {
    match c.call(&Request::List).await.unwrap() {
        Response::Downloads { downloads } => downloads.into_iter().find(|d| d.id == id),
        other => panic!("list failed: {other:?}"),
    }
}

/// Poll the list until `id` matches `done` (30 s at most).
async fn wait_for(c: &mut Client, id: i64, done: impl Fn(&DownloadInfo) -> bool) -> DownloadInfo {
    for _ in 0..300 {
        if let Some(d) = row(c, id).await
            && done(&d)
        {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "#{id} never reached the expected state: {:?}",
        row(c, id).await
    );
}

fn assert_file(dir: &Path, name: &str, size: usize) {
    let want = content::bytes(content::file_seed(name, None, 0), 0, size);
    assert!(
        std::fs::read(dir.join(name)).unwrap() == want,
        "{name}: content differs"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_add_and_watch() {
    let (home, out) = (temp_dir("two-home"), temp_dir("two-out"));
    let base = format!("http://{}", testserver::spawn().await.unwrap());
    let (paths, _daemon) = start(&home).await;

    let mut a = Client::connect(&paths).await.unwrap();
    let id = add(
        &mut a,
        format!("{base}/file/w.bin?size=2000000&throttle=4000000"),
        &out,
        0,
    )
    .await;

    // A second client watches the same download finish.
    let mut b = Client::connect(&paths).await.unwrap();
    let mut msg = b.call(&Request::Watch).await.unwrap();
    let mut saw_progress = false;
    loop {
        let Response::Downloads { downloads } = msg else {
            panic!("{msg:?}")
        };
        let d = downloads.iter().find(|d| d.id == id).unwrap();
        saw_progress |= d.status == Status::Downloading && d.downloaded > 0;
        if d.status == Status::Complete {
            assert_eq!(
                (d.name.as_deref(), d.size),
                (Some("w.bin"), Some(2_000_000))
            );
            break;
        }
        msg = tokio::time::timeout(Duration::from_secs(20), b.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    assert!(saw_progress, "watch should show progress before completion");
    assert_file(&out, "w.bin", 2_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_resume() {
    let (home, out) = (temp_dir("pause-home"), temp_dir("pause-out"));
    let base = format!("http://{}", testserver::spawn().await.unwrap());
    let (paths, _daemon) = start(&home).await;
    let mut c = Client::connect(&paths).await.unwrap();
    let id = add(
        &mut c,
        format!("{base}/file/p.bin?size=3000000&throttle=1500000"),
        &out,
        1,
    )
    .await;

    wait_for(&mut c, id, |d| d.downloaded > 0).await;
    assert_eq!(c.call(&Request::Pause { id }).await.unwrap(), Response::Ok);
    let paused = row(&mut c, id).await.unwrap();
    assert_eq!(paused.status, Status::Paused);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        row(&mut c, id).await.unwrap().downloaded,
        paused.downloaded,
        "still downloading"
    );

    assert_eq!(c.call(&Request::Resume { id }).await.unwrap(), Response::Ok);
    wait_for(&mut c, id, |d| d.status == Status::Complete).await;
    assert_file(&out, "p.bin", 3_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_resumes_unfinished() {
    let (home, out) = (temp_dir("restart-home"), temp_dir("restart-out"));
    let base = format!("http://{}", testserver::spawn().await.unwrap());
    let (paths, daemon) = start(&home).await;
    let mut c = Client::connect(&paths).await.unwrap();
    let id = add(
        &mut c,
        format!("{base}/file/r.bin?size=4000000&throttle=1000000"),
        &out,
        1,
    )
    .await;
    wait_for(&mut c, id, |d| d.downloaded > 0).await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    assert_eq!(c.call(&Request::Shutdown).await.unwrap(), Response::Ok);
    daemon.await.unwrap().unwrap();
    assert!(out.join("r.bin.slipstream.part").exists());

    // A new daemon picks it up on its own and finishes it.
    let (paths, _daemon) = start(&home).await;
    let mut c = Client::connect(&paths).await.unwrap();
    let before = row(&mut c, id).await.unwrap();
    assert!(before.downloaded > 0, "progress should survive the restart");
    wait_for(&mut c, id, |d| d.status == Status::Complete).await;
    assert_file(&out, "r.bin", 4_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_one_daemon_per_home() {
    let home = temp_dir("single");
    let (paths, _daemon) = start(&home).await;
    let second = slipstream_daemon::run(&paths).await;
    assert_eq!(
        second.err().map(|e| e.kind()),
        Some(io::ErrorKind::AddrInUse)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_can_delete_the_file() {
    let (home, out) = (temp_dir("remove-home"), temp_dir("remove-out"));
    let base = format!("http://{}", testserver::spawn().await.unwrap());
    let (paths, _daemon) = start(&home).await;
    let mut c = Client::connect(&paths).await.unwrap();
    let id = add(
        &mut c,
        format!("{base}/file/x.bin?size=3000000&throttle=1000000"),
        &out,
        1,
    )
    .await;
    wait_for(&mut c, id, |d| d.name.is_some() && d.downloaded > 0).await;

    let req = Request::Remove {
        id,
        delete_file: true,
    };
    assert_eq!(c.call(&req).await.unwrap(), Response::Ok);
    assert!(row(&mut c, id).await.is_none());
    assert!(!out.join("x.bin.slipstream.part").exists());
    assert!(!out.join("x.bin.slipstream.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_errors() {
    let home = temp_dir("errors");
    let (paths, _daemon) = start(&home).await;
    let mut c = Client::connect(&paths).await.unwrap();
    let bad = Request::Add(AddRequest {
        url: "ftp://x/y".into(),
        ..Default::default()
    });
    assert!(matches!(
        c.call(&bad).await.unwrap(),
        Response::Error { .. }
    ));
    let pause = c.call(&Request::Pause { id: 99 }).await.unwrap();
    assert_eq!(
        pause,
        Response::Error {
            message: "no download #99".into()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_apply_to_new_downloads() {
    let (home, out) = (temp_dir("settings-home"), temp_dir("settings-out"));
    let base = format!("http://{}", testserver::spawn().await.unwrap());
    let (paths, _daemon) = start(&home).await;
    let mut c = Client::connect(&paths).await.unwrap();
    let Response::Settings(mut s) = c.call(&Request::GetSettings).await.unwrap() else {
        panic!("no settings")
    };
    assert_eq!(
        (s.max_connections, s.speed_limit, s.catch_enabled),
        (8, 0, true)
    );
    s.download_dir = out.clone();
    s.catch_enabled = false;
    assert_eq!(
        c.call(&Request::SetSettings(s)).await.unwrap(),
        Response::Ok
    );

    // No folder in the request: the settings' folder is used.
    let req = AddRequest {
        url: format!("{base}/file/s.bin?size=1000"),
        ..Default::default()
    };
    let Response::Added { id } = c.call(&Request::Add(req)).await.unwrap() else {
        panic!()
    };
    wait_for(&mut c, id, |d| d.status == Status::Complete).await;
    assert_file(&out, "s.bin", 1000);

    // Catching off reaches the browser rules.
    let Response::CaptureSettings(rules) = c.call(&Request::CaptureSettings).await.unwrap() else {
        panic!()
    };
    assert!(!rules.enabled);
}
