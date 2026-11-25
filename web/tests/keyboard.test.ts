// The stream view's keyboard (src/keyboard.ts): the key sent for a code, the
// keys let go when the page loses the keyboard, and the Keyboard Lock asked
// of the browser.
// Run: npm run test:keyboard   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { KEY_COMBOS, KEY_TO_VK, PressedKeys, applyKeyboardLock, combosFor, comboStrokes, keyboardLockFor, tapsUnderMeta, vkOfCode, escapeCanBeKept, secureAddress } from '../src/keyboard.ts';

test('keys are named by where they are', () => {
  assert.equal(vkOfCode('KeyA'), 0x41);
  assert.equal(vkOfCode('Digit1'), 0x31);
  assert.equal(vkOfCode('MetaLeft'), 0x5B);
  assert.equal(vkOfCode('Tab'), 0x09);
  assert.equal(vkOfCode('F4'), 0x73);
  assert.equal(vkOfCode('Delete'), 0x2E);
  assert.equal(vkOfCode('PrintScreen'), 0x2C);
});

test('Shift, Control and Alt go with their side', () => {
  assert.deepEqual(
    ['ShiftLeft', 'ShiftRight', 'ControlLeft', 'ControlRight', 'AltLeft', 'AltRight'].map(vkOfCode),
    [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5],
  );
});

test('keys that used to be dropped are sent', () => {
  assert.equal(vkOfCode('ContextMenu'), 0x5D);
  assert.equal(vkOfCode('NumpadEnter'), 0x0D);
  assert.equal(vkOfCode('IntlBackslash'), 0xE2);
  assert.equal(vkOfCode('F13'), 0x7C);
  assert.equal(vkOfCode('F24'), 0x87);
});

test('keys without an entry are not sent', () => {
  for (const code of ['AudioVolumeUp', 'AudioVolumeMute', 'MediaPlayPause', 'BrowserBack', 'Unidentified', '']) {
    assert.equal(vkOfCode(code), 0, code);
  }
});

test('every code is a key code of one byte', () => {
  for (const [code, vk] of Object.entries(KEY_TO_VK)) {
    assert.ok(Number.isInteger(vk) && vk > 0 && vk <= 0xFF, code);
  }
});

test('held keys are let go, last pressed first', () => {
  const keys = new PressedKeys();
  keys.note(0xA2, true); // Ctrl
  keys.note(0x43, true); // C
  keys.note(0x43, false);
  keys.note(0x5B, true); // Win
  keys.note(0xA2, true); // Ctrl repeating: still held once
  assert.equal(keys.count, 2);
  assert.deepEqual(keys.releaseAll(), [[0xA2, false], [0x5B, false]]);
  assert.equal(keys.count, 0);
  assert.deepEqual(keys.releaseAll(), []);
});

test('Alt+Tab taken by this computer: Alt is let go and tapped once more', () => {
  const keys = new PressedKeys();
  keys.note(0xA4, true);
  assert.deepEqual(keys.releaseAll(), [[0xA4, false], [0xA4, true], [0xA4, false]]);
});

test('a release without a press holds nothing', () => {
  const keys = new PressedKeys();
  keys.note(0x41, false);
  assert.deepEqual(keys.releaseAll(), []);
});

test('macOS: a key pressed under Command is tapped', () => {
  assert.equal(tapsUnderMeta(true, true, 'KeyC'), true);
  assert.equal(tapsUnderMeta(true, true, 'MetaLeft'), false);
  assert.equal(tapsUnderMeta(true, true, 'MetaRight'), false);
  assert.equal(tapsUnderMeta(true, false, 'KeyC'), false);
  assert.equal(tapsUnderMeta(false, true, 'KeyC'), false, 'other systems send the release');
});

test('what is asked of the Keyboard Lock', () => {
  assert.equal(keyboardLockFor(true, false, true), 'all');
  assert.equal(keyboardLockFor(true, true, true), 'all', 'the pointer lock does not narrow it to Esc');
  assert.equal(keyboardLockFor(true, true, false), 'escape', 'setting off: Esc only, as before');
  assert.equal(keyboardLockFor(false, true, true), 'escape', 'no full screen: the browser locks nothing more');
  assert.equal(keyboardLockFor(false, false, true), 'none');
});

