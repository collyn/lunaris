import { test } from 'node:test';
import assert from 'node:assert/strict';
import { entryScript, isNewerBuild } from '../src/freshPage.ts';

test('the entry script is read from a built index.html', () => {
  const html = '<head><script src="/theme-init.js"></script>\n<script type="module" crossorigin src="./assets/index-B9x_k2.js"></script></head>';
  assert.equal(entryScript(html), 'assets/index-B9x_k2.js');
  assert.equal(entryScript('<script type="module" src="/assets/index-Q1.js">'), 'assets/index-Q1.js');
  assert.equal(entryScript('<script type="module" src="https://h:8443/assets/index-Q1.js">'), 'assets/index-Q1.js');
});

test('a development page names no fingerprinted script', () => {
  assert.equal(entryScript('<script type="module" src="/src/main.tsx"></script>'), null);
  assert.equal(entryScript('<html></html>'), null);
});

test('only another known build counts as newer', () => {
  assert.equal(isNewerBuild('assets/index-A.js', 'assets/index-B.js'), true);
  assert.equal(isNewerBuild('assets/index-A.js', 'assets/index-A.js'), false);
  assert.equal(isNewerBuild(null, 'assets/index-B.js'), false);
  assert.equal(isNewerBuild('assets/index-A.js', null), false);
});
