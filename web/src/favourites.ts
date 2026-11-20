// Favourite computers and the ones used last, kept in this browser for each
// account on each server ("<user>@<server>"). Cards are named by their key
// (computerCards.ts: "host:<id>", "group:<group>", "source:<host>:<id>").
// Pure functions over a key-value store (tests/favourites.test.ts).

type Store = { getItem(key: string): string | null; setItem(key: string, value: string): void };

/** What the list shows: every card, the favourites, or those used lately. */
export type CardShow = 'all' | 'favourites' | 'recent';

export const SHOW_STORE = 'lunaris_computer_show';
const FAVOURITES_PREFIX = 'lunaris_favourites:';
const RECENT_PREFIX = 'lunaris_recent:';
/** The most cards remembered as used. */
export const MAX_RECENT = 30;
const MAX_FAVOURITES = 500;

/** The account on the server the lists are kept for. */
export const accountKey = (user: string, server: string): string =>
  `${user.trim().toLowerCase()}@${server.trim().replace(/^[a-z]+:\/\//i, '').replace(/\/+$/, '').toLowerCase()}`;

const readJson = (store: Store, key: string): unknown => {
  try { return JSON.parse(store.getItem(key) || 'null'); } catch { return null; }
};
const write = (store: Store, key: string, value: unknown) => {
  try { store.setItem(key, JSON.stringify(value)); } catch { /* storage unavailable */ }
};
const cardKeyOk = (k: unknown): k is string => typeof k === 'string' && k.length > 0 && k.length <= 300;

export function loadFavourites(store: Store, account: string): string[] {
  const list = readJson(store, FAVOURITES_PREFIX + account);
  return Array.isArray(list) ? [...new Set(list.filter(cardKeyOk))] : [];
}

/** Makes `card` a favourite, or no longer one; returns the new list. */
export function toggleFavourite(store: Store, account: string, card: string): string[] {
  const now = loadFavourites(store, account);
  const next = now.includes(card) ? now.filter((k) => k !== card) : [...now, card].slice(-MAX_FAVOURITES);
  write(store, FAVOURITES_PREFIX + account, next);
  return next;
}

/** { "<card key>": when it was last connected to (ms) }. */
export function loadRecent(store: Store, account: string): Record<string, number> {
  const map = readJson(store, RECENT_PREFIX + account);
  if (!map || typeof map !== 'object' || Array.isArray(map)) return {};
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(map as Record<string, unknown>)) {
    if (cardKeyOk(k) && typeof v === 'number' && Number.isFinite(v) && v > 0) out[k] = v;
  }
  return out;
}

/** Remembers that `card` was connected to at `when`; the oldest beyond MAX_RECENT are forgotten. */
export function recordRecent(store: Store, account: string, card: string, when: number = Date.now()): Record<string, number> {
  const all = { ...loadRecent(store, account), [card]: when };
  const kept = Object.entries(all).sort((a, b) => b[1] - a[1]).slice(0, MAX_RECENT);
  const next = Object.fromEntries(kept);
  write(store, RECENT_PREFIX + account, next);
  return next;
}

export function loadShow(store: Store): CardShow {
  try {
    const v = store.getItem(SHOW_STORE);
    return v === 'favourites' || v === 'recent' ? v : 'all';
  } catch { return 'all'; }
}
export function saveShow(store: Store, show: CardShow): void {
  try { store.setItem(SHOW_STORE, show); } catch { /* storage unavailable */ }
}

/**
 * The cards to show, in order. "all": every card, the favourites first
 * (each group in the order given). "favourites": only those. "recent":
 * only the cards connected to, the last used first.
 */
export function arrangeCards<C extends { key: string }>(
  cards: C[],
  show: CardShow,
  favourites: string[],
  recent: Record<string, number>,
): C[] {
  const fav = new Set(favourites);
  if (show === 'favourites') return cards.filter((c) => fav.has(c.key));
  if (show === 'recent') {
    return cards
      .map((c, i) => ({ c, i }))
      .filter(({ c }) => recent[c.key] !== undefined)
      .sort((a, b) => recent[b.c.key] - recent[a.c.key] || a.i - b.i)
      .map(({ c }) => c);
  }
  return [...cards.filter((c) => fav.has(c.key)), ...cards.filter((c) => !fav.has(c.key))];
}
