// First-start setup helpers (src/setup.ts).
// Run: npm run test:setup   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseSetupStatus, setupFormError, SETUP_CODE_PLACES } from '../src/setup.ts';

test('only a clear "needed" shows the setup page', () => {
  assert.equal(parseSetupStatus(true, { needed: true }), true);
  assert.equal(parseSetupStatus(true, { needed: false }), false);
  assert.equal(parseSetupStatus(false, { needed: true }), false, 'an error answer');
  assert.equal(parseSetupStatus(true, null), false);
  assert.equal(parseSetupStatus(true, 'needed'), false);
  assert.equal(parseSetupStatus(true, { needed: 'yes' }), false);
});

test('the form needs a code, a name and two matching passwords', () => {
  assert.equal(setupFormError('', 'boss', 'secret1', 'secret1'), 'Enter the setup code');
  assert.equal(setupFormError(' - ', 'boss', 'secret1', 'secret1'), 'Enter the setup code');
  assert.equal(setupFormError('abc', '  ', 'secret1', 'secret1'), 'Username is required');
  assert.equal(setupFormError('abc', 'boss', '123456789', '123456789'), 'Password must be at least 10 characters long');
  assert.equal(setupFormError('abc', 'boss', 'secret12345', 'secret12346'), 'The passwords do not match');
  assert.equal(setupFormError('abc', 'boss', 'secret12345', 'secret12345'), null);
});

test('the hint names the packages\' paths', () => {
  const paths = SETUP_CODE_PLACES.map(p => p.path);
  assert.ok(paths.includes('/var/lib/lunaris/setup_code.txt'));
  assert.ok(paths.includes('%ProgramData%\\Lunaris\\Server\\setup_code.txt'));
});

test('a new account password is at least 10 characters', async () => {
  const { newPasswordError, MIN_ACCOUNT_PASSWORD } = await import('../src/setup.ts');
  assert.equal(MIN_ACCOUNT_PASSWORD, 10);
  assert.equal(newPasswordError('123456789'), 'Password must be at least 10 characters long');
  assert.equal(newPasswordError('1234567890'), null);
});
