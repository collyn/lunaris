// Agent versions and how each agent is updated (src/agentUpdate.ts).
// Run: npm run test:agent-update   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { agentUpdateHow, agentUpdateState, compareVersions, installKindLabel } from '../src/agentUpdate.ts';
import { updateCommand } from '../src/installCommand.ts';

test('versions compare by number, a pre-release before its release', () => {
  assert.ok(compareVersions('0.7.5', '0.7.6') < 0);
  assert.ok(compareVersions('0.7.10', '0.7.9') > 0);
  assert.ok(compareVersions('1.0.0', '0.99.99') > 0);
  assert.equal(compareVersions('v0.7.6', '0.7.6'), 0);
  assert.ok(compareVersions('0.8.0-rc.1', '0.8.0') < 0);
  assert.ok(compareVersions('0.8.0-rc.2', '0.8.0-rc.10') < 0);
  // Not a version: no guess.
  assert.equal(compareVersions('dev', '0.7.6'), 0);
});

test('an agent older than the server has an update', () => {
  const build = { version: '0.7.5', os: 'linux', install: 'deb' };
  assert.equal(agentUpdateState(build, '0.7.6'), 'older');
  assert.equal(agentUpdateState(build, '0.7.5'), 'current');
  assert.equal(agentUpdateState(build, '0.7.4'), 'newer');
  assert.equal(agentUpdateState(undefined, '0.7.6'), 'unknown');
  assert.equal(agentUpdateState(build, null), 'unknown');
});

test('the update commands need no token', () => {
  const linux = updateCommand('linux', 'https://lunaris.example/');
  assert.equal(linux, 'curl -fsSL https://lunaris.example/install/agent.sh | sudo bash -s -- --update');
  const win = updateCommand('windows', 'https://lunaris.example');
  assert.ok(win.endsWith('& ([scriptblock]::Create((irm https://lunaris.example/install/agent.ps1))) -Update'), win);
  assert.ok(!win.includes('LUNARIS_INSTALL_TOKEN') && !linux.includes('LUNARIS_INSTALL_TOKEN'));
  // Pinned to the server's own certificate, like the install command.
  const fetch = { scriptOrigin: 'https://10.0.0.5:8443', spkiSha256: `${'A'.repeat(43)}=`, certSha256: 'AB'.repeat(32) };
  assert.ok(updateCommand('linux', 'http://10.0.0.5:8090', { fetch }).startsWith(`curl -fsSLk --pinnedpubkey 'sha256//${'A'.repeat(43)}=' https://10.0.0.5:8443/install/agent.sh |`));
  assert.ok(updateCommand('windows', 'http://10.0.0.5:8090', { fetch }).includes(`[LunarisPin]::Use('${'AB'.repeat(32)}')`));
});

test('each kind of install is updated its own way', () => {
  const how = (install: string) => agentUpdateHow({ version: '0.7.5', os: install === 'windows' ? 'windows' : 'linux', install }, 'https://s');
  assert.deepEqual(how('deb'), { command: 'curl -fsSL https://s/install/agent.sh | sudo bash -s -- --update', shell: 'root' });
  assert.equal((how('appimage') as { shell: string }).shell, 'root');
  assert.equal((how('windows') as { shell: string }).shell, 'admin-powershell');
  assert.ok('howTo' in how('package') && 'howTo' in how('manual') && 'howTo' in how('something-new'));
  assert.equal(installKindLabel('deb'), '.deb package');
  assert.equal(installKindLabel('what'), 'By hand');
});

test('"Update now" is offered only to an older agent that says it can, while online', async () => {
  const { canUpdateFromWeb, updateRefusalText } = await import('../src/agentUpdate.ts');
  const win = (over: object = {}) => ({ version: '0.8.3', os: 'windows', install: 'windows', remote_update: true, ...over });
  assert.equal(canUpdateFromWeb(win(), '0.8.4', true), true);
  assert.equal(canUpdateFromWeb(win(), '0.8.4', false), false, 'offline');
  assert.equal(canUpdateFromWeb(win(), '0.8.3', true), false, 'current');
  assert.equal(canUpdateFromWeb(win(), '0.8.2', true), false, 'newer than the server');
  assert.equal(canUpdateFromWeb(win({ remote_update: false }), '0.8.4', true), false);
  // Agents from before 0.8.3 say nothing; anything but true is no.
  assert.equal(canUpdateFromWeb(win({ remote_update: undefined }), '0.8.4', true), false);
  assert.equal(canUpdateFromWeb(win({ remote_update: 'yes' }) as never, '0.8.4', true), false);
  assert.equal(canUpdateFromWeb(null, '0.8.4', true), false);
  assert.equal(canUpdateFromWeb(win(), null, true), false);
  // A known code has its own words; else what the computer said; else a general line.
  assert.match(updateRefusalText('not_allowed', 'x'), /does not allow/);
  assert.match(updateRefusalText('agent_too_old', undefined), /before updates from the web/);
  assert.equal(updateRefusalText('failed', 'The release signature is not valid: nothing is installed.'), 'The release signature is not valid: nothing is installed.');
  assert.equal(updateRefusalText(undefined, undefined), 'The update could not be started.');
});
