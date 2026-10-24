# Slipstream browser extension

Plain JavaScript WebExtension for Chrome/Edge (MV3) and Firefox. No bundler.
Built in milestone M6; the design is in [`docs/SPEC-browser-capture.md`](../docs/SPEC-browser-capture.md).

## Fixed IDs
- **Chrome/Edge/Brave (unpacked/dev):** `mlmcfpobghbehfiebmeedgpibmbeoajo`. It is derived from the
  public key in [`key.txt`](key.txt), which goes into the Chrome manifest's `"key"` field so the ID
  never changes. Only the public key exists; no private key is kept or needed for unpacked use.
- **Firefox:** `slipstream@strider-ape.github.io` (`browser_specific_settings.gecko.id`).

The native messaging host (`slipstream-nmhost`) only accepts these IDs. Store-published builds get
their own IDs, which will be added to the host's allow-list then.
