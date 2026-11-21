// Sign-in links are no longer accepted (src/App.tsx).

/** Fragment keys of the old `#session=<token>&user=..&connect=..&name=..` link. */
const HASH_KEYS = ['session', 'user', 'connect', 'name'];
/** Query keys of the old `?token=<token>&host_id=..` link. */
const QUERY_KEYS = ['token', 'host_id', 'host_name', 'codec_support', 'res', 'fps', 'bitrate', 'codec'];

/**
 * The address without an old sign-in link's parameters, or null when it
 * carries none. Nothing in them is used: a link that signs a browser in lets
 * whoever made it choose the account (login CSRF).
 */
export const stripSessionLinkParams = (pathname: string, search: string, hash: string): string | null => {
  const h = new URLSearchParams(hash.replace(/^#/, ''));
  const q = new URLSearchParams(search);
  const hashLink = h.has('session');
  const queryLink = q.has('token') || q.has('host_id');
  if (!hashLink && !queryLink) return null;
  if (hashLink) for (const k of HASH_KEYS) h.delete(k);
  if (queryLink) for (const k of QUERY_KEYS) q.delete(k);
  const qs = q.toString();
  const hs = hashLink ? h.toString() : hash.replace(/^#/, '');
  return pathname + (qs ? `?${qs}` : '') + (hs ? `#${hs}` : '');
};
