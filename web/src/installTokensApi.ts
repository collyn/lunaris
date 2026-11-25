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
    async list(): Promise<InstallToken[]> {
      const res = await fetch(url, { headers: auth });
      if (!res.ok) throw await failure(res, 'Could not load the install tokens');
      const data = await res.json();
      return Array.isArray(data) ? data.map(normalize) : [];
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
  };
};
