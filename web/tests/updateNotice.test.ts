// The "a newer Lunaris is out" notice for administrators (src/updateNotice.ts).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseServerInfo } from '../src/serverInfo.ts';
import { updateAvailable, updateToShow } from '../src/updateNotice.ts';

const info = (update: Record<string, unknown> | undefined, version = '0.7.1') =>
  parseServerInfo({ version, ...(update ? { update } : {}) });

test('a newer release is shown with the running version and its notes', () => {
  const i = info({ checks: true, latest: 'v0.7.2', url: 'https://example.test/r', available: true });
  assert.deepEqual(updateToShow(i, null), { latest: '0.7.2', current: '0.7.1', url: 'https://example.test/r' });
  assert.equal(updateAvailable(i), true);
});

test('nothing without a newer release, a check, or for non-administrators', () => {
  assert.equal(updateToShow(info({ checks: true, latest: '0.7.1', available: false }), null), null);
  assert.equal(updateToShow(info({ checks: false, latest: '0.7.2', available: true }), null), null);
  assert.equal(updateToShow(info({ checks: true, available: true }), null), null);
  // Non-administrators get no `update` at all.
  assert.equal(updateToShow(info(undefined), null), null);
  assert.equal(updateToShow(null, null), null);
  assert.equal(updateAvailable(info(undefined)), false);
});

test('a closed notice stays closed for that release only', () => {
  const i = info({ checks: true, latest: '0.7.2', available: true });
  assert.equal(updateToShow(i, '0.7.2'), null);
  assert.equal(updateToShow(i, 'v0.7.2'), null);
  // The tab keeps its dot.
  assert.equal(updateAvailable(i), true);
  // A later release shows again.
  assert.notEqual(updateToShow(info({ checks: true, latest: '0.7.3', available: true }), '0.7.2'), null);
});
