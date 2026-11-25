// One-line agent installation (src/installCommand.ts): the commands and the
// state of each install token.
// Run: npm run test:install   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import {
  bashQuote,
  installCommand,
  installScriptUrl,
  parseInstallFetch,
  parseMaxUses,
  parseTimestamp,
  psQuote,
  relativeParts,
  tokenStatus,
  WINDOWS_PIN_CLASS,
} from '../src/installCommand.ts';

const TOKEN = 'abc_DEF-123';

test('linux command, no name', () => {
  assert.equal(
    installCommand('linux', 'https://lunaris.example', TOKEN),
    "curl -fsSL https://lunaris.example/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN='abc_DEF-123' bash -s --",
  );
});

test('linux command with name and self-signed certificate', () => {
  assert.equal(
    installCommand('linux', 'http://10.0.0.5:8090/', TOKEN, { name: 'PC 01', insecure: true }),
    "curl -fsSLk http://10.0.0.5:8090/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN='abc_DEF-123' bash -s -- --name 'PC 01'",
  );
});

test('windows command, no name', () => {
  assert.equal(
    installCommand('windows', 'https://lunaris.example', TOKEN),
    "$env:LUNARIS_INSTALL_TOKEN='abc_DEF-123'; [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12; & ([scriptblock]::Create((irm https://lunaris.example/install/agent.ps1)))",
  );
});

test('windows command with name; insecure changes nothing there', () => {
  const a = installCommand('windows', 'https://h', TOKEN, { name: "Bob's PC", insecure: true });
  assert.equal(a, "$env:LUNARIS_INSTALL_TOKEN='abc_DEF-123'; [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12; & ([scriptblock]::Create((irm https://h/install/agent.ps1))) -Name 'Bob''s PC'");
  assert.equal(a, installCommand('windows', 'https://h', TOKEN, { name: "Bob's PC" }));
});

const SPKI = 'n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=';
const CERT = 'ab'.repeat(32);
const PINNED = parseInstallFetch({ script_origin: 'https://10.0.0.5:8443', spki_sha256: SPKI, cert_sha256: CERT });

test('the server\'s install answer is checked', () => {
  assert.deepEqual(PINNED, { scriptOrigin: 'https://10.0.0.5:8443', spkiSha256: SPKI, certSha256: CERT.toUpperCase() });
  assert.equal(parseInstallFetch(undefined), null, 'an older server');
  assert.equal(parseInstallFetch({ script_origin: null }), null, 'no HTTPS');
  assert.equal(parseInstallFetch({ script_origin: 'http://h' }), null);
  assert.equal(parseInstallFetch({ script_origin: "https://h'; id" }), null);
  // A proxy: an origin, its certificate checked as usual.
  assert.deepEqual(parseInstallFetch({ script_origin: 'https://remote.example', spki_sha256: null, cert_sha256: null }),
    { scriptOrigin: 'https://remote.example', spkiSha256: null, certSha256: null });
  // A bad or half pin is no pin.
  assert.equal(parseInstallFetch({ script_origin: 'https://h', spki_sha256: "x'", cert_sha256: CERT })?.spkiSha256, null);
  assert.equal(parseInstallFetch({ script_origin: 'https://h', spki_sha256: SPKI, cert_sha256: 'zz' })?.certSha256, null);
});

test('linux command pinned to the server\'s key, from its HTTPS port', () => {
  // The page itself may be on plain HTTP.
  assert.equal(
    installCommand('linux', 'http://10.0.0.5:8090', TOKEN, { fetch: PINNED, insecure: true }),
    `curl -fsSLk --pinnedpubkey 'sha256//${SPKI}' https://10.0.0.5:8443/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN='abc_DEF-123' bash -s --`,
  );
});

