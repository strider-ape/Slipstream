<p align="center"><img src="assets/icons/slipstream-128.png" width="96" alt=""></p>

<h1 align="center">Slipstream</h1>

<p align="center">A light, fast, open-source download manager for <b>Linux and Windows</b>, written in Rust.</p>

<p align="center">
  <a href="https://github.com/strider-ape/Slipstream/releases">Download</a> ·
  <a href="docs/benchmarks.md">Benchmarks</a> ·
  <a href="docs/ROADMAP-v1.md">Roadmap</a>
</p>

> **Preview.** Slipstream works, but it is young. Expect rough edges and please [report them](https://github.com/strider-ape/Slipstream/issues).

## What it does
- **Fast downloads.** Splits each file across up to 8 connections and keeps splitting while that makes it faster. It stops adding connections when they no longer help, so it never loses to a single stream on a line one connection already fills. Slow or stuck parts at the end get a second connection racing them.
- **Never starts over.** Pause, quit, crash or reboot: the download continues from the exact byte where it stopped. If the file changed on the server, it starts clean instead of mixing two versions.
- **Catches downloads from your browser.** Click a file link in Chrome, Edge, Brave, Vivaldi or Chromium and Slipstream takes it, with the cookies it needs (downloads behind a login work). Right-click any link → *Download with Slipstream*.
- **Light.** The desktop app uses about 25 MB of RAM; the programs together are under 20 MB. On Windows it waits in the tray.
- **Private.** No accounts, no telemetry, no network port: the app and its background service talk only to each other, and only for your user.

## Install
Get the files from the [Releases page](https://github.com/strider-ape/Slipstream/releases).

| System | File | How |
|---|---|---|
| Windows 10/11 (x64) | `Slipstream-Setup-<version>.exe` | Run it. No administrator rights needed. It isn't code-signed yet, so Windows may say "Windows protected your PC": click *More info* → *Run anyway*. |
| Debian / Ubuntu | `slipstream_<version>_amd64.deb` | `sudo apt install ./slipstream_<version>_amd64.deb`, then open *Slipstream* from your app menu. |
| Other Linux, portable | `.tar.gz` / `.zip` | Unpack anywhere and run `slipstream register-browsers` once. |

### Browser extension
The extension isn't in the browser stores yet. Until it is:
- **Chrome, Edge, Brave, Vivaldi:** unzip `slipstream-extension-chrome-<version>.zip`, open `chrome://extensions`, turn on *Developer mode*, click *Load unpacked* and pick the folder.
- **Firefox:** `about:debugging` → *This Firefox* → *Load Temporary Add-on* → the `manifest.json` in the unzipped Firefox package (it is removed when Firefox restarts until a signed version is published).

Then click a download link: the *Download caught* window asks where to save it.

## Command line
Everything the app does is also available from a terminal:
```
slipstream get URL [-d DIR]          # download in this terminal; Ctrl+C pauses, run again to resume
slipstream add URL [-d DIR]          # hand it to the background service
slipstream list | watch
slipstream pause ID | resume ID | remove ID [--delete-file]
slipstream daemon status | stop
slipstream autostart on | off        # start the background service at login
slipstream register-browsers | unregister-browsers
```

## How it works
Four small programs:

| Program | Job |
|---|---|
| `slipstream-gui` | The window, tray icon and *Download caught* popup. |
| `slipstream-daemon` | The background service: runs the downloads, keeps the list in SQLite, resumes after restarts. Started on demand. |
| `slipstream` | The command line. |
| `slipstream-nmhost` | The bridge the browser extension talks to (native messaging). |

A download in progress lives next to its final name as `<name>.slipstream.part` (the data) and `<name>.slipstream.json` (which bytes are done). The details are in [the engine spec](docs/SPEC-engine.md) and [the browser capture spec](docs/SPEC-browser-capture.md).

## Build from source
Needs [Rust](https://rustup.rs) (stable). On Linux also `libfontconfig1-dev` and `libxkbcommon-dev`.
```
cargo build --release
cargo test --workspace
bash extension/build.sh               # extension/dist/chrome and extension/dist/firefox
```
`tools/testserver` is a local HTTP server that fakes slow, flaky and tricky servers for the tests and benchmarks in `scripts/`.

## Not yet
HTTP(S) only for now. Video streams, FTP, proxies, queues and a scheduler come after v1. Also open: Alt+click to force a catch, a Linux tray icon, store listings for the extension, and code signing on Windows.

## License
[GPL-3.0-or-later](LICENSE)
