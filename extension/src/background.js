// Slipstream background script: the Chrome/Edge MV3 service worker and the Firefox
// background script. It notices downloads, asks the Slipstream app (through the native
// messaging host) whether to take them, and then cancels or resumes the browser's copy.
'use strict';

if (typeof SlipstreamRules === 'undefined') importScripts('rules.js'); // Chrome service worker

const ext = globalThis.browser ?? globalThis.chrome;
const HOST = 'org.slipstream.host';
const VERDICT_TIMEOUT_MS = 3000; // no answer by then: the browser keeps the download
const MODIFIER_WINDOW_MS = 3000; // how long a held Alt/Ctrl counts for the next download
const REQUEST_MEMORY_MS = 60000;

let port = null;
let settings = null; // capture rules from the app
let settingsWaiters = [];
const pending = new Map(); // capture id -> resolve(reply)
let seq = 0;
let lastKeys = { force: false, prevent: false, at: 0 };
const decisions = new Map(); // download id -> Promise<'take' | 'release'>
const requests = new Map(); // url -> { method, mime, size, at }, from webRequest

// One line per decision in the extension's console (service worker / background page).
function log(...parts) {
  console.info('[slipstream]', ...parts);
}

// ---- connection to the app ------------------------------------------------------------

function connect() {
  if (port) return port;
  try {
    port = ext.runtime.connectNative(HOST);
  } catch (e) {
    setStatus(false, String(e));
    return null;
  }
  port.onMessage.addListener(onHostMessage);
  port.onDisconnect.addListener(p => {
    const why = (p && p.error && p.error.message) || ext.runtime.lastError?.message || 'not running';
    port = null;
    for (const resolve of pending.values()) resolve({ type: 'error', message: why });
    pending.clear();
    setStatus(false, why);
  });
  const browser = navigator.userAgent;
  port.postMessage({ v: 1, type: 'hello', browser, ext: ext.runtime.getManifest().version });
  return port;
}

function onHostMessage(msg) {
  if (msg.type === 'settings') {
    settings = msg;
    settingsWaiters.forEach(resolve => resolve(settings));
    settingsWaiters = [];
    setStatus(true);
  } else if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  } else if (msg.type === 'error') {
    setStatus(false, msg.message);
  }
}

/** The capture rules; connects first if needed (waits up to 1.5 s). */
function ensureSettings() {
  if (settings) return Promise.resolve(settings);
  connect();
  return new Promise(resolve => {
    settingsWaiters.push(resolve);
    setTimeout(() => resolve(settings), 1500);
  });
}

/** Hand a download to the app; resolves with its reply, or an error after 3 s. */
function ask(capture) {
  return new Promise(resolve => {
    const p = connect();
    if (!p) return resolve({ type: 'error', message: 'app not available' });
    const id = `c-${Date.now()}-${++seq}`;
    const timer = setTimeout(() => {
      pending.delete(id);
      resolve({ type: 'error', message: 'no answer within 3 s' });
    }, VERDICT_TIMEOUT_MS);
    pending.set(id, reply => {
      clearTimeout(timer);
      resolve(reply);
    });
    p.postMessage({ v: 1, type: 'capture', id, ...capture });
  });
}

/** Headers the app needs to fetch the file as this browser would. */
async function headersFor(url, referrer) {
  const headers = [['User-Agent', navigator.userAgent]];
  if (referrer) headers.push(['Referer', referrer]);
  try {
    const cookies = await ext.cookies.getAll({ url });
    if (cookies.length) headers.push(['Cookie', SlipstreamRules.cookieHeader(cookies)]);
  } catch (e) {
    // No cookie access for this URL: send without.
  }
  return headers;
}

// ---- deciding -------------------------------------------------------------------------

/** Take the keys held on the last link click, if it was recent; each click counts once. */
function takeKeys() {
  const fresh = Date.now() - lastKeys.at < MODIFIER_WINDOW_MS;
  const keys = fresh ? { force: lastKeys.force, prevent: lastKeys.prevent } : { force: false, prevent: false };
  lastKeys = { force: false, prevent: false, at: 0 };
  return keys;
}

async function decide(item) {
  const keys = takeKeys();
  const url = item.finalUrl || item.url;
  const seen = requests.get(url) || requests.get(item.url) || {};
  // Downloads made by a form POST can't be repeated by the app yet.
  if (seen.method && seen.method !== 'GET') {
    log('browser keeps it', `made by ${seen.method}`, url);
    return 'release';
  }
  const d = {
    url,
    filename: item.filename || '',
    mime: item.mime || seen.mime || '',
    size: item.totalBytes > 0 ? item.totalBytes : item.fileSize > 0 ? item.fileSize : seen.size || 0,
    incognito: !!item.incognito,
  };
  const verdict = SlipstreamRules.decide(d, await ensureSettings(), keys);
  log(verdict.take ? 'asking the app' : 'browser keeps it', verdict.why, url, keys);
  if (!verdict.take) return 'release';
  const reply = await ask({
    source: 'download',
    url: item.url,
    finalUrl: url,
    method: 'GET',
    headers: await headersFor(item.url, item.referrer),
    filename: SlipstreamRules.basename(d.filename) || null,
    mime: d.mime || null,
    size: d.size || null,
    pageUrl: item.referrer || null,
    incognito: d.incognito,
    modifiers: keys,
  });
  log('app answered', reply.action || reply.message, url);
  if (reply.type === 'verdict') return reply.action === 'take' ? 'take' : 'release';
  setStatus(false, reply.message);
  return 'release';
}

