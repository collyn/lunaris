// The matcher of src/pointerInPicture.ts on synthetic pictures.
// Run: npm run test:pointer   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  CHECK_INTERVAL_MS,
  PointerInPictureDrag,
  looksForPointerInPicture,
  SentPositions,
  buildTemplates,
  checkOnce,
  matchRegion,
  MAX_READ_PIXELS,
  searchBox,
  searchGroups,
  type CursorImage,
  type LumaRegion,
  type Point,
} from '../src/pointerInPicture.ts';

// The test arrow of the spec: 12 x 19, hot spot 0,0. X black, . white.
const ARROW = [
  'X',
  'XX',
  'X.X',
  'X..X',
  'X...X',
  'X....X',
  'X.....X',
  'X......X',
  'X.......X',
  'X........X',
  'X.........X',
  'X......XXXXX',
  'X...X..X',
  'X..X X..X',
  'X.X  X..X',
  'XX    X..X',
  'X     X..X',
  '       XX',
];

const cursorFromRows = (rows: string[], width: number, height: number): CursorImage => {
  const rgba = new Uint8Array(width * height * 4);
  rows.forEach((row, y) => {
    for (let x = 0; x < row.length; x += 1) {
      const c = row[x];
      if (c === ' ') continue;
      const v = c === 'X' ? 0 : 255;
      rgba.set([v, v, v, 255], (y * width + x) * 4);
    }
  });
  return { width, height, hotspotX: 0, hotspotY: 0, rgba };
};

const arrow = cursorFromRows(ARROW, 12, 19);

const W = 320;
const H = 240;

const rng = (seed: number) => () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};

const flat = (luma: number): Uint8Array => new Uint8Array(W * H).fill(luma);
const noisy = (seed: number): Uint8Array => {
  const r = rng(seed);
  return Uint8Array.from({ length: W * H }, () => Math.floor(r() * 256));
};

const cursorLuma = (c: CursorImage, x: number, y: number): [number, number] => {
  if (x < 0 || y < 0 || x >= c.width || y >= c.height) return [0, 0];
  const i = (y * c.width + x) * 4;
  return [0.299 * c.rgba[i] + 0.587 * c.rgba[i + 1] + 0.114 * c.rgba[i + 2], c.rgba[i + 3] / 255];
};

/** Draws the cursor with alpha, its hot spot at (px, py), `scale` times its
 *  size: nearest neighbour (source pixel floor((x + 0.5) / scale)), or
 *  bilinear with `smooth`. */
const draw = (pic: Uint8Array, c: CursorImage, px: number, py: number, scale = 1, smooth = false): void => {
  const w = Math.round(c.width * scale);
  const h = Math.round(c.height * scale);
  for (let y = 0; y < h; y += 1) {
    for (let x = 0; x < w; x += 1) {
      let l: number;
      let a = 0;
      if (!smooth) {
        [l, a] = cursorLuma(c, Math.floor((x + 0.5) / scale), Math.floor((y + 0.5) / scale));
      } else {
        const sx = (x + 0.5) / scale - 0.5;
        const sy = (y + 0.5) / scale - 0.5;
        const x0 = Math.floor(sx);
        const y0 = Math.floor(sy);
        const fx = sx - x0;
        const fy = sy - y0;
        let la = 0;
        for (const [ix, iy, wgt] of [[x0, y0, (1 - fx) * (1 - fy)], [x0 + 1, y0, fx * (1 - fy)], [x0, y0 + 1, (1 - fx) * fy], [x0 + 1, y0 + 1, fx * fy]]) {
          const [sl, sa] = cursorLuma(c, ix, iy);
          la += sl * sa * wgt;
          a += sa * wgt;
        }
        l = a > 0 ? la / a : 0;
      }
      const tx = px - Math.round(c.hotspotX * scale) + x;
      const ty = py - Math.round(c.hotspotY * scale) + y;
      if (a <= 0 || tx < 0 || ty < 0 || tx >= W || ty >= H) continue;
      pic[ty * W + tx] = Math.round(l * a + pic[ty * W + tx] * (1 - a));
    }
  }
};

