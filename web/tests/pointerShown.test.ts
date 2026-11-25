// Which pointers the stream view shows (src/pointerShown.ts), case by case.
// Run: npm run test:pointer   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  hostPointerInPicture,
  saysPointerAlwaysInPicture,
  showsHostCursor,
  showsOwnCursor,
  streamCursorCss,
  type PointerView,
} from '../src/pointerShown.ts';

const HOST_CSS = 'url("arrow.png") 4 4, default';

// A session before anything came from the host and before the mouse moved.
const view = (over: Partial<PointerView> = {}): PointerView => ({
  streaming: true,
  hostOs: 'unknown',
  alwaysInPicture: false,
  buttonHeld: false,
  inWindowMoveSize: false,
  foundInPicture: false,
  hostCursorVisible: false,
  hardwareMouse: false,
  trackpad: false,
  hideLocalCursor: true,
  pointerLocked: false,
  hostCssCursor: null,
  hasNativeCursorImage: false,
  ...over,
});

const shown = (v: PointerView) => ({ host: showsHostCursor(v), own: showsOwnCursor(v), css: streamCursorCss(v) });

test('only "always" says the pointer is always in the picture', () => {
  assert.equal(saysPointerAlwaysInPicture({ pointer_in_picture: 'always' }), true);
  // Hosts from before the field, hosts that send null, and anything else.
  assert.equal(saysPointerAlwaysInPicture({}), false);
  assert.equal(saysPointerAlwaysInPicture({ pointer_in_picture: null }), false);
  assert.equal(saysPointerAlwaysInPicture({ pointer_in_picture: true }), false);
  assert.equal(saysPointerAlwaysInPicture({ pointer_in_picture: 'never' }), false);
  assert.equal(saysPointerAlwaysInPicture(null), false);
  // The other two flags do not say it.
  assert.equal(saysPointerAlwaysInPicture({ in_window_move_size: true, pointer_maybe_in_picture: true } as object), false);
});

test('a Linux host: the system draws the host-shaped pointer', () => {
  const v = view({ hostOs: 'linux', hostCursorVisible: true, hardwareMouse: true, hostCssCursor: HOST_CSS, hasNativeCursorImage: true });
  assert.deepEqual(shown(v), { host: false, own: false, css: HOST_CSS });
  // Before the mouse moved the page draws the host's cursor where the host says.
  assert.deepEqual(shown({ ...v, hardwareMouse: false }), { host: true, own: false, css: HOST_CSS });
  // Pointer locked: the page draws its own.
  assert.deepEqual(shown({ ...v, pointerLocked: true }), { host: false, own: true, css: 'none' });
  // "Show local cursor": the system's ordinary pointer, nothing drawn.
  assert.deepEqual(shown({ ...v, hideLocalCursor: false }), { host: false, own: false, css: 'default' });
});

test('a Windows host: the system draws the host-shaped pointer too (no lag)', () => {
  const v = view({ hostOs: 'windows', hostCursorVisible: true, hardwareMouse: true, hostCssCursor: HOST_CSS, hasNativeCursorImage: true });
  assert.deepEqual(shown(v), { host: false, own: false, css: HOST_CSS });
  // "Show local cursor": the system's ordinary pointer, nothing drawn.
  assert.deepEqual(shown({ ...v, hideLocalCursor: false }), { host: false, own: false, css: 'default' });
  // An image the system cannot use (too large): the page draws it.
  assert.deepEqual(shown({ ...v, hostCssCursor: null }), { host: false, own: true, css: 'none' });
  assert.deepEqual(shown({ ...v, hostCssCursor: null, hideLocalCursor: false }), { host: false, own: false, css: 'default' });
  // Pointer locked: the page draws its own.
  assert.deepEqual(shown({ ...v, pointerLocked: true }), { host: false, own: true, css: 'none' });
  assert.deepEqual(shown({ ...v, pointerLocked: true, hideLocalCursor: false }), { host: false, own: true, css: 'none' });
  // A window moved with the button held: the pointer is in the picture, none over it.
  assert.deepEqual(shown({ ...v, inWindowMoveSize: true, buttonHeld: true }), { host: false, own: false, css: 'none' });
});

test('no cursor message has ever come: the page draws its own under the mouse', () => {
  // Nothing before the mouse moves; the system pointer is hidden.
  assert.deepEqual(shown(view()), { host: false, own: false, css: 'none' });
  assert.deepEqual(shown(view({ hardwareMouse: true })), { host: false, own: true, css: 'none' });
  assert.deepEqual(shown(view({ hardwareMouse: true, hideLocalCursor: false })), { host: false, own: false, css: 'default' });
});

