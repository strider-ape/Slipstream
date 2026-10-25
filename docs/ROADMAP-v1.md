# ROADMAP: v1 = fast segmented engine + browser catching

Work milestone by milestone. Each one ends with a demo the user can run, tests passing on **Linux (WSL2) and Windows**, and a commit. Explain what was built and why in simple words.

| # | Milestone | Done when |
|---|---|---|
| **M0** | Repo setup: name, license, Cargo workspace (`core daemon cli nmhost gui`), `extension/`, `tools/testserver`, GitHub Actions CI on `windows-latest` + `ubuntu-latest` (fmt, clippy, test). Copy `handoff/repo-CLAUDE.md` → repo `CLAUDE.md`; copy both SPECs → repo `docs/`. | CI is green on an empty skeleton |
| **M1** | Test server (`tools/testserver`, axum): Range support, per-connection throttle, a fault mode (drop the connection after N bytes), ignore-Range mode, changing-ETag mode, Content-Disposition names, cookie-protected route, POST-generated file. | `cargo run -p testserver` serves all scenarios in `TEST-PLAN.md` |
| **M2** | Single-connection download in `core` + `cli`: probe, filename rules, sparse file + sidecar, resume after kill, If-Range. | `slipstream get URL` works; kill/resume gives the same SHA-256 |
| **M3** | **Dynamic segmentation**: ramp-up, best_split/split, keep-alive reuse, hedging, safety checks, retries/backoff, token-bucket limiter, progress events. | All engine acceptance targets in SPEC-engine §13 pass against the test server; a real-world benchmark is recorded in `docs/benchmarks.md` |
| **M4** | Daemon + IPC (named pipe / Unix socket) + SQLite catalogue; the CLI becomes a client (`add`, `list`, `pause`, `resume`, `remove`); single instance; autostart option. | Two CLIs can add and watch downloads; a daemon restart resumes unfinished ones |
| **M5** | `nmhost` + `slipstream --register-browsers` for Chrome/Edge/Brave/Firefox on Windows (HKCU) and Linux (JSON files). | A host round-trip test passes (`hello` → `settings`) |
| **M6** | WebExtension (Chrome/Edge MV3 + Firefox): capture flow, cookies/headers/POST, rules from the daemon, Alt/Ctrl modifiers, context menu, 3 s fallback. | SPEC-browser-capture §10 passes on both OSes |
| **M7** | Minimal **Slint** GUI (software renderer, classic table layout, follows system light/dark; see `prototypes/README.md`): download list (name, size, progress, speed, ETA, status), segment bar, Add-URL dialog, "new download caught" popup (folder + Start/Cancel), pause/resume/open folder, tray icon, settings (max connections, download folder, speed limit, catch on/off). | The user can do everything without a terminal; idle RAM < 40 MB |
| **M8** | Packaging + first release: Windows zip + installer (`cargo-wix` MSI or Inno Setup), Linux `.deb` (`cargo-deb`) + AppImage; GitHub Release via CI; README with screenshots. | A fresh machine install catches a Chrome download end-to-end |

After M8: sit down with the user and choose v2 (likely video/HLS+DASH with ffmpeg, queues/scheduler, categories, site grabber, HTTP/3).

## Working agreements for the building Claude
- Keep crates small and dependencies few; check binary size and RAM at each milestone (the user cares about "light and fast").
- Every engine behavior in the SPEC gets a test against the test server.
- Commit after each green step; push to GitHub.
- Write everything from these specs; never copy third-party code or assets into the repo.
