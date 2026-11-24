// Finding the host's pointer in the decoded picture.
//
// A computer shown through a connector cannot say when its pointer is drawn
// into its picture (on Windows: while a window is moved). The host only says
// "the pointer may be in the picture" (`pointer_maybe_in_picture` in the
// `host_cursor` messages); the viewer looks for the host's cursor image in
// the picture near the positions it sent, and hides its own pointer only
// once it has seen it there on three checks in a row.
//
// The desktop client has the same matcher (same steps, same numbers): a
// number changed here must be changed there too.
//
// Everything down to `PictureReader` is free of the DOM (tests:
// tests/pointerInPicture.test.ts, `npm run test:pointer`).

/** A cursor pixel belongs to the template when its alpha is at least this. */
export const INK_ALPHA = 128;
/** Fewer ink pixels than this: the cursor cannot be told from a background. */
export const MIN_INK_PIXELS = 40;
/** Smaller spread of luma (max - min) over the ink pixels: the same. */
export const MIN_LUMA_SPREAD = 96;
/** Sizes the host may draw the cursor at, tried in this order. */
export const SCALES: readonly number[] = [1.0, 1.25, 1.5, 1.75, 2.0];
/** The template is tried this many pixels around each position, each way. */
export const SEARCH_RADIUS = 3;
/** A placement matches when the mean absolute luma difference is at most this. */
export const MAX_SCORE = 26;
/** Checks in a row that must find the pointer. */
export const CHECKS_NEEDED = 3;
/** Positions sent longer ago than this are not looked at. */
export const POSITION_WINDOW_MS = 600;
/** At most this many positions per check. */
export const MAX_POSITIONS = 16;
/** At most one check per this many milliseconds. */
export const CHECK_INTERVAL_MS = 30;

export const LOG_PREFIX = 'pointer looked for in the picture: ';

export type CursorImage = {
  width: number;
  height: number;
  hotspotX: number;
  hotspotY: number;
  /** RGBA, 8 bits, straight alpha, `width * height * 4` bytes. */
  rgba: Uint8Array | Uint8ClampedArray;
};

/** The cursor at one scale: its ink pixels and their luma. */
export type Template = {
  scale: number;
  width: number;
  height: number;
  hotX: number;
  hotY: number;
  count: number;
  /** Ink pixel offsets from the template's top left. */
  dx: Int16Array;
  dy: Int16Array;
  luma: Uint8Array;
  /** The box of the ink pixels (inclusive). */
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
};

/** A rectangle of the picture's luma; (x, y) is its top left in stream
 *  pixels. Everything outside it counts as outside the picture. */
export type LumaRegion = {
  x: number;
  y: number;
  width: number;
  height: number;
  data: Uint8Array;
};

export type Point = { x: number; y: number };
export type Box = { x: number; y: number; width: number; height: number };

export const lumaOf = (r: number, g: number, b: number): number =>
  Math.round(0.299 * r + 0.587 * g + 0.114 * b);

/** The cursor resampled by nearest neighbour to `scale`, or null when it has
 *  no ink at all. Source pixel of target pixel x: floor((x + 0.5) * w / W). */
