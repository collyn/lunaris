// node --test tests/streamOptions.test.ts
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { pointerReference, shownPictureSize } from '../src/streamOptions.ts';

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

test('a pointer on a cropped picture is told on the scale of the whole picture', () => {
  const picture = { width: 1920, height: 1080 };
  // Nothing cropped: the usual 4096.
  assert.deepEqual(pointerReference(1920, 1080, picture), { width: 4096, height: 4096 });
  assert.deepEqual(pointerReference(1920, 1088, null), { width: 4096, height: 4096 });
  // 1920x1088 decoded, 1920x1078 shown: the bottom row of what is shown is
  // row 1077 of the host's 1080, not its last one.
  const ref = pointerReference(1920, 1088, picture);
  assert.deepEqual(ref, { width: 4096, height: Math.round((4096 * 1080) / 1078) });
  const shownBottom = 4096 * (1077.5 / 1078); // the middle of the last shown row, as the web sends it
  assert.equal(Math.floor((shownBottom / ref.height) * 1080), 1077);
});