test('windows command accepts only the server\'s certificate', () => {
  const cmd = installCommand('windows', 'http://10.0.0.5:8090', TOKEN, { fetch: PINNED, name: 'PC' });
  assert.equal(
    cmd,
    "$env:LUNARIS_INSTALL_TOKEN='abc_DEF-123'; [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12; "
      + WINDOWS_PIN_CLASS + "[LunarisPin]::Use('" + CERT.toUpperCase() + "'); & ([scriptblock]::Create((irm https://10.0.0.5:8443/install/agent.ps1))) -Name 'PC'",
  );
  // A compiled check, not a script block (no runspace on .NET's thread).
  assert.ok(!cmd.includes('ServerCertificateValidationCallback = {'));
  assert.ok(WINDOWS_PIN_CLASS.startsWith("if (-not ('LunarisPin' -as [type])) { Add-Type '"));
  // Single-quoted for PowerShell: no single quote inside the C#.
  const csharp = WINDOWS_PIN_CLASS.slice(WINDOWS_PIN_CLASS.indexOf("Add-Type '") + 10, WINDOWS_PIN_CLASS.lastIndexOf("' }"));
  assert.ok(!csharp.includes("'"), csharp);
});

test('through a proxy: its https origin, checked as usual', () => {
  const fetch = parseInstallFetch({ script_origin: 'https://remote.example', spki_sha256: null, cert_sha256: null });
  assert.equal(
    installCommand('linux', 'https://remote.example', TOKEN, { fetch }),
    "curl -fsSL https://remote.example/install/agent.sh | sudo LUNARIS_INSTALL_TOKEN='abc_DEF-123' bash -s --",
  );
});

test('a pinned command survives a real bash parse', () => {
  const cmd = installCommand('linux', 'http://h', TOKEN, { fetch: PINNED, name: "it's" });
  execFileSync('bash', ['-n', '-c', cmd]);
});

test('a name of only spaces is left out', () => {
  assert.ok(!installCommand('linux', 'https://h', TOKEN, { name: '   ' }).includes('--name'));
  assert.ok(!installCommand('windows', 'https://h', TOKEN, { name: '  ' }).includes('-Name'));
});

test('script URLs', () => {
  assert.equal(installScriptUrl('linux', 'https://h//'), 'https://h/install/agent.sh');
  assert.equal(installScriptUrl('windows', ' https://h '), 'https://h/install/agent.ps1');
});

test('bash quoting survives a real bash', () => {
  for (const v of ["plain", "it's", "'''", "a b $HOME `id` \"x\" \\ ;|&", ""]) {
    const out = execFileSync('bash', ['-c', `printf %s ${bashQuote(v)}`]).toString();
    assert.equal(out, v);
  }
});

test('powershell quoting doubles every single quote, typographic ones too', () => {
  assert.equal(psQuote("it's"), "'it''s'");
  assert.equal(psQuote('a’b'), "'a’’b'");
  assert.equal(psQuote('$x `y'), "'$x `y'");
});

test('a URL with odd characters is quoted', () => {
  assert.equal(
    installCommand('linux', "https://h/a b", 'T'),
    "curl -fsSL 'https://h/a b/install/agent.sh' | sudo LUNARIS_INSTALL_TOKEN='T' bash -s --",
  );
});

test('timestamps: seconds, ms, ISO, SQLite UTC', () => {
  const iso = Date.UTC(2026, 10, 18, 10, 0, 0);
  assert.equal(parseTimestamp(iso / 1000), iso);
  assert.equal(parseTimestamp(iso), iso);
  assert.equal(parseTimestamp(String(iso / 1000)), iso);
  assert.equal(parseTimestamp('2026-11-18T10:00:00Z'), iso);
  assert.equal(parseTimestamp('2026-11-18T12:00:00+02:00'), iso);
  assert.equal(parseTimestamp('2026-11-18 10:00:00'), iso);
  assert.equal(parseTimestamp('2026-11-18T10:00:00'), iso);
  assert.equal(parseTimestamp(null), null);
  assert.equal(parseTimestamp(''), null);
  assert.equal(parseTimestamp('soon'), null);
});

