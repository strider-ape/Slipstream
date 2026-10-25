# Slipstream

A light, fast, open-source download manager for **Windows and Linux**, written in Rust.

**Status:** early development. M7 GUI working on Windows and Linux (tray icon next). M6 done for Chromium browsers (extension tested in Brave on Windows and Chrome on Linux; Firefox/Zen and Alt+click still open). M5: browser bridge. M4: background daemon, `add`/`list`/`watch`. M3: up to 8 connections per file with dynamic splitting,
slow-tail racing, retries and a speed limit; resumes byte-exact after `kill -9`. See [benchmarks](docs/benchmarks.md).

## Try it
```
cargo run --release -p slipstream-cli -- get https://example.com/file.zip -d downloads
```
Press Ctrl+C to pause; run the same command again to resume. A download is kept in
`<name>.slipstream.part` with its progress in `<name>.slipstream.json` until it finishes.

### Background daemon
`add` hands a download to `slipstream-daemon` (started automatically), which keeps the
list in SQLite and continues unfinished downloads after a restart:
```
slipstream add URL [-d DIR]     # default folder: your Downloads
slipstream list | watch
slipstream pause ID | resume ID | remove ID [--delete-file]
slipstream daemon status | stop
slipstream autostart on | off   # start the daemon at login
```
Clients talk to the daemon over a private Unix socket / named pipe (same user only, no
network port). Demo: `scripts/daemon-demo.sh`.

### Browser bridge
`slipstream register-browsers` tells Chrome, Chromium, Edge, Brave, Vivaldi and Firefox
where `slipstream-nmhost` is (Linux: a JSON file per installed browser; Windows: an HKCU
registry key per browser). The browser extension (coming next) uses it to hand downloads
over with their cookies. `unregister-browsers` undoes it. Snap and Flatpak browsers on
Linux are sandboxed and need extra setup, not covered yet.

## Goals for v1
1. **Fast downloads**: many connections per file with dynamic segmentation, resume after crashes or reboots.
2. **Browser catching**: a Chrome/Edge/Firefox extension hands downloads to Slipstream, with the browser's cookies and headers.

Everything else (video/HLS, queues, scheduler) comes after v1.

## License
[GPL-3.0](LICENSE)
