// One-line agent installation: the commands an administrator pastes into a
// terminal, and the state of each install token. Pure functions only, so they
// can be tested with node --test (tests/installCommand.test.ts).

export type InstallOs = 'linux' | 'windows';

export interface InstallOptions {
  /** Computer name; empty or missing means the agent uses the hostname. */
  name?: string;
  /** Skip the certificate check when downloading (self-signed server).
   *  Only used when the server gave no pin (`pin`). */
  insecure?: boolean;
  /** Where and how to fetch the script, as the server said when it made
   *  the token (POST /api/admin/install-tokens `install`). */
  fetch?: InstallFetch | null;
}

/** The server's own HTTPS listener: the script is fetched from
 *  `scriptOrigin`, and the connection is accepted only with this
 *  certificate (it is often self-signed, so the usual check can't be used):
 *  curl's `--pinnedpubkey` (SHA-256 of the public key, base64) and the
 *  certificate's SHA-256 (hex) in PowerShell. Without pins (a reverse
 *  proxy), the certificate is checked as usual. */
export interface InstallFetch {
  scriptOrigin: string | null;
  spkiSha256: string | null;
  certSha256: string | null;
}

const B64_SHA256 = /^[A-Za-z0-9+/]{43}=$/;
const HEX_SHA256 = /^[0-9a-fA-F]{64}$/;

/** The server's `install` answer, checked; null when it has none (an older
 *  server, or no way to fetch the script over HTTPS). */
export const parseInstallFetch = (raw: unknown): InstallFetch | null => {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Record<string, unknown>;
  const origin = typeof r.script_origin === 'string' && /^https:\/\/[A-Za-z0-9.:[\]-]+$/.test(r.script_origin) ? r.script_origin : null;
  if (!origin) return null;
  const spki = typeof r.spki_sha256 === 'string' && B64_SHA256.test(r.spki_sha256) ? r.spki_sha256 : null;
  const cert = typeof r.cert_sha256 === 'string' && HEX_SHA256.test(r.cert_sha256) ? r.cert_sha256.toUpperCase() : null;
  // Both pins or none: a command for one OS must not go unpinned.
  return spki && cert ? { scriptOrigin: origin, spkiSha256: spki, certSha256: cert } : { scriptOrigin: origin, spkiSha256: null, certSha256: null };
};


/** Quotes a value for bash inside single quotes ('it'\''s'). */
export const bashQuote = (value: string): string =>
  `'${value.replace(/'/g, `'\\''`)}'`;

/** Quotes a value for PowerShell inside single quotes ('it''s'). PowerShell
 *  also treats the typographic single quotes as quote characters, so those
 *  are doubled as well. */
export const psQuote = (value: string): string =>
  `'${value.replace(/['‘’‚‛]/g, (q) => q + q)}'`;

/** A URL without characters any shell would treat specially is left bare
 *  (easier to read); anything else gets quoted. */
const urlWord = (url: string, quote: (v: string) => string): string =>
  /^[A-Za-z0-9_.:/%@+=,~-]+$/.test(url) ? url : quote(url);

/** The server origin without a trailing slash, e.g. "https://lunaris.example". */
export const normalizeOrigin = (origin: string): string => origin.trim().replace(/\/+$/, '');

export const installScriptUrl = (os: InstallOs, origin: string): string =>
  `${normalizeOrigin(origin)}/install/${os === 'linux' ? 'agent.sh' : 'agent.ps1'}`;

/** Windows PowerShell: the class that accepts only the certificate whose
 *  SHA-256 (hex, as BitConverter writes it) it is given (see installCommand). */
export const WINDOWS_PIN_CLASS =
  "if (-not ('LunarisPin' -as [type])) { Add-Type 'public static class LunarisPin { static string pin; "
  + 'static bool Check(object s, System.Security.Cryptography.X509Certificates.X509Certificate c, '
  + 'System.Security.Cryptography.X509Certificates.X509Chain ch, System.Net.Security.SslPolicyErrors e) { '
  + 'using (var h = System.Security.Cryptography.SHA256.Create()) '
  + 'return System.BitConverter.ToString(h.ComputeHash(c.GetRawCertData())).Replace("-", "") == pin; } '
  + 'public static void Use(string sha256) { pin = sha256.ToUpperInvariant(); '
  + "System.Net.ServicePointManager.ServerCertificateValidationCallback = Check; } }' }; ";

/** The command to paste into a terminal: a root shell on Linux, PowerShell
 *  run as Administrator on Windows. */
