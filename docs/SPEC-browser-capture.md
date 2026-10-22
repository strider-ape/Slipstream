# SPEC: Browser catching (v1)

Clean-room spec. It uses only public WebExtension and native-messaging APIs.

## 1. Pieces
```
extension (Chrome/Edge MV3 + Firefox)  ──native messaging (stdio)──►  nmhost.exe / nmhost
                                                                         │ local IPC
                                                                         ▼
                                                               daemon (engine + list + "Add" dialog via GUI)
```
- **extension/**: plain JS, no bundler. One codebase; `manifest.chrome.json` and `manifest.firefox.json` are generated or copied by a tiny build script.
- **nmhost**: tiny Rust binary. It reads `[u32 little-endian length][UTF-8 JSON]` from stdin and writes the same to stdout, forwards to the daemon, and **starts the daemon if it isn't running**. Never print anything else to stdout (it corrupts the stream); log to a file.
- **IPC**: Windows named pipe `\\.\pipe\slipstream-<user SID or session>`; Linux Unix socket `$XDG_RUNTIME_DIR/slipstream/ctl.sock` (mode 0700 dir). Check the peer: same user (`SO_PEERCRED` on Linux; `GetNamedPipeClientProcessId` + token user on Windows). **No TCP port.**

## 2. Capture flow (Chrome/Edge MV3)
1. `webRequest.onBeforeSendHeaders` / `onHeadersReceived` with `extraHeaders` (non-blocking): keep a short LRU (60 s) of request context keyed by URL and requestId: method, request headers, response headers, status, tab, frame, initiator.
2. `downloads.onCreated` → **pause** the download immediately. Look up the context (match by final URL / referrer / time).
3. `downloads.onDeterminingFilename` → return `true` (hold the filename decision) and send `capture` to the host.
4. Host answers `verdict`:
   - `take` → `downloads.cancel` + `downloads.erase`; the daemon starts the download (and the GUI shows it).
   - `release` → `suggest()` + `downloads.resume`, and the browser keeps it.
   - No answer within ~3 s → `release` (never break the user's browser).
5. Cookies: `chrome.cookies.getAll({url})` (respect partitioned cookies) → a `Cookie` header string. Plus the UA (`navigator.userAgent`) and the referer.
6. POST-generated downloads: capture `requestBody` in `onBeforeRequest` (raw bytes or formData) and include it (base64; chunk above ~512 KB).

## 3. Firefox
`webRequest.onHeadersReceived` **blocking** (still allowed in Firefox MV2/MV3) → if the response looks like a download (Content-Disposition: attachment, or a MIME/extension in the capture list, and no `X-Requested-With`) → send `capture` and **await the verdict in the listener**: `take` → `{cancel: true}`; `release` → pass through. Backstop: `downloads.onCreated` → cancel + erase if taken.

## 4. What counts as "a download to catch"
Decided by the **daemon**, which sends rules to the extension on connect (`settings` message), so rule changes never need an extension update:
- Extension list (default: zip rar 7z tar gz xz bz2 iso img exe msi msix apk deb rpm appimage dmg pkg bin mp4 mkv avi mov webm mp3 flac wav m4a pdf epub torrent …)
- Minimum size (default 0; e.g. skip < 1 MB optional)
- Excluded hosts (user-editable), excluded MIME types (text/html, json, images by default)
- Modifier keys: **Alt held = force catch**, **Ctrl/Insert = don't catch** (tracked in a content script via keydown/keyup and sent to the background with the click).
- Context menu: "Download with slipstream" on links; "Download all links with slipstream" (later).

## 5. Protocol v1 (JSON, every message has `v` and `type`)
```json
// extension → host
{"v":1,"type":"hello","browser":"Chrome/141","ext":"0.1.0"}
{"v":1,"type":"capture","id":"c-42","source":"download|context|webrequest",
 "url":"https://...","finalUrl":"https://...","method":"GET",
 "headers":[["Referer","..."],["User-Agent","..."],["Cookie","a=b"]],
 "body":null, "filename":"file.zip","mime":"application/zip","size":123456,
 "pageUrl":"https://...","tab":7,"incognito":false,"modifiers":{"force":false,"prevent":false}}
// host → extension
{"v":1,"type":"settings","captureExt":["zip","exe"],"excludeHosts":["*.example.com"],"minSize":0,"enabled":true}
{"v":1,"type":"verdict","id":"c-42","action":"take|release"}
{"v":1,"type":"error","id":"c-42","message":"daemon not reachable"}
```
Limits: host → browser messages max 1 MB; browser → host is larger, but chunk bodies anyway (`"body":{"b64":"...","part":1,"of":3}`).

## 6. Registration of the native host (installer + `slipstream --register-browsers` repair command)
Host name: `org.slipstream.host` (must match `^[a-z0-9_.]+$`).
| Browser | Windows (registry key, default value = full path of the JSON) | Linux (per-user JSON file) |
|---|---|---|
| Chrome | `HKCU\Software\Google\Chrome\NativeMessagingHosts\org.slipstream.host` | `~/.config/google-chrome/NativeMessagingHosts/org.slipstream.host.json` |
| Chromium | `HKCU\Software\Chromium\NativeMessagingHosts\…` | `~/.config/chromium/NativeMessagingHosts/` |
| Edge | `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\…` | `~/.config/microsoft-edge/NativeMessagingHosts/` |
| Brave | (uses Chrome key) | `~/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts/` |
| Vivaldi | (uses Chrome key) | `~/.config/vivaldi/NativeMessagingHosts/` |
| Firefox | `HKCU\Software\Mozilla\NativeMessagingHosts\org.slipstream.host` | `~/.mozilla/native-messaging-hosts/org.slipstream.host.json` |

Manifest bodies:
- Chromium: `{"name":"org.slipstream.host","description":"...","path":"<abs path to nmhost>","type":"stdio","allowed_origins":["chrome-extension://<EXT_ID>/"]}`
- Firefox: same, but `"allowed_extensions":["slipstream@<your-domain>"]`.

Linux caveats: Snap and Flatpak browsers are sandboxed. Recent Snap Firefox uses the xdg-desktop-portal for native messaging; Flatpak needs a `flatpak-spawn --host` wrapper. Document this; don't block v1 on it.

## 7. Extension ID stability
- Dev: put a fixed `"key"` (public key) in the Chrome manifest so the unpacked extension ID never changes. The ID goes into `allowed_origins`.
- Release: Chrome Web Store + Edge Add-ons + Firefox AMO (Firefox requires signing even for self-distribution; "unlisted" signing works). Store review wants a clear privacy policy: we read cookies/headers only for the download being handed off, nothing is sent anywhere except the local app.

## 8. Permissions (ask for the minimum)
`downloads`, `webRequest`, `cookies`, `contextMenus`, `nativeMessaging`, `storage`, host permission `<all_urls>`. (No `declarativeNetRequest`, `management` or `proxy` in v1.)

## 9. Later (not v1)
- The in-page "Download video" button: content script + closed Shadow DOM over `<video>`, positioned with ResizeObserver/IntersectionObserver.
- "Fetch via browser" fallback for links that only work inside the browser session.
- HLS/DASH detection by MIME (`application/vnd.apple.mpegurl`, `application/dash+xml`) and extension.

## 10. Acceptance for v1
- Clicking a .zip link in Chrome, Edge and Firefox on Windows **and** Linux → the browser download disappears and the download appears in our app with the right name, size and cookies (it works on a login-protected test page).
- With the daemon stopped → the extension falls back to the browser download within 3 s.
- Alt+click forces the catch; Ctrl+click never catches.
