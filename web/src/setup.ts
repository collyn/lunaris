// First start of a server (server/src/setup.rs): no account yet, the first
// admin is made with the one-time setup code the server wrote to a file.
// Pure functions (tests/setup.test.ts).

/** Where the server keeps the code, per kind of install (shown as a hint). */
export const SETUP_CODE_PLACES: { label: string; path: string }[] = [
  { label: 'Linux packages', path: '/var/lib/lunaris/setup_code.txt' },
  { label: 'Windows', path: '%ProgramData%\\Lunaris\\Server\\setup_code.txt' },
  { label: 'Other installs', path: 'setup_code.txt in the server\'s data folder' },
];

/** What GET /api/setup answered: true only for a clear "needed". Older
 *  servers (no such endpoint) and errors show the sign-in page. */
export const parseSetupStatus = (ok: boolean, data: unknown): boolean =>
  ok && !!data && typeof data === 'object' && (data as Record<string, unknown>).needed === true;

/** Why the form can't be sent yet (an English key for t()), or null. The
 *  server checks the same, and the code. */
export const setupFormError = (code: string, username: string, password: string, confirm: string): string | null => {
  if (!code.replace(/[^0-9a-z]/gi, '')) return 'Enter the setup code';
  if (!username.trim()) return 'Username is required';
  if (password.length < 6) return 'Password must be at least 6 characters long';
  if (password !== confirm) return 'The passwords do not match';
  return null;
};
