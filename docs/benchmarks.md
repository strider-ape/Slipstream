# Benchmarks

## Local engine benchmark (M3, 2026-10-24)
Machine: Windows 11 laptop, WSL2 Ubuntu 24.04 (16 vCPU, 3.4 GB RAM), release build,
local test server (`scripts/bench-local.sh`). Every download's SHA-256 was checked.

| Scenario | Result | Target (SPEC-engine §13) |
|---|---|---|
| 1 GiB, server caps each connection at 1 MB/s, 8 connections | **7.71 MB/s** (139 s) | ≥ 7 MB/s ✅ |
| Same, peak RAM | 23 MiB | < 50 MB ✅ |
| 1 GiB with `--limit 100M`, into RAM (`/dev/shm`) | 10.25 s (limit held exactly) | |
| Same, CPU | user 0.06 s, kernel 3.0 s (30% of a core); `curl` alone: kernel 1.07 s | < 5% ❌ in WSL, see below |
| 1 GiB throttled run, into RAM, first 60 s | **2% CPU**, 6 MiB RAM | ✅ |
| Release binary (`slipstream`) | 5.3 MiB | < 10 MB ✅ |

**About CPU.** Slipstream's own code is cheap (0.06 s of user CPU per GiB). Almost all CPU
is kernel time copying data into the file. Inside WSL2 that is expensive: plain `curl`
writing 256 MB to WSL's virtual disk spent 2.3 s in the kernel, and the same throttled
1 GiB run that used 2% CPU into RAM used 64% when writing to the virtual disk (dirty-page
writeback in a 3.4 GB VM). Eight connections writing at scattered offsets also cost the
kernel more than one sequential writer. Re-measure on bare-metal Linux and native Windows
before judging the 5% target.

## Real-world (TEST-PLAN §B), 2026-10-24
Home connection in India, WSL2, files saved to RAM (`scripts/bench-real.sh`), tools taking
turns each round, 3 rounds, median. All tools produced the same SHA-256.

**Hetzner Singapore, 100 MB** (`sin-speed.hetzner.com/100MB.bin`)

| Tool | Median | Runs (MB/s) |
|---|---|---|
| curl, 1 stream | **3.6 MB/s** | 3.6 3.6 3.7 |
| slipstream -c 8 | **3.5 MB/s** | 3.3 3.8 3.5 |
| slipstream -c 16 | **3.7 MB/s** | 3.2 3.7 4.1 |
| aria2c -x16 -s16 | **3.6 MB/s** | 3.6 3.6 3.0 |

This server allows 2 connections per client (more get `429` as an HTML page) and caps the
client's total speed, so no tool can beat one stream here. It exposed a real bug (an HTML
429 was reported as "link expired"); fixed, and the test server now sends HTML 429s.

**OVH, 125 MB** (`proof.ovh.net/files/1Gb.dat`), round 1 only: curl 9.0 MB/s,
slipstream -c 8 8.4 MB/s, -c 16 7.6 MB/s. Then aria2c's 16 connections failed and the server
stopped accepting any connection from us (a temporary block), so the run was stopped.

**Takeaways**
- On these servers one stream already gets all the server/line allows, so more connections
  can't help. The win is on servers that throttle *per connection* (locally: 7.7 vs 1 MB/s).
- When the line is the bottleneck Slipstream is 5–15% behind one stream. Likely causes:
  racing connections only for being slower than others (duplicate bytes on a full line), and
  ramping up even when more connections add no speed. To fix next.
- Aggressive connection counts can get a client blocked; keep 8 as the default.