export const buildTemplate = (cursor: CursorImage, scale: number): Template | null => {
  const w = cursor.width;
  const h = cursor.height;
  const W = Math.round(w * scale);
  const H = Math.round(h * scale);
  if (w <= 0 || h <= 0 || W <= 0 || H <= 0 || cursor.rgba.length < w * h * 4) return null;
  const dx: number[] = [];
  const dy: number[] = [];
  const luma: number[] = [];
  let minX = W, minY = H, maxX = -1, maxY = -1;
  for (let y = 0; y < H; y += 1) {
    const sy = Math.min(h - 1, Math.floor(((y + 0.5) * h) / H));
    for (let x = 0; x < W; x += 1) {
      const sx = Math.min(w - 1, Math.floor(((x + 0.5) * w) / W));
      const i = (sy * w + sx) * 4;
      if (cursor.rgba[i + 3] < INK_ALPHA) continue;
      dx.push(x);
      dy.push(y);
      luma.push(lumaOf(cursor.rgba[i], cursor.rgba[i + 1], cursor.rgba[i + 2]));
      if (x < minX) minX = x;
      if (x > maxX) maxX = x;
      if (y < minY) minY = y;
      if (y > maxY) maxY = y;
    }
  }
  const count = dx.length;
  if (count === 0) return null;
  // The ink pixels are kept in a scattered order (every `step`-th, wrapping
  // round), so that a placement that does not match is given up after a few
  // pixels from all over the cursor. The score is a sum: its order changes
  // nothing in the answer.
  let step = Math.max(1, Math.round(count * 0.618));
  const gcd = (a: number, b: number): number => (b === 0 ? a : gcd(b, a % b));
  while (gcd(step, count) !== 1) step += 1;
  const tdx = new Int16Array(count);
  const tdy = new Int16Array(count);
  const tl = new Uint8Array(count);
  for (let i = 0, j = 0; i < count; i += 1, j = (j + step) % count) {
    tdx[i] = dx[j];
    tdy[i] = dy[j];
    tl[i] = luma[j];
  }
  return {
    scale,
    width: W,
    height: H,
    hotX: Math.round(cursor.hotspotX * scale),
    hotY: Math.round(cursor.hotspotY * scale),
    count,
    dx: tdx,
    dy: tdy,
    luma: tl,
    minX, minY, maxX, maxY,
  };
};

/** The templates of a cursor at every scale, in the order tried. Empty when
 *  the cursor cannot be told from a background (too little ink, or all of
 *  one brightness): the answer is then always "not found". */
export const buildTemplates = (cursor: CursorImage): Template[] => {
  const base = buildTemplate(cursor, 1.0);
  if (!base || base.count < MIN_INK_PIXELS) return [];
  let lo = 255;
  let hi = 0;
  for (let i = 0; i < base.count; i += 1) {
    const v = base.luma[i];
    if (v < lo) lo = v;
    if (v > hi) hi = v;
  }
  if (hi - lo < MIN_LUMA_SPREAD) return [];
  const out: Template[] = [];
  for (const scale of SCALES) {
    const t = scale === 1.0 ? base : buildTemplate(cursor, scale);
    if (t) out.push(t);
  }
  return out;
};

/** Luma of an RGBA buffer (`width * height * 4` bytes), into `out` when it
 *  is large enough. */
export const lumaFromRgba = (
  rgba: Uint8Array | Uint8ClampedArray,
  width: number,
  height: number,
  out?: Uint8Array,
): Uint8Array => {
  const n = width * height;
  const data = out && out.length >= n ? out : new Uint8Array(n);
  for (let i = 0, j = 0; i < n; i += 1, j += 4) {
    data[i] = (0.299 * rgba[j] + 0.587 * rgba[j + 1] + 0.114 * rgba[j + 2] + 0.5) | 0;
  }
  return data;
};

/** The part of the picture one check needs: every placement of every
 *  template around every position, cut to the picture. Null when nothing of
 *  it is inside the picture. */
export const searchBox = (
  positions: readonly Point[],
  templates: readonly Template[],
  pictureWidth: number,
  pictureHeight: number,
): Box | null => {
  if (positions.length === 0 || templates.length === 0) return null;
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const p of positions) {
    for (const t of templates) {
      const left = p.x - t.hotX - SEARCH_RADIUS;
      const top = p.y - t.hotY - SEARCH_RADIUS;
      if (left < x0) x0 = left;
      if (top < y0) y0 = top;
      if (left + 2 * SEARCH_RADIUS + t.width > x1) x1 = left + 2 * SEARCH_RADIUS + t.width;
      if (top + 2 * SEARCH_RADIUS + t.height > y1) y1 = top + 2 * SEARCH_RADIUS + t.height;
    }
  }
  x0 = Math.max(0, x0);
  y0 = Math.max(0, y0);
  x1 = Math.min(pictureWidth, x1);
  y1 = Math.min(pictureHeight, y1);
  if (x1 <= x0 || y1 <= y0) return null;
  return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
};

/** Reading more than this many pixels in one piece is not worth it: the
 *  positions are then read in several smaller pieces. */
export const MAX_READ_PIXELS = 40000;