export const installCommand = (
  os: InstallOs,
  origin: string,
  token: string,
  opts: InstallOptions = {},
): string => {
  const name = (opts.name ?? '').trim();
  const f = opts.fetch?.scriptOrigin ? opts.fetch : null;
  const url = installScriptUrl(os, f?.scriptOrigin ?? origin);
  const pinned = !!(f?.spkiSha256 && f.certSha256);
  if (os === 'linux') {
    // -k only skips the CA check: with --pinnedpubkey curl still refuses any
    // other key than this server's.
    const curl = pinned
      ? `curl -fsSLk --pinnedpubkey ${bashQuote(`sha256//${f!.spkiSha256}`)}`
      : opts.insecure ? 'curl -fsSLk' : 'curl -fsSL';
    let cmd = `${curl} ${urlWord(url, bashQuote)} | sudo LUNARIS_INSTALL_TOKEN=${bashQuote(token)} bash -s --`;
    if (name) cmd += ` --name ${bashQuote(name)}`;
    return cmd;
  }
  // Windows PowerShell 5.1 (.NET Framework). Older Windows offers only SSL 3 /
  // TLS 1.0 unless told otherwise, and the server (like GitHub) takes TLS 1.2
  // or 1.3: TLS 1.2 is added first (the script does so too, for its own
  // downloads).
  const tls = '[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12; ';
  // irm accepts the server's certificate only when its SHA-256 is this one
  // (the script turns the check back to normal for its own downloads). The
  // check is compiled C#: .NET calls it on a thread without a PowerShell
  // runspace, where a script block fails ("There is no Runspace available")
  // and irm only says "An unexpected error occurred on a send". Compiled once
  // per PowerShell window (Add-Type cannot add the class twice). Without a
  // pin `insecure` changes nothing here (no switch to skip the check); the
  // page explains the options.
  const check = pinned ? `${WINDOWS_PIN_CLASS}[LunarisPin]::Use(${psQuote(f!.certSha256!)}); ` : '';
  let cmd = `$env:LUNARIS_INSTALL_TOKEN=${psQuote(token)}; ${tls}${check}& ([scriptblock]::Create((irm ${urlWord(url, psQuote)})))`;
  if (name) cmd += ` -Name ${psQuote(name)}`;
  return cmd;
};

// ---- Tokens ---------------------------------------------------------------

export interface InstallToken {
  id: string;
  label: string;
  created_at: string | number | null;
  expires_at: string | number | null;
  max_uses: number | null;
  uses: number;
  revoked: boolean;
}

export type TokenStatus = 'active' | 'expired' | 'used_up' | 'revoked';

/** Milliseconds since the epoch, or null. Accepts unix seconds (or ms),
 *  ISO 8601 strings, and SQLite's "YYYY-MM-DD HH:MM:SS" which is UTC. */
export const parseTimestamp = (value: unknown): number | null => {
  if (value === null || value === undefined || value === '') return null;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) return null;
    return value < 1e12 ? value * 1000 : value; // seconds vs milliseconds
  }
  if (typeof value !== 'string') return null;
  const s = value.trim();
  if (/^\d+(\.\d+)?$/.test(s)) return parseTimestamp(Number(s));
  // A date and time without a zone: the server stores UTC.
  const m = /^(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2}(:\d{2}(\.\d+)?)?)$/.exec(s);
  const ms = Date.parse(m ? `${m[1]}T${m[2]}Z` : s);
  return Number.isNaN(ms) ? null : ms;
};

/** Revoked wins, then expired, then used up. */
export const tokenStatus = (tok: Pick<InstallToken, 'expires_at' | 'max_uses' | 'uses' | 'revoked'>, now: number = Date.now()): TokenStatus => {
  if (tok.revoked) return 'revoked';
  const exp = parseTimestamp(tok.expires_at);
  if (exp !== null && exp <= now) return 'expired';
  if (tok.max_uses !== null && tok.max_uses !== undefined && tok.uses >= tok.max_uses) return 'used_up';
  return 'active';
};

/** "Valid for" choices in the create form, in hours. */
export const VALIDITY_CHOICES: { hours: number; label: string }[] = [
  { hours: 1, label: '1 hour' },
  { hours: 24, label: '24 hours' },
  { hours: 24 * 7, label: '7 days' },
  { hours: 24 * 30, label: '30 days' },
];

/** The "max computers" field: empty means unlimited (null); otherwise a
 *  whole number of at least 1, or undefined when it is not valid. */
export const parseMaxUses = (text: string): number | null | undefined => {
  const s = text.trim();
  if (!s) return null;
  if (!/^\d+$/.test(s)) return undefined;
  const n = Number(s);
  return n >= 1 && Number.isSafeInteger(n) ? n : undefined;
};

/** A rough relative time: [amount, unit], unit one of minute/hour/day, and
 *  the sign tells past (<0) from future (>0). Under a minute gives 0. */
export const relativeParts = (target: number, now: number = Date.now()): { amount: number; unit: 'minute' | 'hour' | 'day'; future: boolean } => {
  const diff = target - now;
  const abs = Math.abs(diff);
  const min = 60_000, hour = 60 * min, day = 24 * hour;
  if (abs >= day) return { amount: Math.round(abs / day), unit: 'day', future: diff > 0 };
  if (abs >= hour) return { amount: Math.round(abs / hour), unit: 'hour', future: diff > 0 };
  return { amount: Math.round(abs / min), unit: 'minute', future: diff > 0 };
};
