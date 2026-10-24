# Slipstream

A light, fast, open-source download manager for **Windows and Linux**, written in Rust.

**Status:** early development. M2 done: single-connection downloads that survive `kill -9`
and resume byte-exact (`scripts/crash-test.sh`). Next: M3, many connections per file.

## Try it
```
cargo run --release -p slipstream-cli -- get https://example.com/file.zip -d downloads
```
Press Ctrl+C to pause; run the same command again to resume. A download is kept in
`<name>.slipstream.part` with its progress in `<name>.slipstream.json` until it finishes.

## Goals for v1
1. **Fast downloads**: many connections per file with dynamic segmentation, resume after crashes or reboots.
2. **Browser catching**: a Chrome/Edge/Firefox extension hands downloads to Slipstream, with the browser's cookies and headers.

Everything else (video/HLS, queues, scheduler) comes after v1.

## License
[GPL-3.0](LICENSE)