/** The positions of one check, in their order, cut into runs whose
 *  `searchBox` stays small (a fast drag across the picture would otherwise
 *  have most of the picture read back). Matching the runs one after the
 *  other, stopping at the first match, is the same check. */
export const searchGroups = (
  positions: readonly Point[],
  templates: readonly Template[],
  pictureWidth: number,
  pictureHeight: number,
): { box: Box; positions: Point[] }[] => {
  const groups: { box: Box; positions: Point[] }[] = [];
  let run: Point[] = [];
  let runBox: Box | null = null;
  for (const p of positions) {
    const grown = searchBox([...run, p], templates, pictureWidth, pictureHeight);
    if (run.length > 0 && grown && grown.width * grown.height > MAX_READ_PIXELS) {
      if (runBox) groups.push({ box: runBox, positions: run });
      run = [p];
      runBox = searchBox(run, templates, pictureWidth, pictureHeight);
    } else {
      run.push(p);
      runBox = grown;
    }
  }
  if (run.length > 0 && runBox) groups.push({ box: runBox, positions: run });
  return groups;
};

/** One check: the scale of the first placement that matches, or 0 when none
 *  does. Positions are whole stream pixels. */
export const matchRegion = (
  region: LumaRegion,
  templates: readonly Template[],
  positions: readonly Point[],
): number => {
  const rw = region.width;
  const rh = region.height;
  const data = region.data;
  for (const p of positions) {
    for (const t of templates) {
      const limit = MAX_SCORE * t.count;
      const baseX = p.x - t.hotX - region.x;
      const baseY = p.y - t.hotY - region.y;
      const tdx = t.dx;
      const tdy = t.dy;
      const tl = t.luma;
      const n = t.count;
      for (let oy = -SEARCH_RADIUS; oy <= SEARCH_RADIUS; oy += 1) {
        const top = baseY + oy;
        // A placement with an ink pixel outside the picture is skipped.
        if (top + t.minY < 0 || top + t.maxY >= rh) continue;
        for (let ox = -SEARCH_RADIUS; ox <= SEARCH_RADIUS; ox += 1) {
          const left = baseX + ox;
          if (left + t.minX < 0 || left + t.maxX >= rw) continue;
          const origin = top * rw + left;
          let sum = 0;
          let i = 0;
          // The mean is over all ink pixels; once the sum is past the limit
          // the placement cannot match any more.
          for (; i < n && sum <= limit; i += 1) {
            const d = data[origin + tdy[i] * rw + tdx[i]] - tl[i];
            sum += d < 0 ? -d : d;
          }
          if (i === n && sum <= limit) return t.scale;
        }
      }
    }
  }
  return 0;
};

/** The pointer positions the viewer sent, for the matcher. */
export class SentPositions {
  private readonly xs: Int32Array;
  private readonly ys: Int32Array;
  private readonly ts: Float64Array;
  private readonly capacity: number;
  private next = 0;
  private size = 0;

  constructor(capacity = 256) {
    this.capacity = capacity;
    this.xs = new Int32Array(capacity);
    this.ys = new Int32Array(capacity);
    this.ts = new Float64Array(capacity);
  }

  /** A position sent to the host, in stream pixels, at `now` (ms). */
  record(x: number, y: number, now: number): void {
    const rx = Math.round(x);
    const ry = Math.round(y);
    if (this.size > 0) {
      const last = (this.next + this.capacity - 1) % this.capacity;
      if (this.xs[last] === rx && this.ys[last] === ry) {
        this.ts[last] = now;
        return;
      }
    }
    this.xs[this.next] = rx;
    this.ys[this.next] = ry;
    this.ts[this.next] = now;
    this.next = (this.next + 1) % this.capacity;
    if (this.size < this.capacity) this.size += 1;
  }

  clear(): void {
    this.size = 0;
    this.next = 0;
  }

