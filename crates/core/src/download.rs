//! Single-connection download with resume (milestone M2). M3 adds more connections
//! on top of the same probe, state and storage.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
    ETAG, IF_RANGE, LAST_MODIFIED, RANGE,
};
use reqwest::{Client, Response, StatusCode, Url};

use crate::filename;
use crate::segmented::{self, End, Job};
use crate::state::{Segment, State, Validator};
use crate::storage::{PartFile, part_path, sidecar_path};

/// How often progress is reported.
const TICK: Duration = Duration::from_millis(250);
/// How often progress is saved to the sidecar (a crash loses at most this much).
const SAVE_EVERY: Duration = Duration::from_secs(2);

/// What to download and where.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub url: String,
    /// Headers to replay, e.g. from the browser (Cookie, Referer, User-Agent...).
    pub headers: Vec<(String, String)>,
    /// Folder to save into.
    pub dir: PathBuf,
    /// File name to use instead of the server's.
    pub name: Option<String>,
    /// Most connections to use (0 = default, 8).
    pub connections: usize,
    /// Speed limit in bytes/s (0 = unlimited).
    pub speed_limit: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub downloaded: u64,
    pub size: Option<u64>,
    /// Connections currently downloading.
    pub connections: usize,
}

#[derive(Debug)]
pub struct Outcome {
    pub path: PathBuf,
    pub size: u64,
    /// Offset we continued from (0 = fresh start).
    pub resumed_from: u64,
    /// True when an earlier partial download had to start over (file changed on the
    /// server, or the server can't resume).
    pub restarted: bool,
}

