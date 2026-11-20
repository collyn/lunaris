// Favourite and recently used computers (src/favourites.ts).
// Run: npm run test:favourites
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_RECENT, SHOW_STORE, accountKey, arrangeCards, loadFavourites, loadRecent, loadShow, recordRecent, saveShow, toggleFavourite,
} from '../src/favourites.ts';

const memory = (init: Record<string, string> = {}) => {
  const data = new Map(Object.entries(init));
  return { getItem: (k: string) => (data.has(k) ? data.get(k)! : null), setItem: (k: string, v: string) => { data.set(k, v); } };
};
const cards = ['host:a', 'host:b', 'group:c', 'source:h:d'].map((key) => ({ key }));
const keys = (list: { key: string }[]) => list.map((c) => c.key);

test('one list per account and server', () => {
  assert.equal(accountKey('Alice', 'https://Example.test:8443/'), 'alice@example.test:8443');
  const s = memory();
  toggleFavourite(s, accountKey('alice', 'srv'), 'host:a');
  assert.deepEqual(loadFavourites(s, accountKey('alice', 'srv')), ['host:a']);
  assert.deepEqual(loadFavourites(s, accountKey('bob', 'srv')), []);
  assert.deepEqual(loadFavourites(s, accountKey('alice', 'other')), []);
});

test('a favourite is toggled', () => {
  const s = memory();
  assert.deepEqual(toggleFavourite(s, 'u@s', 'host:b'), ['host:b']);
  assert.deepEqual(toggleFavourite(s, 'u@s', 'group:c'), ['host:b', 'group:c']);
  assert.deepEqual(toggleFavourite(s, 'u@s', 'host:b'), ['group:c']);
});

test('nonsense saved is ignored', () => {
  const s = memory({ 'lunaris_favourites:u@s': JSON.stringify(['host:a', 5, '', 'host:a']), 'lunaris_recent:u@s': JSON.stringify({ 'host:a': 'x', 'host:b': 7 }) });
  assert.deepEqual(loadFavourites(s, 'u@s'), ['host:a']);
  assert.deepEqual(loadRecent(s, 'u@s'), { 'host:b': 7 });
  assert.deepEqual(loadFavourites(memory({ 'lunaris_favourites:u@s': '{' }), 'u@s'), []);
});

test('recent: the last used first, the oldest forgotten', () => {
  const s = memory();
  for (let i = 0; i < MAX_RECENT + 5; i++) recordRecent(s, 'u@s', `host:${i}`, 1000 + i);
  const recent = loadRecent(s, 'u@s');
  assert.equal(Object.keys(recent).length, MAX_RECENT);
  assert.equal(recent['host:0'], undefined);
  assert.equal(recent[`host:${MAX_RECENT + 4}`], 1000 + MAX_RECENT + 4);
});

test('arranging the cards', () => {
  const fav = ['group:c', 'host:gone'];
  const recent = { 'host:b': 10, 'source:h:d': 30, 'host:gone': 50 };
  assert.deepEqual(keys(arrangeCards(cards, 'all', fav, recent)), ['group:c', 'host:a', 'host:b', 'source:h:d']);
  assert.deepEqual(keys(arrangeCards(cards, 'favourites', fav, recent)), ['group:c']);
  assert.deepEqual(keys(arrangeCards(cards, 'recent', fav, recent)), ['source:h:d', 'host:b']);
  assert.deepEqual(keys(arrangeCards(cards, 'all', [], {})), keys(cards));
  assert.deepEqual(arrangeCards(cards, 'recent', [], {}), []);
});

test('what is shown is remembered', () => {
  const s = memory();
  assert.equal(loadShow(s), 'all');
  saveShow(s, 'recent');
  assert.equal(loadShow(s), 'recent');
  assert.equal(loadShow(memory({ [SHOW_STORE]: 'bogus' })), 'all');
});
