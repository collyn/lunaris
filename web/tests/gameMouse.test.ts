// The "Game mouse" row of a computer's settings (src/gameMouse.ts).
// Run: npm run test:game-mouse   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { USBIP_WIN2_RELEASES, gameMouseInfo } from '../src/gameMouse.ts';

const windows = (game_mouse?: string) => ({ version: '0.7.8', os: 'windows', install: 'windows', game_mouse });

test('only Windows computers have the row', () => {
  assert.equal(gameMouseInfo(undefined), null);
  assert.equal(gameMouseInfo(null), null);
  assert.equal(gameMouseInfo({ version: '0.7.8', os: 'linux', install: 'deb' }), null);
});

test('with usbip-win2 the mouse is ready, with the way to use it', () => {
  const info = gameMouseInfo(windows('usbip'));
  assert.equal(info?.value, 'Ready (usbip-win2)');
  assert.match(info?.note ?? '', /Lock Pointer/);
  assert.equal(info?.link, undefined);
});

test('without it the row says how to get it', () => {
  const info = gameMouseInfo(windows('none'));
  assert.equal(info?.value, 'Not installed');
  assert.match(info?.note ?? '', /restart its agent/);
  assert.deepEqual(info?.link, { href: USBIP_WIN2_RELEASES, label: 'Download usbip-win2' });
});

test('an older agent does not say: it is to be updated', () => {
  assert.equal(gameMouseInfo(windows())?.value, 'Needs agent 0.7.8 or newer');
  assert.equal(gameMouseInfo(windows('something new'))?.value, 'Needs agent 0.7.8 or newer');
});
