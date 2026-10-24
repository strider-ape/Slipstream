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

## Real-world (TEST-PLAN §B)
Not yet recorded: browser vs Slipstream vs `aria2c -x16 -s16` on a large ISO / speed-test file.
