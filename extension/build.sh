#!/usr/bin/env bash
# Assemble the extension for each browser (same scripts, different manifest):
#   extension/dist/chrome   -> Chrome/Edge/Brave: "Load unpacked"
#   extension/dist/firefox  -> Firefox: about:debugging -> "Load Temporary Add-on" (manifest.json)
set -euo pipefail
cd "$(dirname "$0")"
rm -rf dist
for browser in chrome firefox; do
  mkdir -p "dist/$browser"
  cp src/*.js "dist/$browser/"
  cp "manifest.$browser.json" "dist/$browser/manifest.json"
done
echo "built $(pwd)/dist/chrome and $(pwd)/dist/firefox"
