// Whether a computer's agent is older than this server, and how it is
// updated (the agent says how it was installed: common::AgentBuild).
// Pure functions, tested with node --test (tests/agentUpdate.test.ts).
import { updateCommand, type InstallFetch, type InstallOs } from './installCommand.ts';

/** What an agent said it is. */
export interface AgentBuild {
  version: string;
  os: string;
  /** "deb", "appimage", "windows", "package", "manual". */
  install: string;
}

/** Compares two versions ("0.7.6", "0.8.0-rc.1"): negative when `a` is
 *  older. A pre-release is older than its release; anything not a version
 *  compares as equal (no update is offered on a guess). */
export const compareVersions = (a: string, b: string): number => {
  const parse = (v: string) => {
    const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$/.exec(v.trim());
    return m ? { nums: [Number(m[1]), Number(m[2]), Number(m[3])], pre: m[4] ?? null } : null;
  };
  const x = parse(a);
  const y = parse(b);
  if (!x || !y) return 0;
  for (let i = 0; i < 3; i++) if (x.nums[i] !== y.nums[i]) return x.nums[i] - y.nums[i];
  if (x.pre === y.pre) return 0;
  if (x.pre === null) return 1;
  if (y.pre === null) return -1;
  return x.pre.localeCompare(y.pre, 'en', { numeric: true });
};

/** "older": an update to the server's version is there; "current"; "newer"
 *  (the agent is ahead of this server); "unknown": the agent did not say. */
export type AgentUpdateState = 'older' | 'current' | 'newer' | 'unknown';

export const agentUpdateState = (build: AgentBuild | null | undefined, serverVersion: string | null | undefined): AgentUpdateState => {
  if (!build?.version || !serverVersion) return 'unknown';
  const c = compareVersions(build.version, serverVersion);
  return c < 0 ? 'older' : c > 0 ? 'newer' : 'current';
};

/** How this agent is updated: a command to run there (the installer's
 *  update mode), or, when it was installed some other way, what to do
 *  (`howTo`, a message to translate). */
export type AgentUpdateHow =
  | { command: string; shell: 'root' | 'admin-powershell' }
  | { howTo: string };

export const agentUpdateHow = (
  build: AgentBuild,
  origin: string,
  opts: { fetch?: InstallFetch | null; insecure?: boolean } = {},
): AgentUpdateHow => {
  const os: InstallOs | null = build.install === 'windows' ? 'windows' : build.install === 'deb' || build.install === 'appimage' ? 'linux' : null;
  if (os) return { command: updateCommand(os, origin, opts), shell: os === 'linux' ? 'root' : 'admin-powershell' };
  if (build.install === 'package') {
    return { howTo: 'This agent was installed with the system\'s package manager: update the lunaris-agent package with it.' };
  }
  return { howTo: 'This agent was not installed with the install command (it runs from where it was put): replace it with the new version, or install it with the command from the Install agents page.' };
};

/** How an install kind is shown. */
export const installKindLabel = (install: string): string => ({
  deb: '.deb package',
  appimage: 'AppImage',
  windows: 'Windows installer',
  package: 'System package',
} as Record<string, string>)[install] ?? 'By hand';
