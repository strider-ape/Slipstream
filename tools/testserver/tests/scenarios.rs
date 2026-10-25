//! Every scenario of the test server, checked over real HTTP on a random local port.

use std::time::{Duration, Instant};

use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, COOKIE, ETAG,
    IF_RANGE, LOCATION, RANGE, RETRY_AFTER,
};
use reqwest::{Client, StatusCode, redirect};
use testserver::content;

async fn server() -> String {
    format!("http://{}", testserver::spawn().await.unwrap())
}

/// The bytes the server should send for file `name` (default seed, unchanged file).
fn expected(name: &str, size: usize) -> Vec<u8> {
    content::bytes(content::file_seed(name, None, 0), 0, size)
}

async fn get(url: &str, headers: &[(reqwest::header::HeaderName, &str)]) -> reqwest::Response {
    let mut req = Client::new().get(url);
    for (k, v) in headers {
        req = req.header(k, *v);
    }
    req.send().await.unwrap()
}

#[tokio::test]
async fn whole_file() {
    let base = server().await;
    let res = get(&format!("{base}/file/a.bin?size=300000"), &[]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[ACCEPT_RANGES], "bytes");
    assert_eq!(res.headers()[CONTENT_LENGTH], "300000");
    assert!(res.bytes().await.unwrap() == expected("a.bin", 300_000));
}

#[tokio::test]
async fn range_request() {
    let base = server().await;
    let url = format!("{base}/file/a.bin?size=300000");
    let res = get(&url, &[(RANGE, "bytes=1000-1999")]).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[CONTENT_RANGE], "bytes 1000-1999/300000");
    assert!(res.bytes().await.unwrap() == expected("a.bin", 300_000)[1000..2000]);

    let res = get(&url, &[(RANGE, "bytes=300000-")]).await;
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(res.headers()[CONTENT_RANGE], "bytes */300000");
}

#[tokio::test]
async fn if_range() {
    let base = server().await;
    let url = format!("{base}/file/a.bin?size=5000");
    let etag = get(&url, &[]).await.headers()[ETAG]
        .to_str()
        .unwrap()
        .to_owned();

    let res = get(&url, &[(RANGE, "bytes=10-"), (IF_RANGE, &etag)]).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);

    let res = get(&url, &[(RANGE, "bytes=10-"), (IF_RANGE, "\"stale\"")]).await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "stale validator must get the whole file"
    );
}

#[tokio::test]
async fn range_ignored() {
    let base = server().await;
    let res = get(
        &format!("{base}/file/a.bin?size=5000&no_range=true"),
        &[(RANGE, "bytes=10-19")],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get(ACCEPT_RANGES).is_none());
    assert!(res.bytes().await.unwrap() == expected("a.bin", 5000));
}

#[tokio::test]
async fn chunked_without_length() {
    let base = server().await;
    let res = get(&format!("{base}/file/a.bin?size=200000&chunked=true"), &[]).await;
    assert!(res.headers().get(CONTENT_LENGTH).is_none());
    assert!(res.bytes().await.unwrap() == expected("a.bin", 200_000));
}

#[tokio::test]
async fn throttled_connection() {
    let base = server().await;
    let began = Instant::now();
    // 100 kB at 200 kB/s should take about half a second.
    let res = get(
        &format!("{base}/file/a.bin?size=100000&throttle=200000"),
        &[],
    )
    .await;
    assert!(res.bytes().await.unwrap() == expected("a.bin", 100_000));
    assert!(
        began.elapsed() >= Duration::from_millis(450),
        "{:?}",
        began.elapsed()
    );
}

#[tokio::test]
async fn slow_tail() {
    let base = server().await;
    let url = format!("{base}/file/a.bin?size=200000&slow_from=100000&slow_rate=100000");

    let began = Instant::now();
    get(&url, &[(RANGE, "bytes=0-49999")])
        .await
        .bytes()
        .await
        .unwrap();
    let fast = began.elapsed();

    let began = Instant::now();
    get(&url, &[(RANGE, "bytes=100000-149999")])
        .await
        .bytes()
        .await
        .unwrap();
    let slow = began.elapsed();

    assert!(slow >= Duration::from_millis(450), "{slow:?}");
    assert!(fast < Duration::from_millis(300), "{fast:?}");
}

