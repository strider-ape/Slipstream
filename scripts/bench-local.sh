#!/usr/bin/env bash
# Engine acceptance benchmark against the local test server (SPEC-engine §13).
#  1. 1 GiB, server throttles each connection to 1 MB/s: 8 connections should reach >= 7 MB/s.
#  2. 1 GiB at a 100 MB/s speed limit: CPU and peak RAM of the client.
# Linux only (uses GNU time). Usage: scripts/bench-local.sh
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q -p testserver -p slipstream-cli
PORT=${PORT:-8788}
target/release/testserver --port "$PORT" >/dev/null &
SRV=$!
OUT=$(mktemp -d)
trap 'kill $SRV 2>/dev/null; rm -rf "$OUT"' EXIT
sleep 0.5
GIB=1073741824
TIME="/usr/bin/time -f %e\t%P\t%M"

check() { # name
  local want got
  want=$(curl -s "http://127.0.0.1:$PORT/file/$1?size=$GIB" | sha256sum | cut -d' ' -f1)
  got=$(sha256sum "$OUT/$1" | cut -d' ' -f1)
  [ "$want" = "$got" ] && echo "sha256 OK" || { echo "sha256 MISMATCH"; exit 1; }
}

echo "== 1) 1 GiB, 1 MB/s per connection, 8 connections"
$TIME -o "$OUT/t1" target/release/slipstream get \
  "http://127.0.0.1:$PORT/file/a.bin?size=$GIB&throttle=1000000" -d "$OUT" -c 8
IFS=$'\t' read -r secs cpu rss < "$OUT/t1"
echo "time ${secs}s  speed $(echo "$GIB / $secs / 1000000" | bc -l | cut -c1-5) MB/s  cpu $cpu  peak RAM $((rss / 1024)) MiB"
check a.bin

echo "== 2) 1 GiB at a 100 MB/s limit"
$TIME -o "$OUT/t2" target/release/slipstream get \
  "http://127.0.0.1:$PORT/file/b.bin?size=$GIB" -d "$OUT" --limit 100M
IFS=$'\t' read -r secs cpu rss < "$OUT/t2"
echo "time ${secs}s  speed $(echo "$GIB / $secs / 1000000" | bc -l | cut -c1-5) MB/s  cpu $cpu (of one core)  peak RAM $((rss / 1024)) MiB"
check b.bin
ls -l target/release/slipstream | awk '{print "binary size:", $5 / 1048576, "MiB"}'