const boxBlur3 = (pic: Uint8Array): Uint8Array => {
  const out = new Uint8Array(pic.length);
  for (let y = 0; y < H; y += 1) {
    for (let x = 0; x < W; x += 1) {
      let sum = 0;
      let n = 0;
      for (let dy = -1; dy <= 1; dy += 1) {
        for (let dx = -1; dx <= 1; dx += 1) {
          const xx = x + dx;
          const yy = y + dy;
          if (xx < 0 || yy < 0 || xx >= W || yy >= H) continue;
          sum += pic[yy * W + xx];
          n += 1;
        }
      }
      out[y * W + x] = Math.round(sum / n);
    }
  }
  return out;
};

const addNoise = (pic: Uint8Array, amount: number, seed: number): Uint8Array => {
  const r = rng(seed);
  return pic.map((v) => Math.max(0, Math.min(255, v + Math.round((r() * 2 - 1) * amount))));
};

const whole = (pic: Uint8Array): LumaRegion => ({ x: 0, y: 0, width: W, height: H, data: pic });

/** The region the player would read back for these positions. */
const cut = (pic: Uint8Array, positions: Point[], cursor: CursorImage): LumaRegion => {
  const box = searchBox(positions, buildTemplates(cursor), W, H);
  assert.ok(box);
  const data = new Uint8Array(box.width * box.height);
  for (let y = 0; y < box.height; y += 1) {
    data.set(pic.subarray((box.y + y) * W + box.x, (box.y + y) * W + box.x + box.width), y * box.width);
  }
  return { ...box, data };
};

const find = (pic: Uint8Array, positions: Point[], cursor: CursorImage = arrow): number => {
  const templates = buildTemplates(cursor);
  const a = matchRegion(whole(pic), templates, positions);
  if (templates.length > 0) {
    // The cut-out region must give the same answer as the whole picture,
    // and so must the regions the player reads back, one after the other.
    assert.equal(matchRegion(cut(pic, positions, cursor), templates, positions), a);
    let b = 0;
    for (const group of searchGroups(positions, templates, W, H)) {
      b = matchRegion(cut(pic, group.positions, cursor), templates, group.positions);
      if (b > 0) break;
    }
    assert.equal(b, a);
  }
  return a;
};

const AT = { x: 150, y: 100 };
const NEAR = [{ x: 151, y: 99 }, { x: 140, y: 95 }, { x: 120, y: 80 }];

test('1. arrow on a noisy background: found', () => {
  const pic = noisy(1);
  draw(pic, arrow, AT.x, AT.y);
  assert.equal(find(pic, NEAR), 1.0);
  assert.equal(find(pic, [AT]), 1.0);
});

test('2. arrow on a bright (235) and on a dark (20) background: found', () => {
  for (const bg of [235, 20]) {
    const pic = flat(bg);
    draw(pic, arrow, AT.x, AT.y);
    assert.equal(find(pic, NEAR), 1.0, `background ${bg}`);
  }
});

const blurCases = (): [string, Uint8Array][] => {
  const cases: [string, Uint8Array][] = [['bright', flat(235)], ['dark', flat(20)], ['grey', flat(128)], ['noisy', noisy(3)]];
  for (const [, pic] of cases) draw(pic, arrow, AT.x, AT.y);
  return cases;
};

// The spec's case 3 as written. The matcher of the spec (mean absolute luma
// difference <= 26 over all ink pixels) does not stand a full 3x3 box blur of
// this arrow: its outline is one pixel wide, the blur moves those pixels 150
// and more away from black, and the best placement scores 66 to 93. No limit
// lets that through and still keeps "no arrow" (108 and up) safely out. Kept
// as a todo so it is seen; what the matcher does stand is in the next test.
test('3. arrow blurred (3x3 box) with +-6 luma noise: found', { todo: 'not reachable with the numbers of the spec' }, () => {
  for (const [name, bg] of blurCases()) {
    assert.equal(find(addNoise(boxBlur3(bg), 6, 33), NEAR), 1.0, name);
  }
});

test('3b. arrow softened (a fifth of the way to a 3x3 box blur) with +-6 luma noise: found', () => {
  for (const [name, bg] of blurCases()) {
    const blurred = boxBlur3(bg);
    const soft = bg.map((v, i) => Math.round(v * 0.8 + blurred[i] * 0.2));
    assert.equal(find(addNoise(soft, 6, 33), NEAR), 1.0, name);
    assert.equal(find(addNoise(bg, 6, 34), NEAR), 1.0, `${name}, noise only`);
  }
});

