// Stream settings: this browser's defaults, and what a computer has of its own.
//
// The defaults are the keys the web always kept (lunaris_stream_res, ...).
// A computer (a Lunaris computer, or a computer of another type reached
// through one) may keep its own resolution, frame rate, bitrate, codec,
// chroma, encoder and display: only those that differ from the defaults are
// kept, under lunaris_computer_stream, so the rest follow the defaults.
// "Use defaults" forgets what the computer has of its own.
//
// Used for sessions in this browser (StreamPlayer) and for lunaris://connect
// links (App.tsx openInDesktopApp). Pure functions over a key-value store
// (tests/streamPrefs.test.ts).

import { DEFAULT_BITRATE, DEFAULT_CODEC, DEFAULT_RESOLUTION, FPS_OPTIONS } from './streamOptions.ts';

export type KeyValue = { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem?(key: string): void };

/** The settings a computer can have of its own. */
export type ComputerPrefs = {
  resolution: string;
  fps: number;
  bitrate: number;
  codec: string;
  /** '420' | '444' */
  chroma: string;
  encoder: string;
  display: string;
};

export const COMPUTER_PREF_KEYS = ['resolution', 'fps', 'bitrate', 'codec', 'chroma', 'encoder', 'display'] as const;
export type ComputerPrefKey = typeof COMPUTER_PREF_KEYS[number];

/** Where each default is kept (the keys the web always used). */
export const DEFAULT_STORE_KEYS: Record<ComputerPrefKey, string> = {
  resolution: 'lunaris_stream_res',
  fps: 'lunaris_stream_fps',
  bitrate: 'lunaris_stream_bitrate',
  codec: 'lunaris_stream_codec',
  chroma: 'lunaris_stream_chroma',
  encoder: 'lunaris_stream_encoder',
  display: 'lunaris_stream_display',
};

export const OVERRIDES_STORE = 'lunaris_computer_stream';
/** The display's refresh rate as this browser measured it (Hz). */
export const REFRESH_STORE = 'lunaris_refresh_hz';
/** The most computers kept (the oldest are forgotten). */
const MAX_COMPUTERS = 300;

/** The key of a Lunaris computer, and of a computer reached through one. */
export const hostPrefsKey = (hostId: string) => `host:${hostId}`;
export const sourcePrefsKey = (hostId: string, sourceId: string) => `source:${hostId}:${sourceId}`;

const read = (store: KeyValue, key: string): string | null => {
  try { return store.getItem(key); } catch { return null; }
};

const positive = (text: string | null | undefined): number | null => {
  const n = Number(text);
  return text !== null && text !== undefined && text !== '' && Number.isFinite(n) && n > 0 ? n : null;
};

/**
 * The frame rate for a display of `hz`: the highest of the frame rates
 * offered that the display shows (5% slack: 59.94 Hz is 60), never below 60
 * for a display of 45 Hz or more; 60 when the rate is not known.
 */
export function fpsForRefresh(hz: number | null | undefined): number {
  if (!hz || !Number.isFinite(hz) || hz <= 0) return 60;
  const fits = FPS_OPTIONS.filter((f) => f <= hz * 1.05);
  const best = fits.length ? Math.max(...fits) : Math.min(...FPS_OPTIONS);
  return hz >= 45 && best < 60 ? 60 : best;
}

/**
 * The refresh rate from frame times (ms between requestAnimationFrame
 * calls): from their median, so a few late frames don't count. Null when
 * there are too few or they make no sense (a hidden tab, a busy page).
 */
export function refreshFromIntervals(intervals: number[]): number | null {
  const ok = intervals.filter((d) => Number.isFinite(d) && d > 1 && d < 100).sort((a, b) => a - b);
  if (ok.length < 10) return null;
  const median = ok[Math.floor(ok.length / 2)];
  const hz = 1000 / median;
  return hz >= 20 && hz <= 500 ? Math.round(hz * 100) / 100 : null;
}

/** The refresh rate measured before (REFRESH_STORE), if any. */
export const loadRefresh = (store: KeyValue): number | null => {
  const hz = positive(read(store, REFRESH_STORE));
  return hz && hz >= 20 && hz <= 500 ? hz : null;
};

/** This browser's defaults; a frame rate never saved follows the display. */
export function loadDefaults(store: KeyValue): ComputerPrefs {
  return {
    resolution: read(store, DEFAULT_STORE_KEYS.resolution) || DEFAULT_RESOLUTION,
    fps: positive(read(store, DEFAULT_STORE_KEYS.fps)) ?? fpsForRefresh(loadRefresh(store)),
    bitrate: positive(read(store, DEFAULT_STORE_KEYS.bitrate)) ?? DEFAULT_BITRATE,
    codec: read(store, DEFAULT_STORE_KEYS.codec) || DEFAULT_CODEC,
    chroma: read(store, DEFAULT_STORE_KEYS.chroma) === '444' ? '444' : '420',
    encoder: read(store, DEFAULT_STORE_KEYS.encoder) || 'auto',
    display: read(store, DEFAULT_STORE_KEYS.display) || 'default',
  };
}

export function saveDefaults(store: KeyValue, prefs: ComputerPrefs): void {
  for (const key of COMPUTER_PREF_KEYS) {
    try { store.setItem(DEFAULT_STORE_KEYS[key], String(prefs[key])); } catch { /* storage unavailable */ }
  }
}