test('token status', () => {
  const now = Date.UTC(2026, 10, 18);
  const base = { expires_at: (now + 3600_000) / 1000, max_uses: null, uses: 0, revoked: false };
  assert.equal(tokenStatus(base, now), 'active');
  assert.equal(tokenStatus({ ...base, max_uses: 5, uses: 4 }, now), 'active');
  assert.equal(tokenStatus({ ...base, max_uses: 5, uses: 5 }, now), 'used_up');
  assert.equal(tokenStatus({ ...base, expires_at: now / 1000 }, now), 'expired');
  assert.equal(tokenStatus({ ...base, expires_at: '2020-01-01 00:00:00' }, now), 'expired');
  assert.equal(tokenStatus({ ...base, expires_at: null }, now), 'active');
  // Revoked wins over everything, expired over used up.
  assert.equal(tokenStatus({ ...base, revoked: true, expires_at: 0, max_uses: 1, uses: 1 }, now), 'revoked');
  assert.equal(tokenStatus({ ...base, expires_at: 1, max_uses: 1, uses: 1 }, now), 'expired');
});

test('max computers field', () => {
  assert.equal(parseMaxUses(''), null);
  assert.equal(parseMaxUses('  '), null);
  assert.equal(parseMaxUses('25'), 25);
  assert.equal(parseMaxUses('0'), undefined);
  assert.equal(parseMaxUses('-3'), undefined);
  assert.equal(parseMaxUses('2.5'), undefined);
  assert.equal(parseMaxUses('abc'), undefined);
});

test('relative time', () => {
  const now = 1_000_000_000_000;
  assert.deepEqual(relativeParts(now + 3 * 86400_000, now), { amount: 3, unit: 'day', future: true });
  assert.deepEqual(relativeParts(now - 2 * 3600_000, now), { amount: 2, unit: 'hour', future: false });
  assert.deepEqual(relativeParts(now + 5 * 60_000, now), { amount: 5, unit: 'minute', future: true });
});

test('an installed agent is joined with one command, nothing downloaded', async () => {
  const { enrollCommand } = await import('../src/installCommand.ts');
  const pinned = { scriptOrigin: 'https://srv.example:8443', spkiSha256: 'x'.repeat(43) + '=', certSha256: 'AB'.repeat(32) };
  const desktop = enrollCommand('linux-desktop', 'http://srv.example:8090', 'lit_abc', { fetch: pinned });
  assert.equal(desktop, `LUNARIS_INSTALL_TOKEN='lit_abc' lunaris-agent --enroll --server 'https://srv.example:8443' --server-cert-sha256 ${'AB'.repeat(32)}`);
  assert.ok(!/curl|irm|sudo/.test(desktop), 'no download, no root for the desktop agent');
  const headless = enrollCommand('linux-headless', 'http://srv.example:8090', 'lit_abc', { fetch: pinned, name: "Bob's PC" });
  assert.match(headless, /^sudo env LUNARIS_INSTALL_TOKEN='lit_abc' lunaris-agent --system --enroll --server 'https:\/\/srv\.example:8443'/);
  assert.match(headless, / --name 'Bob'\\''s PC'$/);
  const win = enrollCommand('windows', 'http://srv.example:8090', "lit_a'b", { fetch: pinned, name: 'PC 1' });
  assert.match(win, /^\$env:LUNARIS_INSTALL_TOKEN='lit_a''b'; \$a = Join-Path \$env:ProgramFiles 'Lunaris Agent\\lunaris-agent\.exe'; & \$a --system --enroll --server 'https:\/\/srv\.example:8443' --server-cert-sha256 [A-F0-9]{64} --name 'PC 1'; if \(\$LASTEXITCODE -eq 0\) \{ & \$a --install-service \}; Remove-Item Env:\\LUNARIS_INSTALL_TOKEN$/);
  // A server without HTTPS: the page's own address, no certificate.
  assert.equal(enrollCommand('linux-desktop', 'http://srv.example:8090/', 't', {}), `LUNARIS_INSTALL_TOKEN='t' lunaris-agent --enroll --server 'http://srv.example:8090'`);
  // A trusted certificate: HTTPS without a pin.
  assert.ok(!enrollCommand('windows', 'x', 't', { fetch: { scriptOrigin: 'https://a.example', spkiSha256: null, certSha256: null } }).includes('cert-sha256'));
});
