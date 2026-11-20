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
};

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
  return info;
}

declare const __APP_VERSION__: string | undefined;

/** The version this page was built as (vite.config.ts, from the workspace's Cargo.toml). */
export const webVersion = (): string => (typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : '');
