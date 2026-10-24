//! Many connections per file: dynamic segmentation (SPEC-engine §6–9).
//!
//! One supervisor per download decides *who downloads what*; each connection is a
//! worker task that streams one byte range into the shared part file. All the
//! bookkeeping (segments, connections, backoff) sits behind one short-held mutex.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, Response, StatusCode};
use tokio::task::{AbortHandle, Id, JoinSet};

use crate::download::{Error, Progress, content_range, header, send};
use crate::limiter::Limiter;
use crate::state::{Segment, State};
use crate::storage::PartFile;

const TICK: Duration = Duration::from_millis(250);

/// `SLIPSTREAM_DEBUG=1` prints the supervisor's decisions to stderr.
macro_rules! debug {
    ($($arg:tt)*) => {
        if *DEBUG.get_or_init(|| std::env::var_os("SLIPSTREAM_DEBUG").is_some()) {
            eprintln!("
[{:>7.2}s] {}", START.get_or_init(Instant::now).elapsed().as_secs_f64(), format!($($arg)*));
        }
    };
}
static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
const SAVE_EVERY: Duration = Duration::from_secs(2);
/// Connections double at most this often.
const RAMP_EVERY: Duration = Duration::from_secs(1);
/// A doubling must raise the speed by this factor, or it is undone.
const PAYOFF: f64 = 1.15;
/// Race a slow (not stalled) connection only if that should finish this much sooner, in s.
const TAIL_GAIN: f64 = 2.0;
/// Tries per segment before giving up on the download.
const MAX_TRIES: u32 = 10;
/// A connection with no data for this long gets raced by a fresh one.
const STALL: Duration = Duration::from_secs(2);
/// Don't split pieces smaller than twice this (64 KiB, or 1 MiB on fast links).
const MIN_SEG_SLOW: u64 = 64 * 1024;
const MIN_SEG_FAST: u64 = 1024 * 1024;

/// Fixed inputs of one segmented run.
pub(crate) struct Job<'a> {
    pub client: &'a Client,
    pub part: &'a Arc<PartFile>,
    pub sidecar: &'a Path,
    pub max_conn: usize,
    /// Bytes/s for the whole download, 0 = unlimited.
    pub limit: u64,
}

/// How a run ended without finishing.
pub(crate) enum End {
    /// Start the download over; `single`: with one plain connection.
    Restart {
        single: bool,
    },
    Failed(Error),
}

/// Why a worker stopped early.
enum Fail {
    /// Network trouble: try this segment again after a backoff.
    Retry(String),
    /// 429/503: the server wants fewer connections; try again after this long.
    Busy(Duration),
    /// If-Range didn't match: the file changed on the server.
    Changed,
    /// The server doesn't honour ranges after all.
    RangeIgnored,
    Fatal(Error),
}

/// Shared by the supervisor and every worker.
struct Ctx {
    client: Client,
    part: Arc<PartFile>,
    url: String,
    headers: Vec<(String, String)>,
    if_range: Option<String>,
    size: u64,
    /// The file itself is HTML, so an HTML answer isn't an error page.
    html_ok: bool,
    limiter: Option<Limiter>,
    shared: Mutex<Shared>,
}

struct Shared {
    state: State,
    conns: HashMap<Id, Conn>,
    /// Segments waiting after a failure: (tries so far, when to try again).
    backoff: HashMap<u32, (u32, Instant)>,
}

/// One live connection.
struct Conn {
    seg: u32,
    first_byte: Option<Instant>,
    last_data: Instant,
    bytes: u64,
}

impl Conn {
    fn speed(&self, now: Instant) -> f64 {
        self.first_byte.map_or(0.0, |t| {
            self.bytes as f64 / now.duration_since(t).as_secs_f64().max(0.001)
        })
    }
}

fn seg(state: &State, id: u32) -> &Segment {
    state
        .segments
        .iter()
        .find(|s| s.id == id)
        .expect("segment exists")
}

fn seg_mut(state: &mut State, id: u32) -> &mut Segment {
    state
        .segments
        .iter_mut()
        .find(|s| s.id == id)
        .expect("segment exists")
}

fn is_html(mime: Option<&str>) -> bool {
    mime.is_some_and(|m| m.starts_with("text/html"))
}