test('4. no arrow in the picture: not found', () => {
  for (const pic of [noisy(4), flat(235), flat(20), flat(128), addNoise(boxBlur3(noisy(5)), 6, 6)]) {
    assert.equal(find(pic, NEAR), 0);
    assert.equal(find(pic, [AT]), 0);
  }
});

test('5. an arrow 60 px away from every position: not found', () => {
  for (const make of [() => noisy(7), () => flat(235), () => flat(20)]) {
    const pic = make();
    draw(pic, arrow, AT.x + 60, AT.y);
    assert.equal(find(pic, [AT, { x: AT.x, y: AT.y + 60 }, { x: AT.x + 120, y: AT.y }, { x: AT.x + 60, y: AT.y - 60 }]), 0);
  }
});

test('6. arrow drawn at 1.5x, template 1x: found at scale 1.5', () => {
  for (const make of [() => noisy(8), () => flat(235), () => flat(20)]) {
    const pic = make();
    draw(pic, arrow, AT.x, AT.y, 1.5);
    assert.equal(find(pic, NEAR), 1.5);
  }
  // Once a scale has matched in a drag only that scale is tried.
  const drag = new PointerInPictureDrag();
  drag.update(true, true);
  const templates = buildTemplates(arrow);
  const big = flat(235);
  draw(big, arrow, AT.x, AT.y, 1.5);
  checkOnce(drag, templates, whole(big), [AT], 0);
  assert.equal(drag.scale, 1.5);
  assert.deepEqual(drag.templatesToTry(templates).map((t) => t.scale), [1.5]);
  const small = flat(235);
  draw(small, arrow, AT.x, AT.y, 1.0);
  assert.equal(find(small, [AT]), 1.0);
  checkOnce(drag, templates, whole(small), [AT], 100);
  assert.equal(drag.run, 0, 'the 1x arrow is not looked for after 1.5 matched');
});

// A limit of the spec's matcher: a cursor the host enlarges smoothly (not by
// repeating pixels) scores 37 to 49 at its scale, above the limit of 26.
test('6b. arrow drawn at 1.5x with smooth scaling: found', { todo: 'not reachable with the numbers of the spec' }, () => {
  const pic = flat(235);
  draw(pic, arrow, AT.x, AT.y, 1.5, true);
  assert.equal(find(pic, NEAR), 1.5);
});

test('7. fewer than 40 ink pixels, and an all-white cursor: not found', () => {
  const small = cursorFromRows(ARROW.slice(0, 7), 12, 19); // 27 ink pixels
  assert.deepEqual(buildTemplates(small), []);
  const white: CursorImage = { ...arrow, rgba: arrow.rgba.map((v, i) => (i % 4 === 3 ? v : 255)) };
  assert.deepEqual(buildTemplates(white), []);
  const black: CursorImage = { ...arrow, rgba: arrow.rgba.map((v, i) => (i % 4 === 3 ? v : 0)) };
  assert.deepEqual(buildTemplates(black), []);
  for (const c of [small, white, black]) {
    for (const bg of [235, 20]) {
      const pic = flat(bg);
      draw(pic, c, AT.x, AT.y);
      assert.equal(find(pic, [AT], c), 0);
    }
  }
});

test('8. decision over a drag, and the log line', () => {
  const templates = buildTemplates(arrow);
  const withArrow = flat(235);
  draw(withArrow, arrow, AT.x, AT.y);
  const without = flat(235);

  const drag = new PointerInPictureDrag();
  assert.equal(drag.update(true, false), null);
  assert.equal(drag.active, false, 'no drag while the host does not say "maybe"');
  assert.equal(drag.update(false, true), null);
  assert.equal(drag.active, false, 'no drag while the button is up');
  assert.equal(drag.update(true, true), null);
  assert.equal(drag.active, true);

  // found, found, not found, found, found, found
  const seen = [true, true, false, true, true, true];
  const decided: boolean[] = [];
  let now = 1000;
  for (const s of seen) {
    assert.equal(drag.wantsCheck(now), true);
    checkOnce(drag, templates, whole(s ? withArrow : without), [AT], now);
    assert.equal(drag.wantsCheck(now + CHECK_INTERVAL_MS - 1), false, 'at most one check per 30 ms');
    decided.push(drag.inPicture);
    now += 33;
  }
  assert.deepEqual(decided, [false, false, false, false, false, true]);
  assert.equal(drag.wantsCheck(now + 1000), false, 'no more checks once decided');
  assert.equal(checkOnce(drag, templates, whole(without), [AT], now + 1000), false);
  assert.equal(drag.inPicture, true, 'stays for the rest of the drag');
  assert.equal(drag.update(true, true), null);
  assert.equal(drag.update(false, true), 'pointer looked for in the picture: found after 6 checks');
  assert.equal(drag.inPicture, false, 'over when the button is released');

  // A drag in which it is never found; ended by "maybe" going false.
  assert.equal(drag.update(true, true), null);
  for (let i = 0; i < 4; i += 1) {
    checkOnce(drag, templates, whole(without), [AT], 5000 + i * 33);
    assert.equal(drag.inPicture, false);
  }
  assert.equal(drag.update(true, false), 'pointer looked for in the picture: not found (4 checks)');
  assert.equal(drag.update(false, false), null, 'one line per drag');

  // A drag that ends before any picture was looked at.
  drag.update(true, true);
  assert.equal(drag.update(false, false), 'pointer looked for in the picture: not found (0 checks)');
});

