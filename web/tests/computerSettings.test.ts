// A computer's own settings as the web edits them (src/computerSettings.ts).
// Run: npm run test:computer-settings   (node --test, no packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { portProblem, settingsChanges, type ComputerSettings } from '../src/computerSettings.ts';

const loaded: ComputerSettings = {
  remote_changes: true, direct_enabled: true, direct_port: 8091, has_password: true,
  approve_mode: 'password-click', server_sessions: 'trusted', allow_clipboard: true, allow_file_transfer: true,
  file_access: 'home', rustdesk_enabled: false, rustdesk_port: 21118, rustdesk_file_transfer: false,
  wayland_capture: 'auto', input_ready: true, version: '0.6.7', os: 'linux',
};

test('only what changed is sent', () => {
  assert.deepEqual(settingsChanges(loaded, { ...loaded }), {});
  assert.deepEqual(
    settingsChanges(loaded, { ...loaded, direct_port: 9000, allow_clipboard: false, wayland_capture: 'portal' }),
    { direct_port: 9000, allow_clipboard: false, wayland_capture: 'portal' },
  );
});

test('read-only values and the switch are never sent', () => {
  const draft = { ...loaded, remote_changes: false, has_password: false, input_ready: false, version: 'x', os: 'y', linked_by_address: true };
  assert.deepEqual(settingsChanges(loaded, draft), {});
});

test('ports are 1024 to 65535', () => {
  for (const ok of [1024, 8091, 65535]) assert.equal(portProblem(ok), false, String(ok));
  for (const bad of [0, 80, 1023, 65536, NaN, 8091.5]) assert.equal(portProblem(bad), true, String(bad));
});
