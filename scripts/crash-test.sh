#!/usr/bin/env bash
# Real crash test (Linux/WSL): start a throttled download, kill -9 the CLI mid-way,
# run the same command again, and compare SHA-256 with the server's file.
# Usage: scripts/crash-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."
echo "== build (release)"
cargo build --release -q -p testserver -p slipstream-cli
SERVER=target/release/testserver
CLI=target/release/slipstream
PORT=${PORT:-8787}

"$SERVER" --port "$PORT" &
SRV=$!
OUT=$(mktemp -d)
trap 'kill $SRV 2>/dev/null; rm -rf "$OUT"' EXIT
sleep 0.5

# 100 MiB, server limited to 10 MB/s per connection, so it takes ~10 s.
URL="http://127.0.0.1:$PORT/file/big.bin?size=104857600&throttle=10000000"

echo "== download, then kill -9 after 4 s"
"$CLI" get "$URL" -d "$OUT" &
DL=$!
sleep 4
kill -9 $DL
wait $DL 2>/dev/null || true
echo
echo "-- left on disk after the crash:"
ls -ls "$OUT"
grep -E '"(start|end|done)"' "$OUT"/big.bin.slipstream.json

echo "== resume (same command)"
"$CLI" get "$URL" -d "$OUT"

echo "== compare"
WANT=$(curl -s "http://127.0.0.1:$PORT/file/big.bin?size=104857600" | sha256sum | cut -d' ' -f1)
GOT=$(sha256sum "$OUT/big.bin" | cut -d' ' -f1)
echo "server:     $WANT"
echo "downloaded: $GOT"
if [ "$WANT" = "$GOT" ]; then echo "== PASS: SHA-256 matches after kill -9 + resume"; else echo "== FAIL"; exit 1; fi
