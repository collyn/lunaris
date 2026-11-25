// What GET /api/server/info answers (server/src/server_info.rs), checked.
// Pure functions (tests/serverInfo.test.ts).

export type ServerLinks = { website: string; issues: string; releases: string };

export type ServerInfo = {
  version: string;
  links: ServerLinks;
  /** Administrators only. */
  update?: { checks: boolean; checkedAt?: number; latest?: string; url?: string; available?: boolean; error?: string };
  ports?: { http: number | null; https: number | null };
  dataDir?: string;
  /** Administrators only: the server gateway (server/src/gateway.rs). */
  gateway?: GatewayInfo;
  /** Administrators only: how this server was installed (server_info.rs install_kind_of). */
  install?: { kind: InstallKind; arch: string };
};

export type InstallKind = 'docker' | 'windows' | 'deb' | 'package' | 'service' | 'manual';
const INSTALL_KINDS: InstallKind[] = ['docker', 'windows', 'deb', 'package', 'service', 'manual'];

export type GatewayState = 'disabled' | 'not_found' | 'unsupported' | 'starting' | 'running' | 'failing';

/** The agent the server runs itself to reach RustDesk, Sunshine, RDP and
 *  VNC computers (GET /api/server/info, GET|PUT /api/admin/server/gateway). */
export type GatewayInfo = {
  /** Its switch. */
  enabled: boolean;
  /** LUNARIS_GATEWAY=on|off in the server's environment decides instead of the switch. */
  forced?: 'on' | 'off';
  state: GatewayState;
  error?: string;
  /** The lunaris-agent it runs (or found too old). */
  program?: string;
  hostId?: string;
  failures: number;
};

const GATEWAY_STATES: GatewayState[] = ['disabled', 'not_found', 'unsupported', 'starting', 'running', 'failing'];

export function parseGateway(body: unknown): GatewayInfo | undefined {
  if (!body || typeof body !== 'object' || Array.isArray(body)) return undefined;
  const g = body as Record<string, unknown>;
  const state = GATEWAY_STATES.find(s => s === g.state);
  if (!state) return undefined;
  return {
    enabled: g.enabled === true,
    forced: g.forced === 'on' || g.forced === 'off' ? g.forced : undefined,
    state,
    error: str(g.error),
    program: str(g.program),
    hostId: str(g.host_id),
    failures: num(g.failures) ?? 0,
  };
}

/** Whether the gateway offers a way to add computers of other types now. */
export const gatewayRunning = (g: GatewayInfo | undefined): boolean => !!g && g.state === 'running';

export const DEFAULT_LINKS: ServerLinks = {
  website: 'https://github.com/collyn/lunaris',
  issues: 'https://github.com/collyn/lunaris/issues',
  releases: 'https://github.com/collyn/lunaris/releases',
};

const str = (v: unknown): string | undefined => (typeof v === 'string' ? v : undefined);
const num = (v: unknown): number | undefined => (typeof v === 'number' && Number.isFinite(v) ? v : undefined);
/** Only http(s) links are followed. */
const link = (v: unknown, fallback: string): string => {
  const s = str(v);
  return s && /^https?:\/\//i.test(s) ? s : fallback;
};

export function parseServerInfo(body: unknown): ServerInfo | null {
  if (!body || typeof body !== 'object' || Array.isArray(body)) return null;
  const b = body as Record<string, unknown>;
  const l = (b.links && typeof b.links === 'object' ? b.links : {}) as Record<string, unknown>;
  const info: ServerInfo = {
    version: str(b.version) || '',
    links: {
      website: link(l.website, DEFAULT_LINKS.website),
      issues: link(l.issues, DEFAULT_LINKS.issues),
      releases: link(l.releases, DEFAULT_LINKS.releases),
    },
  };
  if (b.update && typeof b.update === 'object') {
    const u = b.update as Record<string, unknown>;
    info.update = {
      checks: u.checks === true,
      checkedAt: num(u.checked_at),
      latest: str(u.latest),
      url: str(u.url) && /^https?:\/\//i.test(str(u.url)!) ? str(u.url) : undefined,
      available: u.available === true,
      error: str(u.error),
    };
  }
  if (b.ports && typeof b.ports === 'object') {
    const p = b.ports as Record<string, unknown>;
    info.ports = { http: num(p.http) ?? null, https: num(p.https) ?? null };
  }
  if (typeof b.data_dir === 'string') info.dataDir = b.data_dir;
  const gateway = parseGateway(b.gateway);
  if (gateway) info.gateway = gateway;
  if (b.install && typeof b.install === 'object') {
    const i = b.install as Record<string, unknown>;
    const kind = INSTALL_KINDS.find(k => k === i.kind);
    if (kind) info.install = { kind, arch: str(i.arch) || 'x86_64' };
  }
  return info;
}

declare const __APP_VERSION__: string | undefined;

/** The version this page was built as (vite.config.ts, from the workspace's Cargo.toml). */
export const webVersion = (): string => (typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : '');

/** One step of "How to update": a sentence, and a command to copy when there is one. */
export type UpdateStep = { text: string; command?: string };

const DOWNLOAD = 'https://github.com/collyn/lunaris/releases/download';

/** What updates this server to `version` (its install kind and machine).
 *  The sentences are English keys for t(); `%1` in them is the version. */
export function updateSteps(kind: InstallKind, arch: string, version: string): UpdateStep[] {
  const v = version.replace(/^v/, '');
  const arm = arch === 'aarch64';
  switch (kind) {
    case 'deb': {
      const file = `lunaris-server_${v}_${arm ? 'arm64' : 'amd64'}.deb`;
      return [
        { text: 'Download the package and install it over this one (the service restarts; accounts, computers and settings stay):', command: `curl -LO ${DOWNLOAD}/v${v}/${file}\nsudo apt install ./${file}` },
      ];
    }
    case 'service': {
      const dir = `lunaris-server-linux-${arm ? 'aarch64' : 'x86_64'}`;
      return [
        { text: 'Download the new server and run its install again (it replaces the program and the service; the data and settings stay):', command: `curl -L ${DOWNLOAD}/v${v}/${dir}.tar.gz | tar xz\nsudo ./${dir}/lunaris-server install` },
      ];
    }
    case 'docker':
      return [
        { text: 'In the folder with the Lunaris source and docker-compose.yml, take the new version and build the image again (the data volume stays):', command: `git fetch --tags && git checkout v${v}\ndocker compose up -d --build` },
      ];
    case 'windows':
      return [
        { text: 'Download the server installer for Windows (%1) and run it on this computer: it updates the LunarisServer service and keeps the data.' },
      ];
    case 'package':
      return [
        { text: 'This server was installed from a package: update it with the package manager that installed it.' },
      ];
    case 'manual':
      return [
        { text: 'This server runs from where it was unpacked: stop it, unpack the new version over it and start it again. Installing it as a service makes later updates one command:', command: `curl -L ${DOWNLOAD}/v${v}/lunaris-server-linux-${arm ? 'aarch64' : 'x86_64'}.tar.gz | tar xz\nsudo ./lunaris-server-linux-${arm ? 'aarch64' : 'x86_64'}/lunaris-server install` },
      ];
  }
}

/** The Windows installer of `version`, for the Windows step's link. */
export const windowsInstallerUrl = (version: string): string =>
  `${DOWNLOAD}/v${version.replace(/^v/, '')}/lunaris-server-installer-windows.exe`;
