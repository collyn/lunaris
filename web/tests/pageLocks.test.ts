// Pointer lock and full screen where the browser lacks them (src/pageLocks.ts).
// Run: npm run test:page-locks   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { canFullscreen, exitFullscreen, exitPointerLock, requestFullscreen } from '../src/pageLocks.ts';

// Safari on an iPhone: no Pointer Lock, no element full screen.
const iphone = () => ({ pointerLockElement: undefined, fullscreenElement: undefined, fullscreenEnabled: undefined }) as any;

test('on an iPhone nothing throws and full screen is not offered', async () => {
  assert.doesNotThrow(() => exitPointerLock(iphone()));
  assert.equal(canFullscreen(iphone()), false);
  await exitFullscreen(iphone());
  await assert.rejects(requestFullscreen({} as Element));
});

test('a locked pointer and full screen are left where the browser has them', async () => {
  const calls: string[] = [];
  const doc = {
    pointerLockElement: {},
    fullscreenElement: {},
    fullscreenEnabled: true,
    exitPointerLock: () => { calls.push('pointer'); },
    exitFullscreen: async () => { calls.push('fullscreen'); },
  } as any;
  exitPointerLock(doc);
  await exitFullscreen(doc);
  assert.equal(canFullscreen(doc), true);
  assert.deepEqual(calls, ['pointer', 'fullscreen']);
});

test('nothing is left when nothing is held', async () => {
  const calls: string[] = [];
  const doc = {
    pointerLockElement: null,
    fullscreenElement: null,
    fullscreenEnabled: true,
    exitPointerLock: () => { calls.push('pointer'); },
    exitFullscreen: async () => { calls.push('fullscreen'); },
  } as any;
  exitPointerLock(doc);
  await exitFullscreen(doc);
  assert.deepEqual(calls, []);
});
