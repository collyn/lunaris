// Two-factor sign-in with an authenticator app (server/src/mfa.rs): what the
// sign-in answers mean, and how codes and secrets are shown and typed.
// Pure functions (tests/mfa.test.ts).

/** What POST /api/auth/login (or /api/auth/login/mfa) answered. */
export type LoginAnswer =
  | { kind: 'session'; token: string; username: string; role: string | null }
  | { kind: 'mfa'; mfaToken: string; username: string }
  | { kind: 'error'; error: string };

export const parseLoginAnswer = (ok: boolean, data: unknown, fallbackError = 'Authentication failed'): LoginAnswer => {
  const d = (data && typeof data === 'object' ? data : {}) as Record<string, unknown>;
  if (ok && d.mfa_required === true && typeof d.mfa_token === 'string' && d.mfa_token) {
    return { kind: 'mfa', mfaToken: d.mfa_token, username: typeof d.username === 'string' ? d.username : '' };
  }
  if (ok && typeof d.token === 'string' && d.token) {
    return {
      kind: 'session',
      token: d.token,
      username: typeof d.username === 'string' ? d.username : '',
      role: typeof d.role === 'string' ? d.role : null,
    };
  }
  return { kind: 'error', error: typeof d.error === 'string' && d.error ? d.error : fallbackError };
};

/** A code from the app as typed or pasted: digits only, at most six. */
export const normalizeTotpInput = (text: string): string => text.replace(/\D/g, '').slice(0, 6);

export const isCompleteTotp = (text: string): boolean => /^\d{6}$/.test(text);

/** A recovery code as typed: what the server compares (it ignores case and dashes too). */
export const normalizeRecoveryInput = (text: string): string => text.trim().replace(/\s+/g, '');

export const isPlausibleRecoveryCode = (text: string): boolean =>
  text.replace(/[^0-9a-z]/gi, '').length >= 8;

/** The base32 secret in groups of four, for reading and typing into an app. */
export const groupSecret = (secret: string): string =>
  secret.replace(/\s+/g, '').replace(/(.{4})(?=.)/g, '$1 ');

/** The text of the recovery-codes file. */
export const recoveryCodesText = (
  codes: string[],
  account: string,
  server: string,
  when: Date,
  heading = 'Lunaris recovery codes',
  note = 'Each code signs you in once if you cannot use your authenticator app. Keep them somewhere safe.',
): string => {
  const date = when.toISOString().slice(0, 10);
  return [
    heading,
    `${account} @ ${server} (${date})`,
    '',
    note,
    '',
    ...codes.map((c, i) => `${String(i + 1).padStart(2, ' ')}. ${c}`),
    '',
  ].join('\n');
};

/** A file name for the recovery codes: lunaris-recovery-codes-<account>.txt */
export const recoveryCodesFileName = (account: string): string => {
  const safe = account.replace(/[^\p{L}\p{N}._-]+/gu, '-').replace(/^-+|-+$/g, '');
  return `lunaris-recovery-codes${safe ? `-${safe}` : ''}.txt`;
};

/**
 * The (English, translatable) message of an error answer. The server names
 * a wrong account password with the code "wrong_password", in `code` or as
 * the `error` itself.
 */
export const accountErrorMessage = (data: unknown, fallback: string): string => {
  const d = (data && typeof data === 'object' ? data : {}) as Record<string, unknown>;
  if (d.code === 'wrong_password' || d.error === 'wrong_password') return 'Wrong password';
  return typeof d.error === 'string' && d.error ? d.error : fallback;
};