test('the Keyboard Lock is asked once, for the right keys', async () => {
  const calls: (string[] | 'all' | 'unlock')[] = [];
  const keyboard = {
    lock: (codes?: string[]) => { calls.push(codes ?? 'all'); return Promise.resolve(); },
    unlock: () => { calls.push('unlock'); },
  };
  assert.equal(applyKeyboardLock(keyboard, 'all'), true);
  assert.equal(applyKeyboardLock(keyboard, 'escape'), true);
  assert.equal(applyKeyboardLock(keyboard, 'none'), true);
  assert.deepEqual(calls, ['all', ['Escape'], 'unlock']);
});

test('a browser without Keyboard Lock, or one that refuses', async () => {
  assert.equal(applyKeyboardLock(undefined, 'all'), false);
  assert.equal(applyKeyboardLock({}, 'all'), false);
  // A refusal is not an error of the page.
  assert.equal(applyKeyboardLock({ lock: () => Promise.reject(new Error('no')) }, 'all'), true);
  await new Promise((r) => setTimeout(r, 0));
});

test('a combination goes down in order and up in the opposite one', () => {
  assert.deepEqual(comboStrokes([0xA2, 0xA0, 0x1B]), [
    [0xA2, true], [0xA0, true], [0x1B, true], [0x1B, false], [0xA0, false], [0xA2, false],
  ]);
  assert.deepEqual(comboStrokes([0x5B]), [[0x5B, true], [0x5B, false]]);
});

test('the combinations offered name keys that are sent', () => {
  const sent = new Set(Object.values(KEY_TO_VK));
  for (const combo of KEY_COMBOS) {
    assert.ok(combo.keys.length > 0, combo.id);
    for (const vk of combo.keys) assert.ok(sent.has(vk), `${combo.id}: ${vk}`);
  }
  assert.equal(new Set(KEY_COMBOS.map((c) => c.id)).size, KEY_COMBOS.length);
});

test('Ctrl+Alt+Del is not offered for a Windows computer', () => {
  const ids = (os: string) => combosFor(os).map((c) => c.id);
  assert.ok(ids('linux').includes('ctrl-alt-del'));
  assert.ok(ids('unknown').includes('ctrl-alt-del'));
  assert.ok(!ids('windows').includes('ctrl-alt-del'));
  assert.ok(!ids('Windows').includes('ctrl-alt-del'));
  assert.ok(ids('windows').includes('alt-tab'));
});

test('Esc is kept only where Keyboard Lock is: a secure page in Chromium', () => {
  const chromium = { lock: async () => {}, unlock: () => {} };
  assert.equal(escapeCanBeKept(chromium, true), true);
  // http://host:8090: Chromium has no navigator.keyboard there.
  assert.equal(escapeCanBeKept(undefined, false), false);
  assert.equal(escapeCanBeKept(chromium, false), false);
  // Safari, Firefox.
  assert.equal(escapeCanBeKept(undefined, true), false);
});

test('the https address of this page, to keep Esc', () => {
  const page = { protocol: 'http:', hostname: '192.168.1.5', pathname: '/', search: '' };
  assert.equal(secureAddress(page, 8443), 'https://192.168.1.5:8443/');
  assert.equal(secureAddress({ ...page, hostname: 'lunaris.example', search: '?a=1' }, 443), 'https://lunaris.example/?a=1');
  assert.equal(secureAddress({ ...page, hostname: '::1' }, 8443), 'https://[::1]:8443/');
  assert.equal(secureAddress(page, null), null);
  assert.equal(secureAddress({ ...page, protocol: 'https:' }, 8443), null);
});

test('the Esc hold ends before Chrome leaves full screen by itself', async () => {
  const { ESCAPE_HOLD_MS } = await import('../src/keyboard.ts');
  // The browser lets go of a Keyboard Lock after 2 s of Esc.
  assert.ok(ESCAPE_HOLD_MS >= 1000 && ESCAPE_HOLD_MS <= 1600);
});
