// Admin API for install tokens (one-line agent installation). All the HTTP
// details live here so the page does not depend on them.
//   POST   /api/admin/install-tokens        create; the token is in this answer only
//   GET    /api/admin/install-tokens        list (never the token itself)
//   DELETE /api/admin/install-tokens/{id}   revoke

import { parseInstallFetch, type InstallFetch, type InstallToken } from './installCommand.ts';

export interface CreatedInstallToken extends InstallToken {
  token: string;
  /** How the command fetches the script (null: an older server). */
  install: InstallFetch | null;
}

export interface CreateInstallTokenRequest {
  label: string;
  valid_hours: number;
  max_uses: number | null;
}

/** An error carrying the server's message (if any) and the HTTP status. */
export class InstallTokenApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

const normalize = (raw: Record<string, unknown>): InstallToken => ({
  id: String(raw.id ?? ''),
  label: typeof raw.label === 'string' ? raw.label : '',
  created_at: (raw.created_at as string | number | null) ?? null,
  expires_at: (raw.expires_at as string | number | null) ?? null,
  max_uses: typeof raw.max_uses === 'number' ? raw.max_uses : null,
  uses: typeof raw.uses === 'number' ? raw.uses : 0,
  revoked: raw.revoked === true || raw.revoked === 1,
  has_command: raw.has_command === true,
  computers: Array.isArray(raw.computers)
    ? (raw.computers as Record<string, unknown>[])
      .filter(c => c && typeof c.id === 'string')
      .map(c => ({ id: String(c.id), name: typeof c.name === 'string' && c.name ? c.name : String(c.id), online: c.online === true }))
    : [],
});

const failure = async (res: Response, fallback: string): Promise<InstallTokenApiError> => {
  let message = fallback;
  try {
    const body = await res.json();
    if (body && typeof body.error === 'string' && body.error) message = body.error;
  } catch { /* not JSON */ }
  return new InstallTokenApiError(message, res.status);
};

export const installTokensApi = (apiBase: string, authToken: string) => {
  const url = `${apiBase}/api/admin/install-tokens`;
  const auth = { Authorization: `Bearer ${authToken}` };
  return {
    /** The tokens, and the computers that joined with a token deleted since. */
    async list(): Promise<{ tokens: InstallToken[]; orphans: NonNullable<InstallToken['computers']> }> {
      const res = await fetch(url, { headers: auth });
      if (!res.ok) throw await failure(res, 'Could not load the install tokens');
      const data = await res.json();
      // Servers before 0.8.4 answer the list itself.
      const tokens = Array.isArray(data) ? data : Array.isArray(data?.tokens) ? data.tokens : [];
      const orphans = normalize({ computers: Array.isArray(data?.deleted_token_computers) ? data.deleted_token_computers : [] }).computers ?? [];
      return { tokens: tokens.map(normalize), orphans };
    },
    async create(req: CreateInstallTokenRequest): Promise<CreatedInstallToken> {
      const res = await fetch(url, {
        method: 'POST',
        headers: { ...auth, 'Content-Type': 'application/json' },
        body: JSON.stringify(req),
      });
      if (!res.ok) throw await failure(res, 'Could not create the install token');
      const data = await res.json();
      return { ...normalize(data), token: String(data.token ?? ''), install: parseInstallFetch(data.install) };
    },
    async revoke(id: string): Promise<void> {
      const res = await fetch(`${url}/${encodeURIComponent(id)}`, { method: 'DELETE', headers: auth });
      if (!res.ok && res.status !== 404) throw await failure(res, 'Could not revoke the install token');
    },
    /** Valid for `validHours` from now (POST .../{id}/extend). */
    async extend(id: string, validHours: number): Promise<InstallToken> {
      const res = await fetch(`${url}/${encodeURIComponent(id)}/extend`, {
        method: 'POST',
        headers: { ...auth, 'Content-Type': 'application/json' },
        body: JSON.stringify({ valid_hours: validHours }),
      });
      if (!res.ok) throw await failure(res, res.status === 404 || res.status === 405 ? 'This server is from before extending install tokens: update it.' : 'Could not extend the install token');
      return normalize(await res.json());
    },
    /** The token again, with how its command fetches the script (GET
     *  .../{id}/command); `renew`: a new secret for it first (POST), for a
     *  token whose secret was not kept. */
    async command(id: string, renew = false): Promise<CreatedInstallToken> {
      const res = await fetch(`${url}/${encodeURIComponent(id)}/command`, { method: renew ? 'POST' : 'GET', headers: auth, cache: 'no-store' });
      if (!res.ok) throw await failure(res, 'Could not read the install token');
      const data = await res.json();
      return { ...normalize(data), token: String(data.token ?? ''), install: parseInstallFetch(data.install) };
    },
    /** Removed from the list for good (DELETE .../{id}/forget). */
    async forget(id: string, withComputers = false): Promise<void> {
      const res = await fetch(`${url}/${encodeURIComponent(id)}/forget${withComputers ? '?with_computers=1' : ''}`, { method: 'DELETE', headers: auth });
      if (res.status === 405) throw new InstallTokenApiError('This server is from before deleting install tokens: update it.', 405);
      if (!res.ok && res.status !== 404) throw await failure(res, 'Could not delete the install token');
    },
  };
};