/// Download every unfinished segment of `state` with up to `job.max_conn`
/// connections. `first` is the probe's response (it covers segment 0 from byte 0).
pub(crate) async fn run(
    job: Job<'_>,
    state: &mut State,
    first: Option<Response>,
    mut cancel: Pin<&mut impl Future<Output = ()>>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), End> {
    let ctx = Arc::new(Ctx {
        client: job.client.clone(),
        part: job.part.clone(),
        url: state.url.clone(),
        headers: state.headers.clone(),
        if_range: state.validator.if_range().map(str::to_owned),
        size: state.validator.size.unwrap_or(0),
        html_ok: is_html(state.mime.as_deref()),
        limiter: (job.limit > 0).then(|| Limiter::new(job.limit)),
        shared: Mutex::new(Shared {
            state: state.clone(),
            conns: HashMap::new(),
            backoff: HashMap::new(),
        }),
    });
    let mut workers = JoinSet::new();
    let mut handles: HashMap<Id, AbortHandle> = HashMap::new();
    let mut max_conn = job.max_conn.max(1);
    let mut allowed = 1;
    debug!(
        "start: {} segments, up to {max_conn} connections",
        state.segments.len()
    );
    if let Some(res) = first {
        let id = state.segments[0].id;
        spawn(&ctx, &mut workers, &mut handles, id, Some(res));
    }

    let mut tick = tokio::time::interval(TICK);
    let (mut last_save, mut last_ramp) = (Instant::now(), Instant::now());
    // After a doubling: (connections before, speed before), until judged.
    let mut ramp_check: Option<(usize, f64)> = None;
    // (time, bytes) samples of about the last second, for an honest average.
    let mut recent: VecDeque<(Instant, u64)> = VecDeque::new();
    // Speed sampled about a second ago, and whether we ramped at least once.
    let (mut sample, mut ramped_once): (Option<(Instant, f64)>, bool) = (None, false);
    // Most connections seen receiving data at the same time.
    let mut proven = 1;
    let (mut speed, mut last_bytes, mut last_at) = (0.0, state.downloaded(), Instant::now());
    let result = loop {
        // Fill free slots: orphaned segments first, then races, then splits.
        while workers.len() < allowed {
            let min_seg = if speed > 1e6 {
                MIN_SEG_FAST
            } else {
                MIN_SEG_SLOW
            };
            let next = assign(&mut ctx.shared.lock().unwrap(), Instant::now(), min_seg);
            let Some(id) = next else { break };
            spawn(&ctx, &mut workers, &mut handles, id, None);
        }
        if ctx
            .shared
            .lock()
            .unwrap()
            .state
            .segments
            .iter()
            .all(Segment::is_complete)
        {
            break Ok(());
        }

        tokio::select! {
            () = &mut cancel => break Err(End::Failed(Error::Cancelled)),
            Some(joined) = workers.join_next_with_id() => {
                let (id, outcome) = match joined {
                    Ok((id, outcome)) => (id, outcome),
                    // Aborted by us (lost a race): nothing to do.
                    Err(e) if e.is_cancelled() => (e.id(), Ok(())),
                    Err(e) => (e.id(), Err(Fail::Fatal(Error::Io(std::io::Error::other(e))))),
                };
                handles.remove(&id);
                let mut sh = ctx.shared.lock().unwrap();
                let Some(conn) = sh.conns.remove(&id) else { continue };
                let others = sh.conns.iter().filter(|(_, c)| c.seg == conn.seg).map(|(i, _)| *i);
                let others: Vec<Id> = others.collect();
                let now = Instant::now();
                match outcome {
                    Ok(()) => {
                        // Segment finished: whoever raced on it can stop.
                        for other in others {
                            if let Some(h) = handles.get(&other) {
                                h.abort();
                            }
                        }
                    }
                    // A racer failing while the original still runs needs no retry.
                    Err(Fail::Retry(_)) if !others.is_empty() => {}
                    Err(Fail::Retry(why)) => {
                        debug!("segment {} failed: {why}", conn.seg);
                        let entry = sh.backoff.entry(conn.seg).or_insert((0, now));
                        entry.0 += 1;
                        if entry.0 > MAX_TRIES {
                            break Err(End::Failed(Error::GaveUp(why)));
                        }
                        entry.1 = now + backoff(entry.0);
                    }
                    Err(Fail::Busy(wait)) => {
                        // Too many connections for this server: stay at what is running now,
                        // but never below a count that already worked side by side (a 429
                        // can be a blip while an old connection is still closing).
                        max_conn = workers.len().max(proven).max(1);
                        debug!("server busy (429/503): cap at {max_conn} connections");
                        allowed = allowed.min(max_conn);
                        let entry = sh.backoff.entry(conn.seg).or_insert((0, now));
                        entry.1 = now + wait;
                    }
                    Err(Fail::Changed) => break Err(End::Restart { single: false }),
                    Err(Fail::RangeIgnored) => break Err(End::Restart { single: true }),
                    Err(Fail::Fatal(e)) => break Err(End::Failed(e)),
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                let (downloaded, all_receiving, receiving) = {
                    let sh = ctx.shared.lock().unwrap();
                    (
                        sh.state.downloaded(),
                        sh.conns.values().all(|c| c.first_byte.is_some()),
                        sh.conns.values().filter(|c| c.first_byte.is_some()).count(),
                    )
                };
                let dt = now.duration_since(last_at).as_secs_f64();
                if dt > 0.0 {
                    let instant = downloaded.saturating_sub(last_bytes) as f64 / dt;
                    speed = 0.7 * speed + 0.3 * instant;
                }
                (last_bytes, last_at) = (downloaded, now);
                recent.push_back((now, downloaded));
                while recent.len() > 1 && now.duration_since(recent[1].0) >= RAMP_EVERY {
                    recent.pop_front();
                }
                let (t0, b0) = recent[0];
                // Plain average over ~1 s: the smoothed `speed` lags and would make a
                // useless doubling look like a gain.
                let rate = downloaded.saturating_sub(b0) as f64
                    / now.duration_since(t0).as_secs_f64().max(0.001);

                // The first connection speeds up on its own for a while (TCP slow start);
                // only add connections once it has levelled off, or the "before" speed
                // is too low and any doubling looks like a gain.
                let mut settled = ramped_once;
                match sample {
                    Some((at, before)) if now.duration_since(at) >= RAMP_EVERY => {
                        settled |= rate < before * PAYOFF;
                        sample = Some((now, rate));
                    }
                    None => sample = Some((now, rate)),
                    _ => {}
                }
                proven = proven.max(receiving);

                // Did the last doubling pay off? Judge a second later: if speed rose by
                // less than PAYOFF, the line (or the server) is the limit, so step back
                // and stop adding connections that would only split the same bandwidth.
                if let Some((before_conns, before_speed)) = ramp_check
                    && now.duration_since(last_ramp) >= RAMP_EVERY
                {
                    ramp_check = None;
                    if rate < before_speed * PAYOFF {
                        debug!("doubling gave {:.2} -> {:.2} MB/s: back to {before_conns}", before_speed / 1e6, rate / 1e6);
                        allowed = before_conns;
                        max_conn = before_conns;
                    }
                }
                // Ramp-up: double the connections while every one is busy, the speed
                // has levelled off, and we're not yet near the speed limit.
                let under_limit = job.limit == 0 || speed < 0.85 * job.limit as f64;
                if ramp_check.is_none() && settled && allowed < max_conn && workers.len() >= allowed
                    && all_receiving && now.duration_since(last_ramp) >= RAMP_EVERY && under_limit
                {
                    debug!("ramp up {} -> {} connections at {:.2} MB/s", allowed, (allowed * 2).min(max_conn), rate / 1e6);
                    ramp_check = Some((allowed, rate));
                    allowed = (allowed * 2).min(max_conn);
                    last_ramp = now;
                    ramped_once = true;
                }
                on_progress(Progress { downloaded, size: Some(ctx.size), connections: workers.len() });
                if now.duration_since(last_save) >= SAVE_EVERY {
                    if let Err(e) = save(&ctx, job.sidecar) {
                        break Err(End::Failed(e));
                    }
                    last_save = now;
                }
            }
        }
    };
    // Stop and wait for every worker, so no file handle stays open.
    workers.shutdown().await;
    *state = ctx.shared.lock().unwrap().state.clone();
    result
}

/// Data first, then the claim that it's there.
fn save(ctx: &Ctx, sidecar: &Path) -> Result<(), Error> {
    ctx.part.sync()?;
    let state = ctx.shared.lock().unwrap().state.clone();
    Ok(state.save(sidecar)?)
}

fn spawn(
    ctx: &Arc<Ctx>,
    workers: &mut JoinSet<Result<(), Fail>>,
    handles: &mut HashMap<Id, AbortHandle>,
    seg: u32,
    first: Option<Response>,
) {
    // Hold the lock while spawning so the worker finds its entry in `conns`.
    let mut sh = ctx.shared.lock().unwrap();
    let handle = workers.spawn(worker(ctx.clone(), seg, first));
    let now = Instant::now();
    let conn = Conn {
        seg,
        first_byte: None,
        last_data: now,
        bytes: 0,
    };
    sh.conns.insert(handle.id(), conn);
    handles.insert(handle.id(), handle);
}

/// Pick work for a free connection: a segment id (two owners on one segment = a race).
fn assign(sh: &mut Shared, now: Instant, min_seg: u64) -> Option<u32> {
    let owners = |sh: &Shared, id: u32| sh.conns.values().filter(|c| c.seg == id).count();

    // 1. Unfinished segments nobody works on (resume, or retry after a failure).
    let orphan = sh.state.segments.iter().find(|s| {
        !s.is_complete()
            && owners(sh, s.id) == 0
            && sh.backoff.get(&s.id).is_none_or(|(_, at)| *at <= now)
    });
    if let Some(s) = orphan {
        return Some(s.id);
    }

    // 2. Race a connection that went silent: its bytes may never come otherwise.
    let stalled = sh.conns.values().find(|c| {
        owners(sh, c.seg) == 1
            && now.duration_since(c.last_data) >= STALL
            && !seg(&sh.state, c.seg).is_complete()
    });
    if let Some(c) = stalled {
        debug!("race stalled segment {}", c.seg);
        return Some(c.seg);
    }

    // 3. Split the biggest remaining segment; the new connection takes the upper half.
    let biggest = sh
        .state
        .segments
        .iter()
        .filter(|s| owners(sh, s.id) == 1)
        .max_by_key(|s| s.remaining().unwrap_or(0));
    if let Some(best) = biggest
        && let (Some(rem), Some(end)) = (best.remaining(), best.end)
        && rem >= 2 * min_seg + 3
    {
        let mid = end - rem / 2;
        let (best_id, new_id) = (best.id, sh.state.segments.iter().map(|s| s.id).max()? + 1);
        seg_mut(&mut sh.state, best_id).end = Some(mid - 1);
        let new = Segment {
            id: new_id,
            start: mid,
            end: Some(end),
            done: 0,
        };
        sh.state.segments.push(new);
        sh.state.segments.sort_by_key(|s| s.start);
        debug!("split segment {best_id} at {mid}: new segment {new_id}");
        return Some(new_id);
    }

    // 4. Nothing worth splitting: race a connection that is far slower than the
    //    best one and would hold up the finish (the slow tail). Racing fetches
    //    bytes twice, so only do it when it saves real time.
    let fastest = sh.conns.values().map(|c| c.speed(now)).fold(0.0, f64::max);
    let slow = sh.conns.values().find(|c| {
        let s = c.speed(now);
        let rem = seg(&sh.state, c.seg).remaining().unwrap_or(0) as f64;
        owners(sh, c.seg) == 1
            && c.first_byte
                .is_some_and(|t| now.duration_since(t) >= RAMP_EVERY)
            && s * 4.0 < fastest
            && rem / s.max(1.0) - rem / fastest >= TAIL_GAIN
    });
    let raced = slow.map(|c| c.seg);
    if let Some(id) = raced {
        debug!("race slow tail of segment {id}");
    }
    raced
}

/// 0.5 s, 1 s, 2 s, 4 s ... up to 30 s, ±20% so connections don't retry in lockstep.
fn backoff(tries: u32) -> Duration {
    let base = (0.5 * 2f64.powi(tries as i32 - 1)).min(30.0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let jitter = 0.8 + f64::from(nanos % 400) / 1000.0;
    Duration::from_secs_f64(base * jitter)
}

/// One connection: request the segment's missing bytes and write them as they come.
async fn worker(ctx: Arc<Ctx>, seg_id: u32, first: Option<Response>) -> Result<(), Fail> {
    let me = tokio::task::id();
    let (mut pos, end, url) = {
        let sh = ctx.shared.lock().unwrap();
        let s = seg(&sh.state, seg_id);
        (
            s.next_offset(),
            s.end.unwrap_or(0),
            sh.state.final_url.clone(),
        )
    };
    let res = match first {
        Some(res) => res,
        None => request(&ctx, &url, pos, end).await?,
    };
    let mut body = res.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| Fail::Retry(e.to_string()))?;
        if let Some(limiter) = &ctx.limiter {
            limiter.acquire(chunk.len()).await;
        }
        // The segment's end can shrink at any moment (split): never write past it.
        let take = {
            let sh = ctx.shared.lock().unwrap();
            let end = seg(&sh.state, seg_id).end.unwrap_or(0);
            if pos > end {
                0
            } else {
                (end + 1 - pos).min(chunk.len() as u64) as usize
            }
        };
        if take > 0 {
            ctx.part
                .write_at(pos, &chunk[..take])
                .map_err(|e| Fail::Fatal(e.into()))?;
        }
        pos += take as u64;

        let mut sh = ctx.shared.lock().unwrap();
        let now = Instant::now();
        if let Some(c) = sh.conns.get_mut(&me) {
            c.first_byte.get_or_insert(now);
            c.last_data = now;
            c.bytes += take as u64;
        }
        let s = seg_mut(&mut sh.state, seg_id);
        let len = s.end.map_or(0, |e| e + 1 - s.start);
        // Racing workers write the same bytes; progress is whoever is furthest.
        s.done = s.done.max(pos - s.start).min(len);
        if s.is_complete() {
            return Ok(());
        }
    }
    if seg(&ctx.shared.lock().unwrap().state, seg_id).is_complete() {
        Ok(())
    } else {
        Err(Fail::Retry("connection closed early".into()))
    }
}