/** One computer's own values, checked (nonsense is dropped). */
function cleanOverride(value: unknown): Partial<ComputerPrefs> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const v = value as Record<string, unknown>;
  const out: Partial<ComputerPrefs> = {};
  for (const key of ['resolution', 'codec', 'encoder', 'display'] as const) {
    if (typeof v[key] === 'string' && v[key] && (v[key] as string).length <= 64) out[key] = v[key] as string;
  }
  for (const key of ['fps', 'bitrate'] as const) {
    if (typeof v[key] === 'number' && Number.isFinite(v[key]) && (v[key] as number) > 0) out[key] = v[key] as number;
  }
  if (v.chroma === '420' || v.chroma === '444') out.chroma = v.chroma;
  return out;
}

/** { "<computer key>": its own values }, as saved. */
export function loadOverrides(store: KeyValue): Record<string, Partial<ComputerPrefs>> {
  try {
    const parsed: unknown = JSON.parse(read(store, OVERRIDES_STORE) || '{}');
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    const out: Record<string, Partial<ComputerPrefs>> = {};
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      const clean = cleanOverride(value);
      if (key.length <= 300 && Object.keys(clean).length > 0) out[key] = clean;
    }
    return out;
  } catch {
    return {};
  }
}

/** What `computer` has of its own ({} when it follows the defaults). */
export const overrideOf = (store: KeyValue, computer: string): Partial<ComputerPrefs> => loadOverrides(store)[computer] || {};

export const hasOverride = (store: KeyValue, computer: string): boolean => Object.keys(overrideOf(store, computer)).length > 0;

/** The settings a computer is connected with: its own, else the defaults. */
export const resolvePrefs = (defaults: ComputerPrefs, own: Partial<ComputerPrefs>): ComputerPrefs => ({ ...defaults, ...own });

export const prefsFor = (store: KeyValue, computer: string | null | undefined): ComputerPrefs => {
  const own = computer ? overrideOf(store, computer) : {};
  const prefs = resolvePrefs(loadDefaults(store), own);
  // The default display is one of a Lunaris computer's own: a computer
  // reached through one has other displays, so it starts from its own.
  if (computer?.startsWith('source:') && !own.display) prefs.display = 'default';
  return prefs;
};

/** The values that differ from the defaults: what a computer keeps of its own. */
export function differences(defaults: ComputerPrefs, values: ComputerPrefs): Partial<ComputerPrefs> {
  const out: Partial<ComputerPrefs> = {};
  for (const key of COMPUTER_PREF_KEYS) {
    if (String(values[key]) !== String(defaults[key])) (out as Record<string, unknown>)[key] = values[key];
  }
  return out;
}

/** Keeps `values` for `computer`: what differs from the defaults (nothing
 *  differs: it follows the defaults again). */
export function saveComputerPrefs(store: KeyValue, computer: string, values: ComputerPrefs): Partial<ComputerPrefs> {
  const own = differences(loadDefaults(store), values);
  const all = loadOverrides(store);
  delete all[computer];
  if (Object.keys(own).length > 0) all[computer] = own;
  const keys = Object.keys(all);
  for (const old of keys.slice(0, Math.max(0, keys.length - MAX_COMPUTERS))) delete all[old];
  try { store.setItem(OVERRIDES_STORE, JSON.stringify(all)); } catch { /* storage unavailable */ }
  return own;
}

/** Changes some of `computer`'s settings (in a session): the others stay as they are. */
export const updateComputerPrefs = (store: KeyValue, computer: string, change: Partial<ComputerPrefs>): Partial<ComputerPrefs> =>
  saveComputerPrefs(store, computer, { ...prefsFor(store, computer), ...change });

/** "Use defaults": the computer has nothing of its own any more. */
export const clearComputerPrefs = (store: KeyValue, computer: string): void => {
  const all = loadOverrides(store);
  if (!(computer in all)) return;
  delete all[computer];
  try { store.setItem(OVERRIDES_STORE, JSON.stringify(all)); } catch { /* storage unavailable */ }
};

/** The codec for a lunaris:// link: 4:4:4 is H.264 only, and the app takes
 *  it as the codec "h264_444" (client-qml/src/deeplink.rs CODECS). */
export const linkCodec = (prefs: Pick<ComputerPrefs, 'codec' | 'chroma'>): string =>
  prefs.chroma === '444' ? 'h264_444' : prefs.codec;

/**
 * Measures the display's refresh rate once (about half a second of
 * animation frames) and keeps it in REFRESH_STORE. Only in a visible page:
 * a hidden one gets no frames.
 */
export function measureRefresh(store: KeyValue, frames = 40): Promise<number | null> {
  return new Promise((resolve) => {
    if (typeof requestAnimationFrame !== 'function' || (typeof document !== 'undefined' && document.visibilityState === 'hidden')) {
      resolve(null);
      return;
    }
    const times: number[] = [];
    const step = (t: number) => {
      times.push(t);
      if (times.length <= frames) { requestAnimationFrame(step); return; }
      const hz = refreshFromIntervals(times.slice(1).map((v, i) => v - times[i]));
      if (hz) { try { store.setItem(REFRESH_STORE, String(hz)); } catch { /* storage unavailable */ } }
      resolve(hz);
    };
    requestAnimationFrame(step);
  });
}
