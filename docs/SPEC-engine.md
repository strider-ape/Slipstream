# SPEC: Segmented download engine (v1)

Clean-room spec written in our own words. It describes **behavior to implement**, not anyone's code. Rationale: these techniques are well known (multi-connection range downloading, work stealing, hedged requests). Popular commercial download managers get their speed from this combination.

## 1. Goals
- Saturate the user's bandwidth on servers that throttle per connection.
- Survive crashes, sleep and network drops: resume exactly where we left off.
- Never produce a corrupt file silently.
- Low CPU and RAM: async I/O, no thread per connection.

## 2. Crate layout (Cargo workspace)
```
crates/
  core/      # engine library: probe, segments, connections, limiter, storage, state
  daemon/    # background process: owns downloads, SQLite catalogue, IPC server
  cli/       # `slipstream add URL ...`, `slipstream list`, talks to daemon over IPC
  nmhost/    # native messaging host (see SPEC-browser-capture.md)
  gui/       # Slint app, software renderer (client of the daemon)
extension/   # WebExtension (plain JS, no bundler)
tools/testserver/  # local HTTP server for tests (Range, throttling, faults)
```

## 3. Networking stack
- `tokio` runtime; `reqwest` (or `hyper` directly) with **rustls** + `rustls-native-certs`. **Always verify certificates.**
- **Force HTTP/1.1 for segment connections** (`http1_only`). HTTP/2 would multiplex all ranges over one TCP connection, which defeats the purpose: per-connection throttling is per TCP stream. HTTP/2 is fine for the probe and for single-stream downloads.
- Send `Accept-Encoding: identity` on ranged requests: byte ranges of compressed bodies can't be stitched together.
- Redirects: follow (max 10) and remember the **final URL** for segment requests. Re-resolve from the original URL when the final URL expires (403/410 on resume).
- Proxy: system proxy + env vars in v1; PAC/SOCKS later.
- Headers: replay what the browser gave us (Cookie, Referer, User-Agent, Origin, Accept*, Authorization/Bearer, custom headers); browser values win. Defaults otherwise: our own `User-Agent: slipstream/<ver>` (setting to override), `Accept: */*`, `Accept-Encoding: identity`, `Accept-Language` from the OS locale, keep-alive.

## 4. Probe
```
GET url  Range: bytes=0-   (+ browser headers)
206 + Content-Range: bytes 0-N/TOTAL  → ranged = true, size = TOTAL
200 + Content-Length                  → ranged = false (single stream), size known
200 no length / chunked               → ranged = false, size unknown
```
- Keep reading the probe response into segment 0. **Never waste the first connection.**
- Store the validator: `ETag` (strong only for If-Range) or `Last-Modified`, plus the size.
- Filename: `Content-Disposition` (`filename*` RFC 5987/6266 first, then `filename`) → else the last path segment of the final URL (percent-decoded) → else `download`. Also try `?filename=`/`?file=`/`?name=` query params before the path, and fix a missing extension from the MIME type. Sanitize for Windows (reserved names CON/PRN/AUX/NUL/COM1…, `<>:"/\|?*`, trailing dots/spaces, max 254 chars) and Linux (`/`, NUL). On a clash with the disk or the list: `name (2).ext`, `name (3).ext`… Same URL already in the list → ask: re-download / open existing / add a copy.

## 5. Segment model
```
Segment { id, start, end (inclusive), done, owner: Option<ConnId>, racing_with: Option<id> }
remaining(s) = s.end + 1 - (s.start + s.done)
```
Segments are sorted, contiguous and non-overlapping (except a racing pair).

## 6. Supervisor (one async task per download)
Tick every 250 ms, or on events:
1. **Ramp-up.** If `ranged` and `allowed < max_conn` and every active connection is already receiving data and ≥1 s has passed since the last ramp and (no limiter, or speed < 85% of the limit): `allowed = min(allowed*2, max_conn)`.
   Defaults: `max_conn = 8`; the UI offers 1, 2, 4, 8, 16, 24, 32; per-host cap table with wildcards (`*.example.com → 2`).
2. **Fill slots.** While `active < allowed` and `best_split()` returns a segment: split it and start a connection for the new upper half.
3. **Connection finished** → `best_split()` again and **reuse its keep-alive socket** for the new range.
   **Hedging (racing):** keep running stats per download for "time to first byte" and "gap between reads" (mean and σ). Any connection whose current gap exceeds `min(mean + 3σ, max_gap_seen)` is *stalled*. A free connection races a stalled one on its **remaining** range, even when a split would also be possible. When nothing can be split, race any connection idle ≥ 2 s. The first to finish wins; cancel the other.
   The same stats choose `min_seg`: use the large value when the mean read gap is < 1.1 s and speed is high.
