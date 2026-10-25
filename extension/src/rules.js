// Pure decision helpers: which browser downloads Slipstream takes (SPEC-browser-capture §4).
// Loaded by the background script in browsers and by the unit tests in Node.
(function (root) {
  'use strict';

  /** Last part of a path or file name ("a/b/c.zip" or "C:\\x\\c.zip" -> "c.zip"). */
  function basename(path) {
    return String(path || '').split(/[/\\]/).pop() || '';
  }

  /** Lowercase extension without the dot, or "" ("Report.PDF" -> "pdf"). */
  function extensionOf(name) {
    const m = /\.([a-z0-9]{1,10})$/i.exec(basename(name));
    return m ? m[1].toLowerCase() : '';
  }

  /** "example.com" matches only itself; "*.example.com" matches it and its subdomains. */
  function hostMatches(host, pattern) {
    host = String(host).toLowerCase();
    pattern = String(pattern).toLowerCase().trim();
    if (pattern.startsWith('*.')) {
      const base = pattern.slice(2);
      return host === base || host.endsWith('.' + base);
    }
    return host === pattern;
  }

  // Responses that are pages or pictures, not files to download.
  const SKIP_MIME = /^(text\/html|application\/(json|xhtml\+xml)|image\/)/i;

  /**
   * Should Slipstream take this download?
   *   d: { url, filename, mime, size, incognito }
   *   s: settings from the app { captureExt, excludeHosts, minSize, enabled }
   *   m: keys held on the click { force (Alt), prevent (Ctrl/Cmd) }
   * Returns { take, why }.
   */
  function decide(d, s, m) {
    const yes = why => ({ take: true, why });
    const no = why => ({ take: false, why });
    m = m || {};
    if (m.prevent) return no('Ctrl held');
    let url;
    try {
      url = new URL(d.url);
    } catch (e) {
      return no('bad URL');
    }
    // blob:, data:, file: only exist inside the browser.
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return no('not http(s)');
    // Keep private browsing private: nothing goes into Slipstream's list.
    if (d.incognito) return no('private window');
    if (m.force) return yes('Alt held');
    if (!s || !s.enabled) return no('catching is off');
    if ((s.excludeHosts || []).some(p => hostMatches(url.hostname, p))) return no('excluded site');
    if (d.mime && SKIP_MIME.test(d.mime)) return no('web page or image');
    const ext = extensionOf(d.filename) || extensionOf(url.pathname);
    if (!ext || !(s.captureExt || []).includes(ext)) return no('file type not in the list');
    if (s.minSize > 0 && d.size > 0 && d.size < s.minSize) return no('smaller than the minimum');
    return yes('matches the rules');
  }

  /** Browser cookies -> one Cookie header value. */
  function cookieHeader(cookies) {
    return cookies.map(c => `${c.name}=${c.value}`).join('; ');
  }

  const api = { basename, extensionOf, hostMatches, decide, cookieHeader };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.SlipstreamRules = api;
})(globalThis);
