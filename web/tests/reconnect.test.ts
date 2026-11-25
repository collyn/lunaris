import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_RECONNECTS, reconnectDelay, signalingCloseLosesSession } from '../src/reconnect.ts';

test('three tries, each waiting longer', () => {
  assert.equal(MAX_RECONNECTS, 3);
  assert.deepEqual([0, 1, 2].map(reconnectDelay), [3000, 8000, 15000]);
  assert.equal(reconnectDelay(3), null);
  assert.equal(reconnectDelay(-1), null);
});

test('the signaling connection closing loses the session only without a picture', () => {
  assert.equal(signalingCloseLosesSession('connected'), false);
  for (const s of ['new', 'connecting', 'disconnected', 'failed', 'closed', null, undefined] as const) {
    assert.equal(signalingCloseLosesSession(s), true, String(s));
  }
});
