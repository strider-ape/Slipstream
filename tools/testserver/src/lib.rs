//! Local HTTP server that imitates the awkward servers a download manager meets in
//! the wild. Used by the engine's tests and by hand. See `docs/TEST-PLAN.md` §A.
//!
//! Every file is virtual (see [`content`]) and every scenario is switched on with
//! query parameters, e.g. `/file/big.bin?size=1073741824&throttle=1000000`.

pub mod content;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::time::Instant;

/// Bytes sent per write when not throttled.
const CHUNK: usize = 64 * 1024;

/// Scenario switches, all optional. Unknown parameters (like `id=` to keep two
/// tests apart) are ignored. Booleans are written `=true`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Opts {
    /// File size in bytes (default 1 MiB).
    pub size: Option<u64>,
    /// Content seed (default: derived from the file name).
    pub seed: Option<u64>,
    /// Ignore `Range` and always answer 200 with the whole file.
    pub no_range: bool,
    /// No `Content-Length` (chunked transfer); implies `no_range`.
    pub chunked: bool,
    /// Per-connection speed limit in bytes/s (0 = unlimited).
    pub throttle: u64,
    /// Percent of responses cut off after a random number of bytes.
    pub drop_pct: u64,
    /// Responses whose range starts at or after this offset...
    pub slow_from: Option<u64>,
    /// ...are throttled to this many bytes/s instead.
    pub slow_rate: u64,
    /// Only the first N such responses are slow (one bad connection), default all.
    pub slow_count: Option<u64>,
    /// Total bytes/s for all connections to this URL together (a full line).
    pub total_rate: u64,
    /// After this many requests the file "changes on the server" (new ETag and bytes).
    pub etag_change_after: u64,
    /// After this many requests, answer with an HTML error page (expired link).
    pub html_after: u64,
    /// More than this many simultaneous downloads of this URL get 429.
    pub conn_limit: u64,
    /// `Retry-After` seconds sent with 429 (default 1).
    pub retry_after: Option<u64>,
    /// Send `Content-Disposition: attachment` with an RFC 6266 `filename*`.
    pub attachment: bool,
}

impl Opts {
    fn size(&self) -> u64 {
        self.size.unwrap_or(1024 * 1024)
    }
}

/// Counters for one scenario URL, so "after N requests" switches work.
#[derive(Default)]
struct Counters {
    requests: AtomicU64,
    active: AtomicU64,
    slow: AtomicU64,
    /// Body bytes sent for this URL (tests check nothing is downloaded twice).
    sent: AtomicU64,
    /// Shared token bucket for `total_rate`: (tokens, last refill).
    line: Mutex<Option<(f64, std::time::Instant)>>,
}

impl Counters {
    /// How long to wait before `n` more bytes fit through the shared "line".
    fn take_line(&self, n: u64, rate: u64) -> Duration {
        let mut line = self.line.lock().unwrap();
        let now = std::time::Instant::now();
        let (tokens, last) = line.get_or_insert((0.0, now));
        let refill = rate as f64 * now.duration_since(*last).as_secs_f64();
        *tokens = (*tokens + refill).min(rate as f64 / 20.0);
        *last = now;
        *tokens -= n as f64;
        Duration::from_secs_f64((-*tokens).max(0.0) / rate as f64)
    }
}

#[derive(Default)]
struct Shared {
    by_url: Mutex<HashMap<String, Arc<Counters>>>,
}

impl Shared {
    fn counters(&self, key: &str) -> Arc<Counters> {
        let mut map = self.by_url.lock().unwrap();
        map.entry(key.to_owned()).or_default().clone()
    }
}

type AppState = State<Arc<Shared>>;

/// The router with every scenario route.
pub fn router() -> Router {
    router_with(Arc::default())
}

fn router_with(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(help))
        .route("/file/{name}", get(file))
        .route("/protected/{name}", get(protected))
        .route("/redirect/{hops}/{name}", get(redirect))
        .route("/generate", post(generate))
        .with_state(shared)
}

/// Cookie that `/protected/...` wants, as a logged-in browser would send it.
pub const SESSION_COOKIE: &str = "session=letmein";

/// Size of the file `POST /generate` returns unless `?size=` says otherwise.
pub const GENERATED_SIZE: u64 = 256 * 1024;

/// Serve on an already-bound listener until the process ends.
pub async fn serve(listener: TcpListener) -> std::io::Result<()> {
    axum::serve(listener, router()).await
}

/// Start the server on a random local port in the background (for tests).
pub async fn spawn() -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(serve(listener));
    Ok(addr)
}

/// Read-only view of what a spawned server sent.
#[derive(Clone)]
pub struct Stats(Arc<Shared>);

