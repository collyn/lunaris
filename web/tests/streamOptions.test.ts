// node --test tests/streamOptions.test.ts
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { pictureClip, pointerReference, shownPictureSize } from '../src/streamOptions.ts';

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

test('where the <video> cannot crop, the whole frame is shown and pointers are on it', () => {
  const picture = { width: 1920, height: 1080 };
  // 1920x1088 shown whole: row 1079 of the frame is the host's last row.
  const ref = pointerReference(1920, 1088, picture, false);
  assert.deepEqual(ref, { width: 4096, height: Math.round((4096 * 1080) / 1088) });
  assert.equal(Math.floor(((4096 * (1079.5 / 1088)) / ref.height) * 1080), 1079);
  assert.deepEqual(pointerReference(1920, 1080, picture, false), { width: 4096, height: 4096 });
  // Another size than a padding: as it is.
  assert.deepEqual(pointerReference(1280, 720, picture, false), { width: 4096, height: 4096 });
  assert.deepEqual(pointerReference(1920, 1088, null, false), { width: 4096, height: 4096 });
});

test('the added lines are clipped off the bottom of the shown frame', () => {
  const picture = { width: 1920, height: 1080 };
  const decoded = { width: 1920, height: 1088 };
  // A box of the frame's shape: the frame fills it, the bottom is cut.
  const phone = { width: 844, height: 478.25 };
  const clip = pictureClip(phone, decoded, picture, 'contain');
  const s = 844 / 1920;
  assert.equal(clip.right, 0);
  assert.ok(Math.abs(clip.bottom - (1088 - 1078) * s) < 0.02, `bottom ${clip.bottom}`);
  // Letterboxed (a tall box): the clip reaches up from the box's bottom to the frame's row 1078.
  const tall = pictureClip({ width: 390, height: 844 }, decoded, picture, 'contain');
  const s2 = 390 / 1920;
  const top = (844 - 1088 * s2) / 2;
  assert.ok(Math.abs(tall.bottom - (844 - (top + 1078 * s2))) < 0.02, `tall ${tall.bottom}`);
  // Stretched: each side on its own scale.
  const fill = pictureClip({ width: 1000, height: 1000 }, decoded, picture, 'fill');
  assert.ok(Math.abs(fill.bottom - (1000 * 10) / 1088) < 0.02);
  // Nothing added, or not known: nothing clipped.
  assert.deepEqual(pictureClip(phone, { width: 1920, height: 1080 }, picture, 'contain'), { right: 0, bottom: 0 });
  assert.deepEqual(pictureClip(phone, decoded, null, 'contain'), { right: 0, bottom: 0 });
  assert.deepEqual(pictureClip({ width: 0, height: 0 }, decoded, picture, 'contain'), { right: 0, bottom: 0 });
});
