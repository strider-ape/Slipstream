# Slipstream

A light, fast, open-source download manager for **Windows and Linux**, written in Rust.

**Status:** early development (milestone M0). Nothing usable yet.

## Goals for v1
1. **Fast downloads**: many connections per file with dynamic segmentation, resume after crashes or reboots.
2. **Browser catching**: a Chrome/Edge/Firefox extension hands downloads to Slipstream, with the browser's cookies and headers.

Everything else (video/HLS, queues, scheduler) comes after v1.

## License
[GPL-3.0](LICENSE)