#[tokio::test]
async fn dropped_connection() {
    let base = server().await;
    let url = format!("{base}/file/a.bin?size=300000&drop_pct=100");
    // Depending on buffering, the cut shows up while reading headers or the body.
    match Client::new().get(url).send().await {
        Err(_) => {}
        Ok(res) => assert!(res.bytes().await.is_err(), "body must be cut off"),
    }
}

#[tokio::test]
async fn file_changes_on_server() {
    let base = server().await;
    let url = format!("{base}/file/a.bin?size=5000&etag_change_after=1");
    let first = get(&url, &[]).await.headers()[ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let second = get(&url, &[]).await;
    assert_ne!(second.headers()[ETAG], first.as_str());
    assert!(second.bytes().await.unwrap() != expected("a.bin", 5000));

    // A segment request validated against the old ETag gets the whole new file.
    let res = get(&url, &[(RANGE, "bytes=100-"), (IF_RANGE, &first)]).await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn html_error_page_later() {
    let base = server().await;
    let url = format!("{base}/file/a.bin?size=5000&html_after=1");
    let first = get(&url, &[]).await;
    assert_eq!(first.headers()[CONTENT_TYPE], "application/octet-stream");
    let second = get(&url, &[(RANGE, "bytes=100-")]).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert!(
        second.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
}

#[tokio::test]
async fn too_many_connections() {
    let base = server().await;
    let url = format!("{base}/file/a.bin?size=1000000&throttle=10000&conn_limit=1&retry_after=3");
    let first = get(&url, &[]).await; // keep its body open
    assert_eq!(first.status(), StatusCode::OK);
    let second = get(&url, &[]).await;
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.headers()[RETRY_AFTER], "3");
}

#[tokio::test]
async fn unicode_attachment_name() {
    let base = server().await;
    let url = format!("{base}/file/%E2%82%AC%20r%C3%A9sum%C3%A9.pdf?attachment=true&size=10");
    let res = get(&url, &[]).await;
    assert_eq!(
        res.headers()[CONTENT_DISPOSITION],
        "attachment; filename=\"_ r_sum_.pdf\"; filename*=UTF-8''%E2%82%AC%20r%C3%A9sum%C3%A9.pdf"
    );
}

#[tokio::test]
async fn cookie_protected() {
    let base = server().await;
    let url = format!("{base}/protected/p.bin?size=5000");
    assert_eq!(get(&url, &[]).await.status(), StatusCode::FORBIDDEN);

    let res = get(&url, &[(COOKIE, "theme=dark; session=letmein")]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.bytes().await.unwrap() == expected("p.bin", 5000));
}

#[tokio::test]
async fn post_generated_file() {
    let base = server().await;
    let form = "report=q3&format=bin";
    let res = Client::new()
        .post(format!("{base}/generate"))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("report.bin")
    );
    let want = content::bytes(
        content::fnv1a(form.as_bytes()),
        0,
        testserver::GENERATED_SIZE as usize,
    );
    assert!(res.bytes().await.unwrap() == want);
}

#[tokio::test]
async fn redirect_chain() {
    let base = server().await;
    let url = format!("{base}/redirect/3/r.bin?size=1000");

    let no_follow = Client::builder()
        .redirect(redirect::Policy::none())
        .build()
        .unwrap();
    let hop = no_follow.get(&url).send().await.unwrap();
    assert_eq!(hop.status(), StatusCode::FOUND);
    assert_eq!(hop.headers()[LOCATION], "/redirect/2/r.bin?size=1000");

    let res = get(&url, &[]).await;
    assert_eq!(res.url().path(), "/file/r.bin");
    assert!(res.bytes().await.unwrap() == expected("r.bin", 1000));
}

#[tokio::test]
async fn test_page_and_login() {
    let base = server().await;
    let page = get(&format!("{base}/page"), &[])
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("/protected/members-only.zip"), "{page}");

    let no_follow = Client::builder()
        .redirect(redirect::Policy::none())
        .build()
        .unwrap();
    let res = no_follow.get(format!("{base}/login")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers()[LOCATION], "/page");
    let cookie = res.headers()[reqwest::header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with(testserver::SESSION_COOKIE), "{cookie}");
}
