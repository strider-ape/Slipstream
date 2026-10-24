#!/usr/bin/env bash
# Real-world benchmark (TEST-PLAN §B): one plain stream (curl, like a browser) vs
# Slipstream vs aria2c on the same URL. Files go to RAM (/dev/shm) so the disk
# doesn't skew results; tools take turns in each round so network swings hit all.
# Prints one markdown row per tool: median speed, every run, SHA-256 prefix.
# Usage: [TOOLS="0 1"] scripts/bench-real.sh URL [RUNS]   (needs curl, aria2c, bc)
# TOOLS picks rows: 0 curl, 1 slipstream -c 8, 2 slipstream -c 16, 3 aria2c.
set -euo pipefail
cd "$(dirname "$0")/.."
URL=$1
RUNS=${2:-3}
cargo build --release -q -p slipstream-cli
SS=target/release/slipstream
OUT=$(mktemp -d -p /dev/shm)
trap 'rm -rf "$OUT"' EXIT

LABELS=("curl, 1 stream" "slipstream -c 8" "slipstream -c 16" "aria2c -x16 -s16")
tool() {
  case $1 in
    0) curl -sSL -o "$OUT/f" "$URL" ;;
    1) "$SS" get "$URL" -d "$OUT" -o f -c 8 ;;
    2) "$SS" get "$URL" -d "$OUT" -o f -c 16 ;;
    3) aria2c -q -x16 -s16 -k1M --file-allocation=none -d "$OUT" -o f "$URL" ;;
  esac
}

declare -a SPEEDS SUMS
for r in $(seq "$RUNS"); do
  for i in ${TOOLS:-0 1 2 3}; do
    rm -f "$OUT"/*
    t0=$(date +%s.%N)
    if ! tool "$i" >/dev/null 2>"$OUT/err"; then
      echo "  run $r: ${LABELS[i]}: FAILED: $(tr '\r' '\n' <"$OUT/err" | grep . | tail -1)" >&2
      SPEEDS[i]+="fail "
      continue
    fi
    t1=$(date +%s.%N)
    mbs=$(echo "$(stat -c %s "$OUT/f") / ($t1 - $t0) / 1000000" | bc -l)
    SPEEDS[i]+="$(printf '%.1f' "$mbs") "
    SUMS[i]=$(sha256sum "$OUT/f" | cut -c1-12)
    echo "  run $r: ${LABELS[i]}: $(printf '%.1f' "$mbs") MB/s" >&2
  done
done

echo "| Tool | Median | Runs (MB/s) | SHA-256 |"
echo "|---|---|---|---|"
for i in ${TOOLS:-0 1 2 3}; do
  median=$(tr ' ' '\n' <<<"${SPEEDS[i]}" | grep -v fail | grep . | sort -g | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}')
  echo "| ${LABELS[i]} | **$median MB/s** | ${SPEEDS[i]% } | \`${SUMS[i]}\` |"
done
