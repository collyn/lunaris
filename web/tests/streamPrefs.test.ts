// Stream settings for each computer (src/streamPrefs.ts).
// Run: npm run test:stream-prefs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  OVERRIDES_STORE, REFRESH_STORE, clearComputerPrefs, differences, fpsForRefresh, hasOverride, hostPrefsKey, linkCodec,
  loadDefaults, loadOverrides, overrideOf, prefsFor, refreshFromIntervals, saveComputerPrefs, saveDefaults, sourcePrefsKey,
  updateComputerPrefs,
} from '../src/streamPrefs.ts';

const memory = (init: Record<string, string> = {}) => {
  const data = new Map(Object.entries(init));
  return {
    data,
    getItem: (k: string) => (data.has(k) ? data.get(k)! : null),
    setItem: (k: string, v: string) => { data.set(k, v); },
    removeItem: (k: string) => { data.delete(k); },
  };
};

test('defaults when nothing is saved', () => {
  const d = loadDefaults(memory());
  assert.deepEqual(d, { resolution: '1080p', fps: 60, bitrate: 15000, codec: 'auto', chroma: '420', encoder: 'auto', display: 'default' });
});

test('saved defaults are read back; nonsense falls back', () => {
  const s = memory({ lunaris_stream_res: '1440p', lunaris_stream_fps: '120', lunaris_stream_bitrate: 'x', lunaris_stream_chroma: '444' });
  const d = loadDefaults(s);
  assert.equal(d.resolution, '1440p');
  assert.equal(d.fps, 120);
  assert.equal(d.bitrate, 15000);
  assert.equal(d.chroma, '444');
  saveDefaults(s, { ...d, fps: 30 });
  assert.equal(s.getItem('lunaris_stream_fps'), '30');
});

test('the frame rate follows the display when none is saved', () => {
  assert.equal(loadDefaults(memory({ [REFRESH_STORE]: '143.9' })).fps, 144);
  assert.equal(loadDefaults(memory({ [REFRESH_STORE]: '143.9', lunaris_stream_fps: '60' })).fps, 60);
  assert.equal(loadDefaults(memory({ [REFRESH_STORE]: 'nonsense' })).fps, 60);
});

test('fps for a refresh rate: capped to the list', () => {
  assert.equal(fpsForRefresh(59.94), 60);
  assert.equal(fpsForRefresh(60), 60);
  assert.equal(fpsForRefresh(75), 75);
  assert.equal(fpsForRefresh(100), 90);
  assert.equal(fpsForRefresh(165), 144);
  assert.equal(fpsForRefresh(360), 240);
  assert.equal(fpsForRefresh(50), 60);
  assert.equal(fpsForRefresh(30), 30);
  assert.equal(fpsForRefresh(24), 30);
  assert.equal(fpsForRefresh(null), 60);
  assert.equal(fpsForRefresh(NaN), 60);
});

test('refresh rate from frame intervals', () => {
  assert.equal(refreshFromIntervals(Array(20).fill(1000 / 60)), 60);
  // A few late frames don't count.
  assert.equal(Math.round(refreshFromIntervals([...Array(30).fill(1000 / 144), 50, 70, 33])!), 144);
  assert.equal(refreshFromIntervals([16, 16]), null);
  assert.equal(refreshFromIntervals(Array(20).fill(200)), null);
});

test('a computer keeps only what differs from the defaults', () => {
  const s = memory({ lunaris_stream_fps: '60' });
  const key = hostPrefsKey('h1');
  const own = saveComputerPrefs(s, key, { ...loadDefaults(s), fps: 120, display: '\\\\.\\DISPLAY2' });
  assert.deepEqual(own, { fps: 120, display: '\\\\.\\DISPLAY2' });
  assert.equal(prefsFor(s, key).fps, 120);
  assert.equal(prefsFor(s, key).resolution, '1080p');
  // Other computers follow the defaults.
  assert.equal(prefsFor(s, hostPrefsKey('h2')).fps, 60);
  // The defaults change: what the computer did not change follows them.
  s.setItem('lunaris_stream_res', '720p');
  assert.equal(prefsFor(s, key).resolution, '720p');
  assert.equal(prefsFor(s, key).fps, 120);
  assert.ok(hasOverride(s, key));
});

test('saving the defaults again, or "Use defaults", forgets the computer', () => {
  const s = memory();
  const key = sourcePrefsKey('h1', 'rdp-1');
  saveComputerPrefs(s, key, { ...loadDefaults(s), bitrate: 50000 });
  assert.ok(hasOverride(s, key));
  saveComputerPrefs(s, key, loadDefaults(s));
  assert.ok(!hasOverride(s, key));
  assert.equal(loadOverrides(s)[key], undefined);
  saveComputerPrefs(s, key, { ...loadDefaults(s), codec: 'h265' });
  clearComputerPrefs(s, key);
  assert.deepEqual(overrideOf(s, key), {});
});

test('a change in a session keeps the computer\'s other values', () => {
  const s = memory();
  const key = hostPrefsKey('h1');
  saveComputerPrefs(s, key, { ...loadDefaults(s), fps: 30 });
  updateComputerPrefs(s, key, { bitrate: 8000 });
  assert.deepEqual(overrideOf(s, key), { fps: 30, bitrate: 8000 });
});

test('nonsense saved is ignored', () => {
  const s = memory({ [OVERRIDES_STORE]: JSON.stringify({ 'host:a': { fps: 'fast', chroma: '422', codec: 'av1' }, 'host:b': 5, 'host:c': {} }) });
  assert.deepEqual(loadOverrides(s), { 'host:a': { codec: 'av1' } });
  assert.deepEqual(loadOverrides(memory({ [OVERRIDES_STORE]: '{broken' })), {});
});

test('differences and the link codec', () => {
  const d = loadDefaults(memory());
  assert.deepEqual(differences(d, d), {});
  assert.deepEqual(differences(d, { ...d, chroma: '444' }), { chroma: '444' });
  assert.equal(linkCodec({ codec: 'h265', chroma: '444' }), 'h264_444');
  assert.equal(linkCodec({ codec: 'h265', chroma: '420' }), 'h265');
});

test('a computer reached through another starts from its own display', () => {
  const s = memory({ lunaris_stream_display: '\\\\.\\DISPLAY2' });
  assert.equal(prefsFor(s, hostPrefsKey('h1')).display, '\\\\.\\DISPLAY2');
  assert.equal(prefsFor(s, sourcePrefsKey('h1', 'vnc-1')).display, 'default');
  saveComputerPrefs(s, sourcePrefsKey('h1', 'vnc-1'), { ...prefsFor(s, sourcePrefsKey('h1', 'vnc-1')), display: '1' });
  assert.equal(prefsFor(s, sourcePrefsKey('h1', 'vnc-1')).display, '1');
});
