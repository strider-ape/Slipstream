#!/usr/bin/env bash
# Open the prototype windows (they stay open) and measure them after they settle.
cd "$(dirname "$0")"
pkill -f 'release/proto-'; sleep 0.5
setsid nohup egui-table/target/release/proto-egui >/tmp/proto-egui.log 2>&1 &
setsid nohup slint-table/target/release/proto-slint >/tmp/proto-slint.log 2>&1 &
SLINT_BACKEND=winit-software setsid nohup slint-table/target/release/proto-slint >/tmp/proto-slint-sw.log 2>&1 &
sleep 8
printf '%-22s %8s %8s %6s\n' "" "RAM(RSS)" "private" "CPU"
for pid in $(pgrep -f 'release/proto-'); do
  label=$(tr '\0' ' ' </proc/$pid/cmdline | sed 's|.*/||')
  grep -q 'SLINT_BACKEND=winit-software' /proc/$pid/environ 2>/dev/null && label="$label (software)"
  rss=$(awk '/^Rss/ {print int($2/1024)}' /proc/$pid/smaps_rollup)
  priv=$(awk '/^Private_(Clean|Dirty)/ {s+=$2} END {print int(s/1024)}' /proc/$pid/smaps_rollup)
  t0=$(awk '{print $14+$15}' /proc/$pid/stat); sleep 5; t1=$(awk '{print $14+$15}' /proc/$pid/stat)
  cpu=$(( (t1 - t0) * 100 / $(getconf CLK_TCK) / 5 ))
  printf '%-22s %5d MiB %5d MiB %5d%%\n' "$label" "$rss" "$priv" "$cpu"
done
echo "GL renderer: $(grep -m1 -iE 'renderer|llvmpipe|d3d12' /tmp/proto-*.log 2>/dev/null | head -1)"
ls /usr/lib/x86_64-linux-gnu/libLLVM* 2>/dev/null | head -1 | xargs -r du -h
