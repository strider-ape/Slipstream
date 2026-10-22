# TEST PLAN (v1)

## A. Local test server scenarios (`tools/testserver`)
| # | Scenario | Expectation |
|---|---|---|
| 1 | 1 GiB file, Range supported, 1 MB/s per-connection throttle | 8 connections ≥ 7 MB/s; SHA-256 matches |
| 2 | Range ignored (always 200 full body) | Detected; single connection; correct file |
| 3 | No Content-Length (chunked) | Single stream; correct file; size unknown in UI |
| 4 | Drops the connection after random N bytes (20% of requests) | Retries with backoff; correct file |
| 5 | ETag changes mid-download | "File changed" detected; no corrupt output |
| 6 | Second request returns an HTML error page (200, text/html) | Detected as an expired link; clear error |
| 7 | One very slow connection near the end | Hedging kicks in; total time not dominated by the slow tail |
| 8 | 429 + Retry-After on >4 connections | Host cap lowered to 4 and remembered |
| 9 | `Content-Disposition: attachment; filename*=UTF-8''%E2%82%AC%20r%C3%A9sum%C3%A9.pdf` | Saved as `€ résumé.pdf` |
| 10 | Cookie-protected route (403 without the cookie) | Works when the cookie is passed from the extension |
| 11 | POST-generated file | Works with the body from the extension |
| 12 | Redirect chain (3 hops) to a ranged file | Segments use the final URL |
| 13 | Kill -9 the daemon at 50%, restart | Resumes; no re-download of completed ranges; hash OK |
| 14 | Disk full (small quota/RAM disk) | Clean error, can resume after freeing space |
| 15 | Target on FAT32 USB, 5 GiB file | Refused up front with a clear message |

## B. Real-world benchmarks (record in repo `docs/benchmarks.md`)
Same URL, same network, 3 runs each, median:
- browser download (single stream) vs **slipstream** vs `aria2c -x16 -s16`.
- Suggested URLs: a big Ubuntu ISO from a nearby mirror, a provider speed-test file (e.g. a Hetzner/OVH/Linode 1 GB test file), a GitHub release asset (redirect to a CDN).

## C. Browser catching matrix
Chrome, Edge and Firefox × Windows and Linux (WSLg):
- plain link click, Alt+click (force), Ctrl+click (prevent), context menu item;
- link behind login (cookie), POST form download, daemon stopped (fallback), incognito/private window.

## D. Resource budget (check every milestone)
- Daemon idle RAM < 20 MB; with 5 active downloads < 50 MB.
- GUI idle RAM < 40 MB; CPU ~0% when idle.
- Release binaries: aim for under 10 MB each.
