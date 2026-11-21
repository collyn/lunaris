// Whether administrators are told on every page that a newer Lunaris is out
// (the Server page has the details). Pure functions (tests/updateNotice.test.ts).

import type { ServerInfo } from './serverInfo';

export type UpdateNotice = {
  /** The newer release, e.g. "0.7.2". */
  latest: string;
  /** The version this server runs. */
  current: string;
  /** Its release notes (http(s) only, checked by parseServerInfo). */
  url?: string;
};

/** localStorage key: the release whose notice was closed. */
export const DISMISSED_KEY = 'lunaris_update_dismissed';

const bare = (v: string) => v.trim().replace(/^v/i, '');

/** The notice to show, or null: none when nothing newer is known, and none
 *  for the release whose notice was closed (a later one shows again). */
export function updateToShow(info: ServerInfo | null, dismissed: string | null): UpdateNotice | null {
  const u = info?.update;
  if (!u || !u.checks || !u.available || !u.latest) return null;
  const latest = bare(u.latest);
  if (!latest) return null;
  if (dismissed && bare(dismissed) === latest) return null;
  return { latest, current: bare(info?.version || ''), url: u.url };
}

/** Whether the Server tab carries its "update" dot (also after the notice was closed). */
export const updateAvailable = (info: ServerInfo | null): boolean =>
  !!info?.update?.checks && !!info.update.available && !!info.update.latest;

export function readDismissed(): string | null {
  try { return localStorage.getItem(DISMISSED_KEY); } catch { return null; }
}

export function writeDismissed(version: string): void {
  try { localStorage.setItem(DISMISSED_KEY, bare(version)); } catch { /* private mode: shows again next time */ }
}