impl Stats {
    /// Body bytes sent so far, over all URLs.
    pub fn bytes_sent(&self) -> u64 {
        let map = self.0.by_url.lock().unwrap();
        map.values().map(|c| c.sent.load(Ordering::SeqCst)).sum()
    }
}

/// Like [`spawn`], plus a [`Stats`] handle.
pub async fn spawn_with_stats() -> std::io::Result<(SocketAddr, Stats)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let shared = Arc::new(Shared::default());
    let app = router_with(shared.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok((addr, Stats(shared)))
}

async fn help() -> &'static str {
    include_str!("help.txt")
}

async fn file(
    State(shared): AppState,
    Path(name): Path<String>,
    Query(opts): Query<Opts>,
    uri: Uri,
    req: HeaderMap,
) -> Response {
    let key = uri.path_and_query().map_or("", |p| p.as_str());
    serve_file(&shared.counters(key), &name, &opts, &req)
}

/// Like `/file`, but 403 unless the request carries [`SESSION_COOKIE`]
/// (a download behind a login, handed over from the browser with its cookies).
async fn protected(
    state: AppState,
    name: Path<String>,
    opts: Query<Opts>,
    uri: Uri,
    req: HeaderMap,
) -> Response {
    let logged_in = req
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .any(|c| c.trim() == SESSION_COOKIE);
    if !logged_in {
        return (StatusCode::FORBIDDEN, "login required").into_response();
    }
    file(state, name, opts, uri, req).await
}

/// `/redirect/3/x` → `/redirect/2/x` → `/redirect/1/x` → `/file/x`, keeping the query.
async fn redirect(Path((hops, name)): Path<(u32, String)>, uri: Uri) -> Response {
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let next = match hops {
        0 | 1 => format!("/file/{}{query}", pct_encode(&name)),
        n => format!("/redirect/{}/{}{query}", n - 1, pct_encode(&name)),
    };
    (StatusCode::FOUND, [(header::LOCATION, next)]).into_response()
}

/// A file that only exists as the answer to a form POST (think "export report").
/// Its bytes depend on the request body, so a wrong or missing body is caught.
async fn generate(
    State(shared): AppState,
    Query(mut opts): Query<Opts>,
    uri: Uri,
    req: HeaderMap,
    form: Bytes,
) -> Response {
    opts.seed = Some(content::fnv1a(&form));
    opts.size = Some(opts.size.unwrap_or(GENERATED_SIZE));
    opts.attachment = true;
    let key = uri.path_and_query().map_or("", |p| p.as_str());
    serve_file(&shared.counters(key), "report.bin", &opts, &req)
}

/// Decrements the active-download count when the response body is dropped.
struct Active(Arc<Counters>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn serve_file(counters: &Arc<Counters>, name: &str, opts: &Opts, req: &HeaderMap) -> Response {
    // n = 1 for the first request to this URL, 2 for the second, ...
    let n = counters.requests.fetch_add(1, Ordering::SeqCst) + 1;

    if opts.html_after > 0 && n > opts.html_after {
        return (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<!doctype html><title>Session expired</title><p>Please log in again.</p>",
        )
            .into_response();
    }

    let active = if opts.conn_limit > 0 {
        let now = counters.active.fetch_add(1, Ordering::SeqCst) + 1;
        let guard = Active(counters.clone());
        if now > opts.conn_limit {
            let wait = opts.retry_after.unwrap_or(1).to_string();
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    (header::RETRY_AFTER, wait),
                    (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
                ],
                // Real servers usually send their 429 as an HTML page.
                "<!doctype html><title>429 Too Many Requests</title>",
            )
                .into_response();
        }
        Some(guard)
    } else {
        None
    };

    let size = opts.size();
    let version = u64::from(opts.etag_change_after > 0 && n > opts.etag_change_after);
    let seed = content::file_seed(name, opts.seed, version);
    let etag = format!("\"{seed:016x}-{size:x}\"");
    let last_modified = format!(
        "{} Oct 2026 10:00:00 GMT",
        ["Wed, 21", "Thu, 22"][version as usize]
    );
    let ranges = !opts.no_range && !opts.chunked;

    // If-Range: only honour Range when the client's validator still matches.
    // A mismatch means "the file changed since you started": send it all with 200.
    let validator_ok = req
        .get(header::IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v == etag || v == last_modified);
    let range = req
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|_| ranges && validator_ok)
        .and_then(|v| parse_range(v, size));

    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ETAG, &etag)
        .header(header::LAST_MODIFIED, &last_modified);
    if ranges {
        res = res.header(header::ACCEPT_RANGES, "bytes");
    }
    if opts.attachment {
        res = res.header(header::CONTENT_DISPOSITION, content_disposition(name));
    }

    let (start, end) = match range {
        None if size == 0 => (0, 0),
        None => (0, size - 1),
        Some(Ok((start, end))) => {
            res = res
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
            (start, end)
        }
        Some(Err(())) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
            )
                .into_response();
        }
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    if !opts.chunked {
        res = res.header(header::CONTENT_LENGTH, HeaderValue::from(len));
    }

    let slow = matches!(opts.slow_from, Some(from) if start >= from)
        && opts
            .slow_count
            .is_none_or(|n| counters.slow.fetch_add(1, Ordering::SeqCst) < n);
    let rate = if slow { opts.slow_rate } else { opts.throttle };
    // Deterministic "random" per request, so a failing test fails the same way twice.
    let r = content::splitmix64(content::fnv1a(name.as_bytes()) ^ n);
    // The cut lands after at least one byte and before the last one.
    let cut_at = (len > 1 && r % 100 < opts.drop_pct).then(|| start + 1 + (r >> 8) % (len - 1));

    res.body(body(Stream {
        seed,
        pos: start,
        end: start + len,
        rate,
        cut_at,
        sent: 0,
        began: Instant::now(),
        _active: active,
        counters: counters.clone(),
        total_rate: opts.total_rate,
    }))
    .unwrap()
}