#[derive(Debug)]
pub enum Error {
    InvalidUrl(String),
    Http(reqwest::Error),
    Io(io::Error),
    /// The server answered with an error status.
    Status(StatusCode),
    /// An HTML page came back instead of the file: expired link or login needed.
    LinkExpired,
    /// The server answered a range request with different bytes than asked.
    BadRange(String),
    /// The connection ended before the whole file arrived (resume to continue).
    Incomplete {
        got: u64,
        expected: u64,
    },
    /// The finished file doesn't have the size the server announced.
    SizeMismatch {
        got: u64,
        expected: u64,
    },
    /// Stopped by the caller; progress is saved.
    Cancelled,
    /// A part kept failing after every retry.
    GaveUp(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidUrl(u) => write!(f, "not a valid http(s) URL: {u}"),
            Error::Http(e) => write!(f, "network error: {e}"),
            Error::Io(e) => write!(f, "disk error: {e}"),
            Error::Status(s) => write!(f, "server answered {s}"),
            Error::LinkExpired => write!(
                f,
                "the server sent a web page instead of the file (link expired or login \
                 needed); capture the download again from the browser"
            ),
            Error::BadRange(why) => write!(f, "server sent the wrong part of the file: {why}"),
            Error::Incomplete { got, expected } => {
                write!(
                    f,
                    "connection ended at {got} of {expected} bytes; run again to resume"
                )
            }
            Error::SizeMismatch { got, expected } => {
                write!(
                    f,
                    "finished file has {got} bytes but the server announced {expected}"
                )
            }
            Error::Cancelled => write!(f, "stopped; run again to resume"),
            Error::GaveUp(why) => write!(f, "gave up after repeated failures: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Download `req`, resuming a matching partial download in `req.dir` if there is one.
/// `cancel` stops the download cleanly (progress saved) when it completes.
pub async fn download(
    req: &Request,
    cancel: impl Future<Output = ()>,
    mut on_progress: impl FnMut(Progress),
) -> Result<Outcome, Error> {
    let url = Url::parse(&req.url).map_err(|_| Error::InvalidUrl(req.url.clone()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidUrl(req.url.clone()));
    }
    let client = client()?;
    std::fs::create_dir_all(&req.dir)?;

    let (target, mut state, mut first, resumed_from, mut restarted) =
        match find_resumable(&req.dir, &req.url) {
            // Ranges work: the workers continue every unfinished segment, each
            // request guarded by If-Range.
            Some((target, old)) if segmentable(&old) => {
                let from = old.downloaded();
                (target, old, None, from, false)
            }
            Some((target, old)) => {
                let r = resume(&client, old).await?;
                (target, r.state, r.res, r.from, r.restarted)
            }
            None => {
                let (state, res) = probe(&client, &req.url, &req.headers).await?;
                let name = match &req.name {
                    Some(n) => filename::sanitize(n),
                    None => filename::choose(
                        header(&res, CONTENT_DISPOSITION),
                        res.url(),
                        state.mime.as_deref(),
                    ),
                };
                let target = filename::unique(&req.dir, &name, taken);
                (target, state, Some(res), 0, false)
            }
        };

    let part_file = part_path(&target);
    let sidecar = sidecar_path(&target);
    tokio::pin!(cancel);
    let mut restarts = 0;
    let part = loop {
        if restarted {
            // Old bytes belong to another version of the file (or can't be resumed).
            let _ = std::fs::remove_file(&part_file);
        }
        let part = Arc::new(PartFile::open(&part_file, state.validator.size)?);
        state.save(&sidecar)?;

        if !segmentable(&state) {
            // One plain stream: unknown size, or a server without ranges.
            if let Some(res) = first.take()
                && !state.segments.is_empty()
            {
                let received = receive(
                    res,
                    &part,
                    &mut state,
                    &sidecar,
                    cancel.as_mut(),
                    &mut on_progress,
                )
                .await;
                // Whatever happened, keep what we have so the next run can resume.
                part.sync()?;
                state.save(&sidecar)?;
                received?;
            }
            break part;
        }

        let job = Job {
            client: &client,
            part: &part,
            sidecar: &sidecar,
            max_conn: if req.connections == 0 {
                8
            } else {
                req.connections
            },
            limit: req.speed_limit,
        };
        let run = segmented::run(
            job,
            &mut state,
            first.take(),
            cancel.as_mut(),
            &mut on_progress,
        )
        .await;
        part.sync()?;
        state.save(&sidecar)?;
        match run {
            Ok(()) => break part,
            Err(End::Failed(e)) => return Err(e),
            Err(End::Restart { .. }) if restarts >= 3 => {
                return Err(Error::GaveUp(
                    "the file keeps changing on the server".into(),
                ));
            }
            Err(End::Restart { single }) => {
                // The file changed (or ranges stopped working): start over.
                restarts += 1;
                restarted = true;
                drop(part);
                let (mut fresh, res) = probe(&client, &state.url, &state.headers).await?;
                fresh.ranged &= !single;
                state = fresh;
                first = Some(res);
            }
        }
    };

    if let Some(expected) = state.validator.size
        && state.segments.iter().any(|s| !s.is_complete())
    {
        return Err(Error::Incomplete {
            got: state.downloaded(),
            expected,
        });
    }
    on_progress(Progress {
        downloaded: state.downloaded(),
        size: state.validator.size,
        connections: 0,
    });

    // Finalize: everything is on disk → check the size, rename, drop the sidecar.
    part.sync()?;
    drop(part);
    let size = std::fs::metadata(&part_file)?.len();
    if let Some(expected) = state.validator.size
        && size != expected
    {
        return Err(Error::SizeMismatch {
            got: size,
            expected,
        });
    }
    let path = if target.exists() {
        // Someone created a file with our name meanwhile; don't overwrite it.
        let name = target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        filename::unique(&req.dir, &name, taken)
    } else {
        target
    };
    std::fs::rename(&part_file, &path)?;
    let _ = std::fs::remove_file(&sidecar);
    Ok(Outcome {
        path,
        size,
        resumed_from,
        restarted,
    })
}

/// Can many connections work on this download? (ranges + known size)
fn segmentable(state: &State) -> bool {
    state.ranged && state.validator.size.is_some() && !state.segments.is_empty()
}

/// A name is taken if the file, its part file or its sidecar exists.
fn taken(p: &Path) -> bool {
    p.exists() || part_path(p).exists() || sidecar_path(p).exists()
}

fn client() -> Result<Client, Error> {
    Ok(Client::builder()
        .user_agent(crate::default_user_agent())
        .connect_timeout(Duration::from_secs(60))
        // No bytes for this long = stalled connection.
        .read_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?)
}

/// A partial download of `url` in `dir`: (final path, saved state).
fn find_resumable(dir: &Path, url: &str) -> Option<(PathBuf, State)> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let target_name = file_name.strip_suffix(".slipstream.json")?;
        let state = State::load(&entry.path()).ok()?;
        (state.url == url).then(|| (dir.join(target_name), state))
    })
}

