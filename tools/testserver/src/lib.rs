//! Local HTTP server that imitates the awkward servers a download manager meets in
//! the wild. Used by the engine's tests and by hand. See `docs/TEST-PLAN.md` §A.
//!
//! Every file is virtual (see [`content`]) and every scenario is switched on with
//! query parameters, e.g. `/file/big.bin?size=1073741824&throttle=1000000`.

pub mod content;

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::time::Instant;

/// Bytes sent per write when not throttled.
const CHUNK: usize = 64 * 1024;

/// Scenario switches, all optional. Unknown parameters (like `id=` to keep two
/// tests apart) are ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Opts {
    /// File size in bytes (default 1 MiB).
    pub size: Option<u64>,
    /// Content seed (default: derived from the file name).
    pub seed: Option<u64>,
}

impl Opts {
    fn size(&self) -> u64 {
        self.size.unwrap_or(1024 * 1024)
    }
}

/// The router with every scenario route.
pub fn router() -> Router {
    Router::new()
        .route("/", get(help))
        .route("/file/{name}", get(file))
}

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

async fn help() -> &'static str {
    include_str!("help.txt")
}

async fn file(Path(name): Path<String>, Query(opts): Query<Opts>, req: HeaderMap) -> Response {
    serve_file(&name, &opts, &req)
}

fn serve_file(name: &str, opts: &Opts, req: &HeaderMap) -> Response {
    let size = opts.size();
    let seed = content::file_seed(name, opts.seed, 0);
    let etag = format!("\"{seed:016x}-{size:x}\"");
    let last_modified = "Wed, 21 Oct 2026 10:00:00 GMT";

    // If-Range: only honour Range when the client's validator still matches.
    // A mismatch means "the file changed since you started": send it all with 200.
    let validator_ok = req
        .get(header::IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v == etag || v == last_modified);
    let range = req
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|_| validator_ok)
        .and_then(|v| parse_range(v, size));

    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, &etag)
        .header(header::LAST_MODIFIED, last_modified);

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
    res.header(header::CONTENT_LENGTH, HeaderValue::from(len))
        .body(body(seed, start, len, 0))
        .unwrap()
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

/// Stream `len` bytes of file `seed` from `start`, at most `rate` bytes/s (0 = no limit).
fn body(seed: u64, start: u64, len: u64, rate: u64) -> Body {
    struct State {
        seed: u64,
        pos: u64,
        end: u64,
        rate: u64,
        sent: u64,
        began: Instant,
    }
    let state = State {
        seed,
        pos: start,
        end: start + len,
        rate,
        sent: 0,
        began: Instant::now(),
    };
    let stream = futures_util::stream::unfold(state, |mut s| async move {
        if s.pos >= s.end {
            return None;
        }
        let mut n = (CHUNK as u64).min(s.end - s.pos);
        if s.rate > 0 {
            // Small writes (~20 per second) keep the throttle smooth; then wait
            // until the moment these bytes are "allowed" to leave.
            n = n.min((s.rate / 20).max(1024));
            let due = s.began + Duration::from_secs_f64((s.sent + n) as f64 / s.rate as f64);
            tokio::time::sleep_until(due).await;
        }
        let chunk = content::bytes(s.seed, s.pos, n as usize);
        s.pos += n;
        s.sent += n;
        Some((Ok::<_, std::io::Error>(chunk), s))
    });
    Body::from_stream(stream)
}

#[cfg(test)]
mod tests {
    use super::parse_range;

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
}