test('positions: newest first, last 600 ms, no repeats, at most 16', () => {
  const sent = new SentPositions();
  assert.deepEqual(sent.recent(0), []);
  sent.record(10, 10, 0);
  sent.record(50.4, 10, 100);
  sent.record(50, 10, 110); // a repeat
  sent.record(52, 12, 120); // within the search radius of the one after it
  sent.record(90, 10, 500);
  assert.deepEqual(sent.recent(550), [{ x: 90, y: 10 }, { x: 52, y: 12 }, { x: 10, y: 10 }]);
  assert.deepEqual(sent.recent(650), [{ x: 90, y: 10 }, { x: 52, y: 12 }, { x: 50, y: 10 }].slice(0, 2));
  // Long after the last one: the pointer still is where it was last sent.
  assert.deepEqual(sent.recent(60_000), [{ x: 90, y: 10 }]);
  for (let i = 0; i < 400; i += 1) sent.record(100 + i * 5, 20, 1000 + i);
  const recent = sent.recent(1400);
  assert.equal(recent.length, 16);
  assert.deepEqual(recent[0], { x: 100 + 399 * 5, y: 20 });
  assert.deepEqual(recent[15], { x: 100 + 384 * 5, y: 20 });
});

test('a placement with ink outside the picture is skipped', () => {
  const pic = flat(235);
  draw(pic, arrow, W - 6, 100); // half of the arrow is cut off by the edge
  assert.equal(find(pic, [{ x: W - 6, y: 100 }]), 0);
  const corner = flat(235);
  draw(corner, arrow, 0, 0);
  assert.equal(find(corner, [{ x: 0, y: 0 }]), 1.0);
});

test('a fast drag is read back in small pieces, with the same answer', () => {
  const templates = buildTemplates(arrow);
  const path = Array.from({ length: 16 }, (_, i) => ({ x: 300 - i * 19, y: 220 - i * 13 }));
  const all = searchBox(path, templates, W, H);
  assert.ok(all && all.width * all.height > MAX_READ_PIXELS);
  const groups = searchGroups(path, templates, W, H);
  assert.ok(groups.length > 1);
  assert.deepEqual(groups.flatMap((g) => g.positions), path, 'every position once, in order');
  for (const g of groups) {
    assert.ok(g.positions.length === 1 || g.box.width * g.box.height <= MAX_READ_PIXELS);
    assert.deepEqual(g.box, searchBox(g.positions, templates, W, H));
  }
  for (const at of [path[0], path[9], path[15]]) {
    const pic = noisy(11);
    draw(pic, arrow, at.x + 2, at.y - 1);
    assert.equal(find(pic, path), 1.0);
  }
  assert.equal(find(noisy(12), path), 0);
  // A slow drag stays one piece.
  assert.equal(searchGroups(NEAR, templates, W, H).length, 1);
});

test('every drag on a Windows host looks; elsewhere only when the host says maybe', () => {
  // Apps that move their own windows: Windows draws the pointer into the
  // picture and the host cannot say so.
  assert.equal(looksForPointerInPicture(false, 'windows'), true);
  assert.equal(looksForPointerInPicture(false, 'linux'), false);
  assert.equal(looksForPointerInPicture(false, 'unknown'), false);
  assert.equal(looksForPointerInPicture(true, 'unknown'), true);
});
