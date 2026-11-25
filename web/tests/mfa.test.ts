// Two-factor sign-in helpers (src/mfa.ts).
// Run: npm run test:mfa   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  groupSecret,
  isCompleteTotp,
  isPlausibleRecoveryCode,
  normalizeRecoveryInput,
  normalizeTotpInput,
  parseLoginAnswer,
  recoveryCodesFileName,
  recoveryCodesText,
} from '../src/mfa.ts';

test('a normal sign-in answer is a session (old servers too)', () => {
  assert.deepEqual(parseLoginAnswer(true, { token: 't', username: 'alice', role: 'admin' }), {
    kind: 'session', token: 't', username: 'alice', role: 'admin',
  });
  assert.deepEqual(parseLoginAnswer(true, { token: 't', username: 'bob' }), {
    kind: 'session', token: 't', username: 'bob', role: null,
  });
});

test('mfa_required asks for the code and never counts as a session', () => {
  const a = parseLoginAnswer(true, { mfa_required: true, mfa_token: 'm', username: 'alice', expires_in: 300 });
  assert.deepEqual(a, { kind: 'mfa', mfaToken: 'm', username: 'alice' });
  // A broken answer is an error, not a session.
  assert.equal(parseLoginAnswer(true, { mfa_required: true }).kind, 'error');
  assert.equal(parseLoginAnswer(true, {}).kind, 'error');
  assert.equal(parseLoginAnswer(true, null).kind, 'error');
});

test('errors carry the server text', () => {
  assert.deepEqual(parseLoginAnswer(false, { error: 'Wrong code' }), { kind: 'error', error: 'Wrong code' });
  assert.deepEqual(parseLoginAnswer(false, 'x', 'Failed'), { kind: 'error', error: 'Failed' });
  // Not ok with a token-looking body is still an error.
  assert.equal(parseLoginAnswer(false, { token: 't' }).kind, 'error');
});

test('codes as typed or pasted', () => {
  assert.equal(normalizeTotpInput('123 456'), '123456');
  assert.equal(normalizeTotpInput(' 12-34-56 78'), '123456');
  assert.equal(normalizeTotpInput('abc'), '');
  assert.ok(isCompleteTotp('000000'));
  assert.ok(!isCompleteTotp('12345'));
  assert.ok(!isCompleteTotp('1234567'));
  assert.equal(normalizeRecoveryInput('  ab12-cd34 ef56 '), 'ab12-cd34ef56');
  assert.ok(isPlausibleRecoveryCode('ab12-cd34-ef56'));
  assert.ok(!isPlausibleRecoveryCode('ab-12'));
});

test('secret in groups of four', () => {
  assert.equal(groupSecret('JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP'), 'JBSW Y3DP EHPK 3PXP JBSW Y3DP EHPK 3PXP');
  assert.equal(groupSecret('ABCDEF'), 'ABCD EF');
  assert.equal(groupSecret('ABCD'), 'ABCD');
  assert.equal(groupSecret(''), '');
});

test('recovery codes file', () => {
  const text = recoveryCodesText(['aaaa-bbbb-cccc', 'dddd-eeee-ffff'], 'alice', 'lunaris.lan:8090', new Date('2026-11-19T10:00:00Z'));
  const lines = text.split('\n');
  assert.equal(lines[0], 'Lunaris recovery codes');
  assert.equal(lines[1], 'alice @ lunaris.lan:8090 (2026-11-19)');
  assert.ok(lines.includes(' 1. aaaa-bbbb-cccc'));
  assert.ok(lines.includes(' 2. dddd-eeee-ffff'));
  assert.equal(recoveryCodesFileName('alice'), 'lunaris-recovery-codes-alice.txt');
  assert.equal(recoveryCodesFileName('Nguyễn Văn/A'), 'lunaris-recovery-codes-Nguyễn-Văn-A.txt');
  assert.equal(recoveryCodesFileName('//'), 'lunaris-recovery-codes.txt');
});

test('a wrong account password is named, other errors pass through', async () => {
  const { accountErrorMessage } = await import('../src/mfa.ts');
  assert.equal(accountErrorMessage({ error: 'wrong_password' }, 'Could not start'), 'Wrong password');
  assert.equal(accountErrorMessage({ error: 'Nope', code: 'wrong_password' }, 'Could not start'), 'Wrong password');
  assert.equal(accountErrorMessage({ error: 'Too many attempts' }, 'Could not start'), 'Too many attempts');
  assert.equal(accountErrorMessage({}, 'Could not start'), 'Could not start');
  assert.equal(accountErrorMessage(null, 'Could not start'), 'Could not start');
});