/// Send a GET with the replayed headers plus our defaults.
pub(crate) async fn send(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
    range: Option<&str>,
    if_range: Option<&str>,
) -> Result<Response, Error> {
    let mut rb = client.get(url);
    for (k, v) in headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
    {
        rb = rb.header(ACCEPT, "*/*");
    }
    // Byte ranges of a compressed body can't be stitched together: ask for raw bytes.
    rb = rb.header(ACCEPT_ENCODING, "identity");
    if let Some(r) = range {
        rb = rb.header(RANGE, r);
    }
    if let Some(v) = if_range {
        rb = rb.header(IF_RANGE, v);
    }
    Ok(rb.send().await?)
}

/// First request: `Range: bytes=0-`. A 206 tells us the server can resume and split;
/// either way the body is the start of the file, so the connection isn't wasted.
async fn probe(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<(State, Response), Error> {
    let res = send(client, url, headers, Some("bytes=0-"), None).await?;
    let state = fresh_state(url, headers, &res)?;
    Ok((state, res))
}

/// Build a brand-new state from a response that starts at byte 0.
fn fresh_state(url: &str, headers: &[(String, String)], res: &Response) -> Result<State, Error> {
    let (ranged, size) = match res.status() {
        StatusCode::PARTIAL_CONTENT => match content_range(res) {
            Some((0, _, total)) => (total.is_some(), total),
            _ => return Err(Error::BadRange("probe answer doesn't start at 0".into())),
        },
        StatusCode::OK => (
            false,
            header(res, CONTENT_LENGTH).and_then(|v| v.parse().ok()),
        ),
        // `bytes=0-` of an empty file can't be satisfied: there is no byte 0.
        StatusCode::RANGE_NOT_SATISFIABLE
            if header(res, CONTENT_RANGE).is_some_and(|v| v.trim() == "bytes */0") =>
        {
            (false, Some(0))
        }
        s => return Err(Error::Status(s)),
    };
    let mut state = State::new(url, headers.to_vec());
    state.final_url = res.url().to_string();
    state.ranged = ranged;
    state.mime = header(res, CONTENT_TYPE).map(str::to_owned);
    state.validator = Validator {
        etag: header(res, ETAG).map(str::to_owned),
        last_modified: header(res, LAST_MODIFIED).map(str::to_owned),
        size,
    };
    if size != Some(0) {
        state.segments.push(Segment {
            id: 0,
            start: 0,
            end: size.map(|s| s - 1),
            done: 0,
        });
    }
    Ok(state)
}

struct Resumed {
    state: State,
    /// `None` when nothing is left to fetch.
    res: Option<Response>,
    from: u64,
    restarted: bool,
}

/// Continue a saved download. Asks for the missing range with `If-Range`, so a
/// changed file comes back whole (200) instead of being glued onto the old bytes.
async fn resume(client: &Client, mut state: State) -> Result<Resumed, Error> {
    let Some(seg) = state.segments.first().cloned() else {
        return Ok(Resumed {
            state,
            res: None,
            from: 0,
            restarted: false,
        });
    };
    let (Some(end), true) = (seg.end, state.ranged) else {
        // The server can't do ranges (or the size is unknown): start again.
        let (state, res) = probe(client, &state.url, &state.headers).await?;
        return Ok(Resumed {
            state,
            res: Some(res),
            from: 0,
            restarted: true,
        });
    };
    let from = seg.next_offset();
    if from > end {
        return Ok(Resumed {
            state,
            res: None,
            from,
            restarted: false,
        });
    }

    let range = format!("bytes={from}-{end}");
    let if_range = state.validator.if_range().map(str::to_owned);
    let mut res = send(
        client,
        &state.final_url,
        &state.headers,
        Some(&range),
        if_range.as_deref(),
    )
    .await?;
    if matches!(res.status().as_u16(), 403 | 404 | 410) && state.final_url != state.url {
        // Redirect targets (signed CDN links) often expire: ask the original URL again.
        res = send(
            client,
            &state.url,
            &state.headers,
            Some(&range),
            if_range.as_deref(),
        )
        .await?;
        state.final_url = res.url().to_string();
    }

    let is_html = |mime: Option<&str>| mime.is_some_and(|m| m.starts_with("text/html"));
    if is_html(header(&res, CONTENT_TYPE)) && !is_html(state.mime.as_deref()) {
        return Err(Error::LinkExpired);
    }
    match res.status() {
        StatusCode::PARTIAL_CONTENT => match content_range(&res) {
            Some((start, _, total)) if start == from && total == state.validator.size => {
                Ok(Resumed {
                    state,
                    res: Some(res),
                    from,
                    restarted: false,
                })
            }
            other => Err(Error::BadRange(format!("asked for {range}, got {other:?}"))),
        },
        StatusCode::OK => {
            // If-Range didn't match: the file changed on the server. Start over
            // with the full body it just sent.
            let state = fresh_state(&state.url, &state.headers, &res)?;
            Ok(Resumed {
                state,
                res: Some(res),
                from: 0,
                restarted: true,
            })
        }
        s => Err(Error::Status(s)),
    }
}

/// Write the response body into the part file at the segment's offset.
async fn receive(
    res: Response,
    part: &PartFile,
    state: &mut State,
    sidecar: &Path,
    mut cancel: std::pin::Pin<&mut impl Future<Output = ()>>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), Error> {
    let mut body = res.bytes_stream();
    let mut last_save = Instant::now();
    let mut last_tick = Instant::now();
    loop {
        let chunk = tokio::select! {
            chunk = body.next() => chunk,
            () = &mut cancel => return Err(Error::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk?;

        let seg = &mut state.segments[0];
        // Never write past the segment's end, even if the server sends more.
        let take = seg
            .remaining()
            .map_or(chunk.len(), |r| r.min(chunk.len() as u64) as usize);
        part.write_at(seg.next_offset(), &chunk[..take])?;
        seg.done += take as u64;
        let finished = take < chunk.len() || seg.is_complete();

        if last_tick.elapsed() >= TICK {
            on_progress(Progress {
                downloaded: state.downloaded(),
                size: state.validator.size,
                connections: 1,
            });
            last_tick = Instant::now();
        }
        if last_save.elapsed() >= SAVE_EVERY {
            // Data first, then the claim that it's there.
            part.sync()?;
            state.save(sidecar)?;
            last_save = Instant::now();
        }
        if finished {
            break;
        }
    }
    if state.validator.size.is_none() {
        // Unknown size: the server closing the stream normally is the end.
        state.validator.size = Some(state.downloaded());
        state.segments[0].end = Some(state.downloaded().saturating_sub(1));
    }
    Ok(())
}

pub(crate) fn header(res: &Response, name: reqwest::header::HeaderName) -> Option<&str> {
    res.headers().get(name)?.to_str().ok()
}

/// `Content-Range: bytes 0-99/1000` → (0, 99, Some(1000)); `/*` total → None.
pub(crate) fn content_range(res: &Response) -> Option<(u64, u64, Option<u64>)> {
    parse_content_range(header(res, CONTENT_RANGE)?)
}

fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let (range, total) = v.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let total = match total {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    Some((start.parse().ok()?, end.parse().ok()?, total))
}

#[cfg(test)]
mod tests {
    use super::parse_content_range;

    #[test]
    fn content_range_header() {
        assert_eq!(
            parse_content_range("bytes 0-99/1000"),
            Some((0, 99, Some(1000)))
        );
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, 9, None)));
        assert_eq!(parse_content_range("bytes */1000"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }
}
