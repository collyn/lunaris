// Moves of a locked pointer (src/relativePointer.ts).
// Run: npm run test:pointer   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_STEP, relativeMovePacket, takeRelativeStep } from '../src/relativePointer.ts';

test('whole pixels are sent and fractions are kept for the next send', () => {
  assert.deepEqual(takeRelativeStep(3.75, -2.5), { dx: 3, dy: -2, restX: 0.75, restY: -0.5 });
  const slow = takeRelativeStep(0.4, -0.4);
  assert.equal(slow.dx, 0);
  assert.equal(slow.dy, 0);
  assert.ok(Object.is(slow.dx, 0) && Object.is(slow.dy, 0), 'no -0 on the wire');
  // Three slow reports of 0.4 px add up to one pixel instead of being lost.
  let rest = 0;
  let sent = 0;
  for (let i = 0; i < 3; i++) {
    const step = takeRelativeStep(rest + 0.4, 0);
    sent += step.dx;
    rest = step.restX;
  }
  assert.equal(sent, 1);
});

test('a move beyond one packet is sent over several', () => {
  const step = takeRelativeStep(40000, -40000);
  assert.equal(step.dx, MAX_STEP);
  assert.equal(step.dy, -MAX_STEP);
  assert.equal(step.restX, 40000 - MAX_STEP);
  assert.equal(step.restY, -40000 + MAX_STEP);
});

test('the packet is what the agent reads as a relative move (type 0)', () => {
  const view = new DataView(relativeMovePacket(-5, 300, 0x01020304));
  assert.equal(view.byteLength, 9);
  assert.equal(view.getUint8(0), 0);
  assert.equal(view.getInt16(1, false), -5);
  assert.equal(view.getInt16(3, false), 300);
  assert.equal(view.getUint32(5, false), 0x01020304);
  // The sequence number wraps like the other mouse packets (>>> 0).
  assert.equal(new DataView(relativeMovePacket(0, 0, -1)).getUint32(5, false), 0xffffffff);
});
