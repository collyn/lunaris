// The server's information page (src/serverInfo.ts).
// Run: npm run test:server-info
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DEFAULT_LINKS, gatewayRunning, parseGateway, parseServerInfo, webVersion } from '../src/serverInfo.ts';

test('what everyone gets', () => {
  const info = parseServerInfo({ version: '0.6.6', links: { website: 'https://example.test/', issues: 'javascript:alert(1)' } })!;
  assert.equal(info.version, '0.6.6');
  assert.equal(info.links.website, 'https://example.test/');
  // Only http(s) links.
  assert.equal(info.links.issues, DEFAULT_LINKS.issues);
  assert.equal(info.update, undefined);
  assert.equal(info.dataDir, undefined);
});

test('what administrators get', () => {
  const info = parseServerInfo({
    version: '0.6.6',
    update: { checks: true, checked_at: 100, latest: '0.7.0', url: 'https://example.test/r', available: true },
    ports: { http: 8090, https: null },
    data_dir: '/var/lib/lunaris-server',
  })!;
  assert.deepEqual(info.update, { checks: true, checkedAt: 100, latest: '0.7.0', url: 'https://example.test/r', available: true, error: undefined });
  assert.deepEqual(info.ports, { http: 8090, https: null });
  assert.equal(info.dataDir, '/var/lib/lunaris-server');
  assert.equal(parseServerInfo({ update: { checks: 'yes', url: 'file:///x' } })!.update!.checks, false);
  assert.equal(parseServerInfo({ update: { url: 'file:///x' } })!.update!.url, undefined);
});

test('nonsense', () => {
  assert.equal(parseServerInfo(null), null);
  assert.equal(parseServerInfo([1]), null);
  assert.equal(parseServerInfo('x'), null);
  // Outside a vite build there is no version.
  assert.equal(webVersion(), '');
});

test('the server gateway', () => {
  const info = parseServerInfo({
    version: '0.6.7',
    gateway: { enabled: true, forced: null, state: 'running', error: null, program: '/usr/bin/lunaris-agent', host_id: 'GW', failures: 0 },
  })!;
  assert.deepEqual(info.gateway, { enabled: true, forced: undefined, state: 'running', error: undefined, program: '/usr/bin/lunaris-agent', hostId: 'GW', failures: 0 });
  assert.ok(gatewayRunning(info.gateway));
  const off = parseGateway({ enabled: false, forced: 'off', state: 'disabled' })!;
  assert.equal(off.forced, 'off');
  assert.equal(gatewayRunning(off), false);
  assert.equal(parseGateway({ enabled: true, state: 'failing', error: 'the gateway agent ended (exit status: 1)', failures: 3 })!.failures, 3);
  // Not one it knows: none.
  assert.equal(parseGateway({ state: 'exploded' }), undefined);
  assert.equal(parseServerInfo({ version: '1' })!.gateway, undefined);
  assert.equal(gatewayRunning(undefined), false);
});

test('how to update follows how the server was installed', async () => {
  const { updateSteps, windowsInstallerUrl, parseServerInfo } = await import('../src/serverInfo.ts');
  const deb = updateSteps('deb', 'x86_64', '0.8.1');
  assert.match(deb[0].command!, /releases\/download\/v0\.8\.1\/lunaris-server_0\.8\.1_amd64\.deb/);
  assert.match(deb[0].command!, /sudo apt install \.\/lunaris-server_0\.8\.1_amd64\.deb/);
  assert.match(updateSteps('deb', 'aarch64', 'v0.8.1')[0].command!, /_arm64\.deb/);
  const service = updateSteps('service', 'x86_64', '0.8.1')[0].command!;
  assert.match(service, /v0\.8\.1\/lunaris-server-linux-x86_64\.tar\.gz \| tar xz/);
  assert.match(service, /sudo \.\/lunaris-server-linux-x86_64\/lunaris-server install/);
  assert.match(updateSteps('docker', 'x86_64', '0.8.1')[0].command!, /git checkout v0\.8\.1\ndocker compose up -d --build/);
  assert.equal(updateSteps('windows', 'x86_64', '0.8.1')[0].command, undefined);
  assert.equal(windowsInstallerUrl('0.8.1'), 'https://github.com/collyn/lunaris/releases/download/v0.8.1/lunaris-server-installer-windows.exe');
  assert.deepEqual(parseServerInfo({ version: '0.8.1', install: { kind: 'service', arch: 'aarch64' } })!.install, { kind: 'service', arch: 'aarch64' });
  assert.equal(parseServerInfo({ version: '0.8.1', install: { kind: 'other' } })!.install, undefined, 'an unknown kind is left out');
});
