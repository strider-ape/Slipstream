// Tells the background script which keys were held when a link was clicked:
// Alt = always use Slipstream, Ctrl (Cmd on macOS) = never. Nothing else is read or sent.
'use strict';
(() => {
  const ext = globalThis.browser ?? globalThis.chrome;
  document.addEventListener(
    'mousedown',
    e => {
      if (e.button > 1 || !(e.altKey || e.ctrlKey || e.metaKey)) return;
      if (!e.target.closest || !e.target.closest('a[href]')) return;
      try {
        const sent = ext.runtime.sendMessage({ type: 'keys', force: e.altKey, prevent: e.ctrlKey || e.metaKey });
        if (sent && sent.catch) sent.catch(() => {});
      } catch (err) {
        // Extension reloaded: this page's script is stale; nothing to do.
      }
    },
    true,
  );
})();