test('a window moved on a Windows host: the drawn pointers hide while a button is held', () => {
  const v = view({ hostOs: 'windows', hostCursorVisible: true, hardwareMouse: true, hasNativeCursorImage: true, inWindowMoveSize: true });
  assert.equal(hostPointerInPicture(v), false, 'no button held');
  assert.equal(showsOwnCursor(v), true);
  const held = { ...v, buttonHeld: true };
  assert.equal(hostPointerInPicture(held), true);
  assert.deepEqual(shown(held), { host: false, own: false, css: 'none' });
  // Found in the picture after the host's "maybe": the same.
  const found = { ...v, inWindowMoveSize: false, foundInPicture: true, buttonHeld: true };
  assert.deepEqual(shown(found), { host: false, own: false, css: 'none' });
  // That flag counts for a Windows host only, as before.
  for (const hostOs of ['linux', 'macos', 'unknown']) {
    assert.equal(hostPointerInPicture({ ...held, hostOs }), false, hostOs);
    assert.equal(hostPointerInPicture({ ...found, hostOs }), false, hostOs);
  }
});

test('"maybe" alone hides nothing', () => {
  // The page knows "maybe" only through what it found: nothing found, nothing hidden.
  const v = view({ hostOs: 'windows', hostCursorVisible: true, hardwareMouse: true, hasNativeCursorImage: true, buttonHeld: true });
  assert.equal(hostPointerInPicture(v), false);
  assert.equal(showsOwnCursor(v), true);
});

test('always in the picture: no pointer at all, in every mode', () => {
  for (const hostOs of ['unknown', 'windows', 'linux', 'macos']) {
    for (const hideLocalCursor of [true, false]) {
      for (const pointerLocked of [false, true]) {
        for (const hardwareMouse of [false, true]) {
          for (const trackpad of [false, true]) {
            for (const buttonHeld of [false, true]) {
              for (const hostCssCursor of [null, HOST_CSS]) {
                const v = view({
                  alwaysInPicture: true, hostOs, hideLocalCursor, pointerLocked, hardwareMouse, trackpad, buttonHeld, hostCssCursor,
                  // Even if a cursor message says "visible" and an image came.
                  hostCursorVisible: true, hasNativeCursorImage: hostCssCursor !== null,
                });
                assert.deepEqual(shown(v), { host: false, own: false, css: 'none' }, JSON.stringify(v));
              }
            }
          }
        }
      }
    }
  }
});

test('always in the picture: the ordinary pointer outside a session', () => {
  // Not streaming (connecting, ended): the picture's place has the ordinary pointer.
  assert.equal(streamCursorCss(view({ alwaysInPicture: true, streaming: false })), 'default');
});

test('a phone in touchpad mode draws its own cursor, unless the pointer is always in the picture', () => {
  const v = view({ trackpad: true, hostCursorVisible: true });
  assert.deepEqual(shown(v), { host: false, own: true, css: 'none' });
  assert.deepEqual(shown({ ...v, alwaysInPicture: true }), { host: false, own: false, css: 'none' });
});

test('with the game mouse on and the pointer locked, one pointer: where the host says', () => {
  const v = view({ hostOs: 'windows', hardwareMouse: true, pointerLocked: true, hasNativeCursorImage: true, hostCursorVisible: true, gameMouse: true });
  // On the desktop: the host's cursor, drawn where the host puts it.
  assert.equal(hostPointerInPicture(v), false);
  assert.equal(showsHostCursor(v), true);
  assert.equal(showsOwnCursor(v), false);
  assert.equal(streamCursorCss(v), 'none');
  // In a game that hides the system's cursor: the game's, in the picture.
  assert.equal(showsHostCursor({ ...v, hostCursorVisible: false }), false);
  assert.equal(showsOwnCursor({ ...v, hostCursorVisible: false }), false);
  // Without it the page draws its own cursor, as before.
  assert.equal(showsOwnCursor({ ...v, gameMouse: false }), true);
});

test('a late host_cursor message is stale; a new session is not', async () => {
  const { isStaleCursorSeq } = await import('../src/pointerShown.ts');
  assert.equal(isStaleCursorSeq(null, 5), false);
  assert.equal(isStaleCursorSeq(10, 11), false);
  assert.equal(isStaleCursorSeq(10, 9), true);
  assert.equal(isStaleCursorSeq(10, 10), true);
  assert.equal(isStaleCursorSeq(50_000, 1), false);
  assert.equal(isStaleCursorSeq(10, undefined), false);
});
