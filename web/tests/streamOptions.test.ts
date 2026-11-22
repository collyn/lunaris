// node --test tests/streamOptions.test.ts
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { shownPictureSize } from '../src/streamOptions.ts';

test('the lines a decoder adds to the picture are left out', () => {
  const picture = { width: 1920, height: 1080 };
  // H.265 coded as 1088 lines, shown whole by Chrome's D3D11 decoder: the
  // padding and the two lines whose colour blends with it are left out.
  assert.deepEqual(shownPictureSize(1920, 1088, picture), { width: 1920, height: 1078 });
  assert.deepEqual(shownPictureSize(1928, 1080, picture), { width: 1918, height: 1080 });
  assert.deepEqual(shownPictureSize(1920, 1080, picture), picture);
  // Not a padding: another size (a joined stream, a new resolution), or no size known.
  assert.deepEqual(shownPictureSize(2560, 1440, picture), { width: 2560, height: 1440 });
  assert.deepEqual(shownPictureSize(1280, 720, picture), { width: 1280, height: 720 });
  assert.deepEqual(shownPictureSize(1920, 1088, null), { width: 1920, height: 1088 });
  assert.deepEqual(shownPictureSize(1920, 1144, { width: 1920, height: 1080 }), { width: 1920, height: 1078 });
  assert.deepEqual(shownPictureSize(1920, 1145, { width: 1920, height: 1080 }), { width: 1920, height: 1145 });
});
