#!/usr/bin/env bash
# M4 demo (Linux/WSL): the CLI as a daemon client, against the local test server.
# add auto-starts the daemon, other CLI calls see the same list, pause/resume,
# stop the daemon mid-download, start it again: the unfinished download continues.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q -p slipstream-cli -p slipstream-daemon -p testserver
B=$PWD/target/debug
export SLIPSTREAM_HOME=$(mktemp -d)   # a private daemon just for this demo
OUT=$(mktemp -d)
PORT=8796
"$B/testserver" --port $PORT >/dev/null &
SRV=$!
trap '"$B/slipstream" daemon stop >/dev/null 2>&1 || true; kill $SRV; rm -rf "$OUT" "$SLIPSTREAM_HOME"' EXIT
sleep 0.5
U="http://127.0.0.1:$PORT/file"
step() { echo; echo "\$ slipstream $*"; "$B/slipstream" "$@"; }

step daemon status
step add "$U/one.bin?size=40000000&throttle=1000000" -d "$OUT" -c 4
step daemon status
sleep 2; step list
step pause 1
sleep 1; step list
step resume 1
sleep 2; step daemon stop
sleep 1; step daemon status
step add "$U/two.bin?size=8000000&throttle=2000000" -d "$OUT"
step list
echo; echo "... waiting for both to finish"
until "$B/slipstream" list | grep -q '^ *1  complete' && "$B/slipstream" list | grep -q '^ *2  complete'; do sleep 1; done
step list
echo
for f in one two; do
  size=$([ $f = one ] && echo 40000000 || echo 8000000)
  want=$(curl -s "$U/$f.bin?size=$size" | sha256sum | cut -c1-16)
  got=$(sha256sum "$OUT/$f.bin" | cut -c1-16)
  [ "$want" = "$got" ] && echo "$f.bin: SHA-256 OK ($got...)" || { echo "$f.bin: MISMATCH"; exit 1; }
done