/// `GET Range: bytes=from-end` with If-Range, and classify the answer.
async fn request(ctx: &Ctx, url: &str, from: u64, end: u64) -> Result<Response, Fail> {
    let range = format!("bytes={from}-{end}");
    let retry = |e: Error| Fail::Retry(e.to_string());
    let if_range = ctx.if_range.as_deref();
    let mut res = send(&ctx.client, url, &ctx.headers, Some(&range), if_range)
        .await
        .map_err(retry)?;
    if matches!(res.status().as_u16(), 403 | 404 | 410) && url != ctx.url {
        // Redirect targets (signed CDN links) often expire: ask the original URL again.
        res = send(&ctx.client, &ctx.url, &ctx.headers, Some(&range), if_range)
            .await
            .map_err(retry)?;
        if res.status().is_success() {
            ctx.shared.lock().unwrap().state.final_url = res.url().to_string();
        }
    }
    // Only a *successful* HTML answer is a login/expired page; error statuses
    // (e.g. 429 "too many connections") often come as HTML too.
    if res.status().is_success() && is_html(header(&res, CONTENT_TYPE)) && !ctx.html_ok {
        return Err(Fail::Fatal(Error::LinkExpired));
    }
    match res.status() {
        StatusCode::PARTIAL_CONTENT => match content_range(&res) {
            Some((start, _, Some(total))) if start == from && total == ctx.size => Ok(res),
            _ => Err(Fail::RangeIgnored),
        },
        StatusCode::OK if ctx.if_range.is_some() => Err(Fail::Changed),
        StatusCode::OK => Err(Fail::RangeIgnored),
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
            let secs = header(&res, RETRY_AFTER)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(1);
            Err(Fail::Busy(Duration::from_secs(u64::min(secs, 60))))
        }
        s if s.is_server_error() => Err(Fail::Retry(format!("server answered {s}"))),
        s => Err(Fail::Fatal(Error::Status(s))),
    }
}