/** Apply a verdict to the browser's download. */
async function finish(id, verdict) {
  if (!decisions.delete(id)) return; // already done
  try {
    if (verdict === 'take') {
      await ext.downloads.cancel(id);
      await ext.downloads.erase({ id });
      flash('✓', '#2e7d32');
    } else {
      await ext.downloads.resume(id);
    }
  } catch (e) {
    // The user may have cancelled it meanwhile, or it wasn't paused.
  }
}

function decisionFor(item) {
  if (!decisions.has(item.id)) decisions.set(item.id, decide(item).catch(() => 'release'));
  return decisions.get(item.id);
}

// Chrome/Edge: hold the "choose a file name" step until we know.
const canHoldFilename = !!ext.downloads.onDeterminingFilename;

ext.downloads.onCreated.addListener(item => {
  // Only new downloads, and never ones an extension started.
  if (item.state !== 'in_progress' || item.byExtensionId) return;
  ext.downloads.pause(item.id).catch(() => {});
  const decision = decisionFor(item);
  // Firefox has no file-name hold. On Chrome this is a backstop in case the
  // file-name event never comes (e.g. "ask where to save" is on).
  decision.then(v => setTimeout(() => finish(item.id, v), canHoldFilename ? 5000 : 0));
});

if (canHoldFilename) {
  ext.downloads.onDeterminingFilename.addListener((item, suggest) => {
    if (item.byExtensionId) return false;
    decisionFor(item).then(v => {
      if (v === 'release') suggest(); // keep the browser's own name
      finish(item.id, v);
    });
    return true; // we answer asynchronously
  });
}

// ---- what the page told us ------------------------------------------------------------

function remember(url, info) {
  const now = Date.now();
  requests.set(url, { ...(requests.get(url) || {}), ...info, at: now });
  if (requests.size > 500) {
    for (const [u, r] of requests) if (now - r.at > REQUEST_MEMORY_MS) requests.delete(u);
  }
}

const pageRequests = { urls: ['<all_urls>'], types: ['main_frame', 'sub_frame', 'other'] };
ext.webRequest.onBeforeRequest.addListener(r => remember(r.url, { method: r.method }), pageRequests);
ext.webRequest.onHeadersReceived.addListener(
  r => {
    const get = name => r.responseHeaders?.find(h => h.name.toLowerCase() === name)?.value;
    remember(r.url, { mime: get('content-type') || '', size: Number(get('content-length')) || 0 });
  },
  pageRequests,
  ['responseHeaders'],
);

// The content script reports Alt/Ctrl held on a link click.
ext.runtime.onMessage.addListener(msg => {
  if (msg && msg.type === 'keys') {
    lastKeys = { force: !!msg.force, prevent: !!msg.prevent, at: Date.now() };
    log('keys held on a link click', lastKeys);
  }
});

// ---- right-click menu -----------------------------------------------------------------

ext.runtime.onInstalled.addListener(() => {
  ext.contextMenus.create({ id: 'slipstream-link', title: 'Download with Slipstream', contexts: ['link'] });
  connect();
});

ext.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId !== 'slipstream-link' || !info.linkUrl) return;
  log('right-click menu', info.linkUrl);
  if (tab && tab.incognito) return flash('!', '#c62828'); // private windows stay private
  const reply = await ask({
    source: 'context',
    url: info.linkUrl,
    finalUrl: info.linkUrl,
    method: 'GET',
    headers: await headersFor(info.linkUrl, info.pageUrl),
    filename: null,
    pageUrl: info.pageUrl || null,
    incognito: false,
    modifiers: { force: true, prevent: false },
  });
  if (reply.type === 'verdict' && reply.action === 'take') flash('✓', '#2e7d32');
  else flash('!', '#c62828');
});

// ---- toolbar status -------------------------------------------------------------------

function setStatus(ok, why) {
  ext.action.setTitle({ title: ok ? 'Slipstream: connected' : `Slipstream: ${why}` });
  ext.action.setBadgeText({ text: ok ? '' : '!' });
  if (!ok) ext.action.setBadgeBackgroundColor({ color: '#c62828' });
}

function flash(text, color) {
  ext.action.setBadgeBackgroundColor({ color });
  ext.action.setBadgeText({ text });
  setTimeout(() => ext.action.setBadgeText({ text: settings ? '' : '!' }), 2500);
}

ext.runtime.onStartup.addListener(connect);
connect();