  /** The positions to look at, newest first, at most MAX_POSITIONS.
   *
   *  - The newest position is always there, however old: the host's pointer
   *    still is where it was last sent.
   *  - Older ones only from the last POSITION_WINDOW_MS.
   *  - A position within SEARCH_RADIUS (both ways) of one already taken is
   *    left out: its placements are among those of the one taken, so nothing
   *    is lost and the sixteen reach further back at a high send rate. */
  recent(now: number): Point[] {
    const out: Point[] = [];
    for (let k = 1; k <= this.size && out.length < MAX_POSITIONS; k += 1) {
      const i = (this.next + this.capacity - k) % this.capacity;
      if (k > 1 && now - this.ts[i] > POSITION_WINDOW_MS) break;
      const x = this.xs[i];
      const y = this.ys[i];
      let covered = false;
      for (const p of out) {
        if (Math.abs(p.x - x) <= SEARCH_RADIUS && Math.abs(p.y - y) <= SEARCH_RADIUS) {
          covered = true;
          break;
        }
      }
      if (!covered) out.push({ x, y });
    }
    return out;
  }
}

/** The decision over one drag: left button held while the host says the
 *  pointer may be in the picture. */
/** Whether a drag looks for the host's pointer in the picture: when the
 *  host says it may be there, and on every drag of a Windows host. Windows
 *  draws its pointer into the picture while a window is dragged; the host
 *  says so (`in_window_move_size`) only for the system's own move loop, not
 *  for the many apps that move their windows themselves (2026-11-24: two
 *  pointers while a window was dragged). */
export const looksForPointerInPicture = (hostSaysMaybe: boolean, hostOs: string): boolean =>
  hostSaysMaybe || hostOs === 'windows';

export class PointerInPictureDrag {
  /** A drag is on. */
  active = false;
  /** Decided for this drag: the pointer is in the picture. */
  inPicture = false;
  /** Checks done in this drag. */
  checks = 0;
  /** Checks in a row that found the pointer. */
  run = 0;
  /** The scale that matched in this drag (0: none yet). */
  scale = 0;
  /** Counts the drags, to tell a check begun in an earlier one. */
  generation = 0;
  private lastCheckAt = -Infinity;

  /** The button and the host's "maybe" as they are now. Returns the log
   *  line when a drag has just ended, else null. */
  update(leftButtonHeld: boolean, maybeInPicture: boolean): string | null {
    const on = leftButtonHeld && maybeInPicture;
    if (on === this.active) return null;
    if (on) {
      this.active = true;
      this.generation += 1;
      this.inPicture = false;
      this.checks = 0;
      this.run = 0;
      this.scale = 0;
      this.lastCheckAt = -Infinity;
      return null;
    }
    const line = this.inPicture
      ? `${LOG_PREFIX}found after ${this.checks} checks`
      : `${LOG_PREFIX}not found (${this.checks} checks)`;
    this.active = false;
    this.inPicture = false;
    return line;
  }

  /** Whether a check is due at `now` (ms). */
  wantsCheck(now: number): boolean {
    return this.active && !this.inPicture && now - this.lastCheckAt >= CHECK_INTERVAL_MS;
  }

  /** The templates to try in the next check: all, or only the scale that
   *  has matched in this drag. */
  templatesToTry(templates: readonly Template[]): readonly Template[] {
    if (this.scale === 0) return templates;
    return templates.filter((t) => t.scale === this.scale);
  }

  /** The result of one check (`matchedScale` 0: not found). Returns true
   *  when this check decided "in the picture". */
  noteCheck(now: number, matchedScale: number): boolean {
    if (!this.active || this.inPicture) return false;
    this.lastCheckAt = now;
    this.checks += 1;
    if (matchedScale > 0) {
      this.scale = matchedScale;
      this.run += 1;
      if (this.run >= CHECKS_NEEDED) {
        this.inPicture = true;
        return true;
      }
    } else {
      this.run = 0;
    }
    return false;
  }
}

/** One whole check on a luma region already read: for tests and for callers
 *  that hold the picture's luma. */
export const checkOnce = (
  drag: PointerInPictureDrag,
  templates: readonly Template[],
  region: LumaRegion,
  positions: readonly Point[],
  now: number,
): boolean => drag.noteCheck(now, matchRegion(region, drag.templatesToTry(templates), positions));

// ---------------------------------------------------------------------------
// Browser part: reading a small region of the decoded picture.