4. Persist state when it has changed and ≥2 s have passed (and always on pause/exit).
5. When all segments are complete → finalize.

### best_split()
```
candidate = argmax over active segments of eff_rem(s) = remaining(s) - owner_speed(s) * connect_latency_estimate
split only if eff_rem >= 2*min_seg (+ small constant)
min_seg: start ~ 64 KiB; 1 MiB when speed > ~1 MB/s
```
### split(s)
```
mid = s.end - remaining(s)/2       // the new connection takes the UPPER half
new = Segment{start: mid, end: s.end, done: 0}
s.end = mid - 1                    // owner must check its end on every read and stop there
request new range with  Range: bytes=mid-end  and  If-Range: <validator>
```
Race: if the owner has already written past `mid` by the time the split lands, move `new.start` forward or cancel the split.

## 7. Safety checks (each one is an automated test)
- A ranged request answered with **200**, or with a wrong `Content-Range` → the server ignores Range: drop to a single connection.
- `If-Range` mismatch (200 on a later request) → **the file changed on the server**: ask/restart from 0.
- A later response with `Content-Type: text/html` while the first was binary → error page (expired link/login): stop and report "link expired, re-capture from browser".
- Racing pair: optionally compare the overlapping bytes; on mismatch → single connection.
- Final size == expected size, otherwise error.

## 8. Retries and timeouts
- Timeouts (common baseline values): connect 60 s (grows on retries), wait for response headers 30 s, stall = no bytes for 30 s. Make them settings with these defaults.
- On error or stall: retry the same segment from `start+done`. Exponential backoff 0.5, 1, 2, 4, 8 … max 30 s, ±20% jitter, max 10 tries per segment, then drop the slot (`allowed -= 1`).
- 429/503: honour `Retry-After`, lower that host's cap and remember it (per-host cap table).
- Fatal: 401/403/404/410 after a re-resolve attempt → error state with a clear message.

## 9. Speed limiter
Token bucket per download + a global one; each read awaits tokens for its byte count. `0` = unlimited.

## 10. Storage (decision for v1)
- **One preallocated output file** `<name>.slipstream.part` in the target folder, plus a sidecar `<name>.slipstream.json` (state). Each connection writes at its own offset (`std::os::windows::fs::FileExt::seek_write` / `std::os::unix::fs::FileExt::write_at`).
- **Windows gotcha:** on NTFS, writing far past the current end forces zero-filling up to that offset (slow for big files). Mark the file **sparse** (`FSCTL_SET_SPARSE` via `DeviceIoControl`) before writing, then `set_len(size)`. On Linux, `set_len` gives a sparse file; optionally `fallocate`.
- FAT32: refuse files > 4 GiB up front with a clear message. Check free space first.
- Finalize: fsync → rename `.part` → final name → delete the sidecar.
- Fallback if sparse files misbehave (network shares, exFAT): one part file per segment and append at the end. Keep the `Storage` trait abstract so both work.

### Sidecar state (`<name>.slipstream.json`, atomic write = tmp + fsync + rename)
```json
{ "v":1, "url":"...", "final_url":"...", "method":"GET", "headers":[["Referer","..."]],
  "validator":{"etag":"\"abc\"","last_modified":null,"size":123456789},
  "segments":[{"id":0,"start":0,"end":61728394,"done":1048576}, ...],
  "created":"2026-10-21T10:00:00Z" }
```
Resume: load the sidecar → re-probe with `If-Range` → continue each unfinished segment from `start+done`. (Because writes are positional, `done` must be persisted. Flush it every ≤2 s; on a crash we lose at most ~2 s of progress, which is fine.)

## 11. Catalogue (daemon)
SQLite (`rusqlite`, bundled): `downloads(id, url, name, dir, size, status, added, finished, error, headers_json)`, `host_caps(host, max_conn)`, `settings(key, value)`. The sidecar is authoritative for byte progress; the DB is the list the UI shows.

Statuses: `queued → probing → downloading ⇄ paused → finalizing → complete | error`.

## 12. Progress events (engine → daemon → UI)
Every 250 ms: `{id, downloaded, size, speed_bps (EWMA), eta_s, conns_active, segments:[{start,end,done}]}`. The segment list drives the segment bar (a per-connection progress strip).

## 13. Acceptance targets for v1
- On a server throttled to 1 MB/s per connection, an 8-connection download of a 1 GiB file reaches ≥7 MB/s.
- Kill -9 mid-download → restart → resumes and the SHA-256 matches.
- Server ignoring Range → completes correctly with 1 connection.
- File changed mid-download → detected, no corrupt output.
- CPU < 5% of one core at 100 MB/s on the laptop; RAM < 50 MB for the daemon with 5 active downloads.
