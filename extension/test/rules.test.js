// Unit tests for the decision rules: `node --test extension/test/rules.test.js`
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const R = require('../src/rules.js');

const settings = {
  captureExt: ['zip', 'iso', 'pdf', 'mp4'],
  excludeHosts: ['*.internal.example', 'intranet.local'],
  minSize: 0,
  enabled: true,
};
const plain = { force: false, prevent: false };
const dl = over => ({ url: 'https://files.example.com/dl/ubuntu.iso', filename: '', mime: '', size: 0, incognito: false, ...over });

test('names and extensions', () => {
  assert.equal(R.basename('C:\\Users\\me\\Downloads\\a.zip'), 'a.zip');
  assert.equal(R.basename('/home/me/b.tar.gz'), 'b.tar.gz');
  assert.equal(R.extensionOf('Report.PDF'), 'pdf');
  assert.equal(R.extensionOf('noext'), '');
  assert.equal(R.extensionOf('/path.with.dots/file'), '');
});

test('host patterns', () => {
  assert.ok(R.hostMatches('a.internal.example', '*.internal.example'));
  assert.ok(R.hostMatches('internal.example', '*.internal.example'));
  assert.ok(!R.hostMatches('notinternal.example', '*.internal.example'));
  assert.ok(R.hostMatches('Intranet.Local', 'intranet.local'));
  assert.ok(!R.hostMatches('x.intranet.local', 'intranet.local'));
});

test('takes files whose type is in the list', () => {
  assert.equal(R.decide(dl(), settings, plain).take, true);
  // The name the browser decided (Content-Disposition) wins over the URL.
  const viaName = dl({ url: 'https://x.example.com/get?id=7', filename: 'C:\\dl\\movie.mp4' });
  assert.equal(R.decide(viaName, settings, plain).take, true);
  assert.equal(R.decide(dl({ url: 'https://x.example.com/song.ogg' }), settings, plain).take, false);
});

test('keys win: Ctrl never, Alt always', () => {
  assert.deepEqual(R.decide(dl(), settings, { prevent: true }), { take: false, why: 'Ctrl held' });
  const odd = dl({ url: 'https://x.example.com/data.csv' });
  assert.deepEqual(R.decide(odd, settings, { force: true }), { take: true, why: 'Alt held' });
  assert.equal(R.decide(odd, { ...settings, enabled: false }, { force: true }).take, true);
});

test('never takes what it cannot or should not', () => {
  assert.equal(R.decide(dl({ url: 'blob:https://x.example.com/1234' }), settings, plain).take, false);
  assert.equal(R.decide(dl({ url: 'data:application/zip;base64,AA==' }), settings, { force: true }).take, false);
  assert.equal(R.decide(dl({ incognito: true }), settings, { force: true }).take, false);
  assert.equal(R.decide(dl({ url: 'https://a.internal.example/x.zip' }), settings, plain).take, false);
  assert.equal(R.decide(dl({ mime: 'text/html; charset=utf-8' }), settings, plain).take, false);
  assert.equal(R.decide(dl(), { ...settings, enabled: false }, plain).take, false);
  assert.equal(R.decide(dl(), null, plain).take, false);
  assert.equal(R.decide(dl({ url: 'not a url' }), settings, plain).take, false);
});

test('minimum size applies only when the size is known', () => {
  const s = { ...settings, minSize: 1000000 };
  assert.equal(R.decide(dl({ size: 5000 }), s, plain).take, false);
  assert.equal(R.decide(dl({ size: 0 }), s, plain).take, true);
  assert.equal(R.decide(dl({ size: 2000000 }), s, plain).take, true);
});

test('cookie header', () => {
  assert.equal(R.cookieHeader([{ name: 'a', value: '1' }, { name: 'session', value: 'x y' }]), 'a=1; session=x y');
  assert.equal(R.cookieHeader([]), '');
});