/** Decoded pictures WebCodecs can hand out as RGBA whatever they are. */
const YUV_FORMATS = ['I420', 'I420A', 'I422', 'I444', 'NV12'];

/** Reads the luma of a rectangle of the decoded picture. */
export class PictureReader {
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private luma: Uint8Array = new Uint8Array(0);
  private rgba: Uint8Array = new Uint8Array(0);
  private frameCopyWorks = typeof VideoFrame === 'function';
  /** Why the last read returned null. */
  error = '';
  /** How the last region was read: 'frame' (WebCodecs) or 'canvas'. */
  path: 'frame' | 'canvas' | '' = '';

  private toLuma(rgba: Uint8Array | Uint8ClampedArray, box: Box): LumaRegion {
    if (this.luma.length < box.width * box.height) {
      this.luma = new Uint8Array(box.width * box.height);
    }
    lumaFromRgba(rgba, box.width, box.height, this.luma);
    return { x: box.x, y: box.y, width: box.width, height: box.height, data: this.luma };
  }

  /** The luma of `box` (whole stream pixels, inside the picture) of a video
   *  element, or null when the picture cannot be read here (`error`). The
   *  result may be a little larger than `box`; its data is reused by the
   *  next read.
   *
   *  The frame on show is copied out through WebCodecs where the browser
   *  can (only the rectangle, off the main thread's back); else it is drawn
   *  to a small canvas (`read`). */
  async readVideo(video: HTMLVideoElement, box: Box): Promise<LumaRegion | null> {
    if (this.frameCopyWorks) {
      let frame: VideoFrame | null = null;
      try {
        frame = new VideoFrame(video);
        const visible = frame.visibleRect;
        // Even edges: a rectangle of a picture with halved colour planes
        // must start on a whole colour sample.
        const x = box.x & ~1;
        const y = box.y & ~1;
        const width = Math.min(video.videoWidth - x, (box.x + box.width - x + 1) & ~1);
        const height = Math.min(video.videoHeight - y, (box.y + box.height - y + 1) & ~1);
        const options = {
          rect: { x: (visible?.x ?? 0) + x, y: (visible?.y ?? 0) + y, width, height },
          format: 'RGBA',
        } as VideoFrameCopyToOptions;
        if (
          visible
          && visible.width === video.videoWidth
          && visible.height === video.videoHeight
          && frame.format !== null
          && YUV_FORMATS.includes(frame.format)
          // A browser that does not convert would ask for the planes' size.
          && frame.allocationSize(options) === width * height * 4
        ) {
          if (this.rgba.length < width * height * 4) this.rgba = new Uint8Array(width * height * 4);
          const layout = await frame.copyTo(this.rgba, options);
          if (layout.length === 1 && layout[0].offset === 0 && layout[0].stride === width * 4) {
            this.path = 'frame';
            return this.toLuma(this.rgba, { x, y, width, height });
          }
        }
        this.frameCopyWorks = false;
      } catch {
        this.frameCopyWorks = false;
      } finally {
        frame?.close();
      }
    }
    return this.read(video, box);
  }

  /** The same through a small 2D canvas: `drawImage` of the rectangle, then
   *  `getImageData`. Works for anything that can be drawn. */
  read(source: CanvasImageSource, box: Box): LumaRegion | null {
    try {
      if (!this.canvas || !this.ctx) {
        this.canvas = document.createElement('canvas');
        this.canvas.width = 0;
        this.canvas.height = 0;
        this.ctx = this.canvas.getContext('2d', { willReadFrequently: true });
        if (!this.ctx) {
          this.error = 'no 2D canvas';
          return null;
        }
      }
      // Grow only: resizing a canvas throws its backing store away.
      if (this.canvas.width < box.width || this.canvas.height < box.height) {
        this.canvas.width = Math.max(this.canvas.width, box.width, 160);
        this.canvas.height = Math.max(this.canvas.height, box.height, 160);
      }
      this.ctx.drawImage(source, box.x, box.y, box.width, box.height, 0, 0, box.width, box.height);
      const image = this.ctx.getImageData(0, 0, box.width, box.height);
      this.path = 'canvas';
      return this.toLuma(image.data, box);
    } catch (err) {
      this.error = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
      return null;
    }
  }
}
