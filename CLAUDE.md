# CLAUDE.md

## Project
`slipstream`: a light, fast, open-source download manager for **Windows and Linux**, written in **Rust**. License: GPL-3.0.
v1 = (1) multi-connection segmented download engine with resume, (2) catching downloads from Chrome/Edge/Firefox via a WebExtension + native messaging host. Nothing else until the user decides on v2.

## Specs (source of truth)
- `docs/SPEC-engine.md`: engine behavior, storage, retries, acceptance targets
- `docs/SPEC-browser-capture.md`: extension, native messaging protocol v1, registration paths
- `docs/ROADMAP-v1.md`: milestones M0–M8; work through them in order
- `docs/TEST-PLAN.md`: test server scenarios, benchmarks, resource budget

`CLAUDE.local.md` (git-ignored, may not exist) has machine-specific notes.

## Rules
- **Clean-room:** implement from the specs only. Never copy third-party code, text, icons or files into this repo. No other product's name in our names or branding.
- Keep it light: few dependencies, check binary size and RAM each milestone (budget in TEST-PLAN §D).
- Always verify TLS certificates. No open TCP ports for IPC (named pipe / Unix socket with peer check).
- Every engine behavior gets a test against `tools/testserver`.
- CI must pass on `windows-latest` and `ubuntu-latest`.
- **Commit and push very often, in small steps** (the user asked for this). No AI attribution lines in commits or PRs.

## About the user
Learner who loves Linux/Android internals. Explain simply and teach as you go ("what this does and why"), step by step, safest first. They make product decisions; you make routine engineering calls and say what you chose. Ask with options and a recommendation when a choice changes the product.

## Layout
```
crates/{core,daemon,cli,nmhost,gui}   extension/   tools/testserver/   docs/
```
GUI: **Slint** with the software renderer (`SLINT_BACKEND=winit-software` behaviour, no OpenGL), classic table layout, theme follows the system. Chosen after the prototypes in `prototypes/` (see its README for the numbers).

Crate package names are prefixed `slipstream-` (e.g. `slipstream-core`); the CLI binary is `slipstream`.

## Commands
- Build/test: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`
- Test server: `cargo run -p testserver`
- Real crash/resume check (Linux/WSL): `scripts/crash-test.sh`
- On the user's Windows laptop, built programs can't run (Smart App Control), so build and test inside WSL2 Ubuntu (`~/src/Slipstream`); CI covers Windows.