/// `attachment; filename="ascii fallback"; filename*=UTF-8''percent-encoded`
fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| match c {
            '"' | '\\' => '_',
            c if c == ' ' || c.is_ascii_graphic() => c,
            _ => '_',
        })
        .collect();
    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        pct_encode(name)
    )
}

/// Percent-encode everything except RFC 3986 "unreserved" characters.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Parse a single `bytes=` range against `size`.
/// `None`: no usable header (serve the whole file). `Some(Err)`: unsatisfiable (416).
pub fn parse_range(header: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // multi-range: like many servers, just send the whole file
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        // Suffix range `bytes=-N`: the last N bytes.
        let n: u64 = b.parse().ok()?;
        if n == 0 || size == 0 {
            return Some(Err(()));
        }
        return Some(Ok((size.saturating_sub(n), size - 1)));
    }
    let start: u64 = a.parse().ok()?;
    let end = match b {
        "" => size.saturating_sub(1),
        b => b.parse::<u64>().ok()?.min(size.saturating_sub(1)),
    };
    if start >= size || start > end {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

/// One response body being sent.
struct Stream {
    seed: u64,
    pos: u64,
    end: u64,
    /// Bytes/s, 0 = unlimited.
    rate: u64,
    /// Fault injection: abort the connection when reaching this offset.
    cut_at: Option<u64>,
    sent: u64,
    began: Instant,
    _active: Option<Active>,
    counters: Arc<Counters>,
    /// Bytes/s shared by every response of this URL, 0 = unlimited.
    total_rate: u64,
}

fn body(stream: Stream) -> Body {
    Body::from_stream(futures_util::stream::unfold(stream, |mut s| async move {
        if s.pos >= s.end {
            return None;
        }
        if s.cut_at.is_some_and(|cut| s.pos >= cut) {
            // An error from the body stream makes the server drop the TCP connection
            // mid-response, exactly like a flaky network or server.
            s.end = s.pos;
            let err = std::io::Error::other("fault injection: connection dropped");
            return Some((Err(err), s));
        }
        let mut n = (CHUNK as u64).min(s.end - s.pos);
        if let Some(cut) = s.cut_at {
            n = n.min(cut - s.pos);
        }
        if s.rate > 0 {
            // Small writes (~20 per second) keep the throttle smooth; then wait
            // until the moment these bytes are "allowed" to leave.
            n = n.min((s.rate / 20).max(1024));
            let due = s.began + Duration::from_secs_f64((s.sent + n) as f64 / s.rate as f64);
            tokio::time::sleep_until(due).await;
        }
        if s.total_rate > 0 {
            // One bucket for all connections to this URL: a full internet line.
            n = n.min((s.total_rate / 20).max(1024));
            let wait = s.counters.take_line(n, s.total_rate);
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let chunk = content::bytes(s.seed, s.pos, n as usize);
        s.counters.sent.fetch_add(n, Ordering::SeqCst);
        s.pos += n;
        s.sent += n;
        Some((Ok(chunk), s))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=500-", 1000), Some(Ok((500, 999))));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=-10", 1000), Some(Ok((990, 999))));
        assert_eq!(parse_range("bytes=-5000", 1000), Some(Ok((0, 999))));
        assert_eq!(parse_range("bytes=1000-", 1000), Some(Err(())));
        assert_eq!(parse_range("bytes=0-1,5-9", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=abc", 1000), None);
    }

    #[test]
    fn unicode_file_name_header() {
        assert_eq!(
            content_disposition("€ résumé.pdf"),
            "attachment; filename=\"_ r_sum_.pdf\"; \
             filename*=UTF-8''%E2%82%AC%20r%C3%A9sum%C3%A9.pdf"
        );
    }
}
