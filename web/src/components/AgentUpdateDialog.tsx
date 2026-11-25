import { t } from '../i18n';
import { agentUpdateHow, agentUpdateState, installKindLabel, type AgentBuild } from '../agentUpdate';
import type { InstallFetch } from '../installCommand';
import { CopyButton } from './AdminInstallAgents';

interface Props {
  name: string;
  build: AgentBuild | undefined;
  /** This server's version: what the update installs. */
  serverVersion: string | null;
  /** The server's origin, and how a command fetches the script from it. */
  origin: string;
  fetch: InstallFetch | null;
  onClose: () => void;
}

const ICON_UPDATE = (
  <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M21 12a9 9 0 1 1-3-6.7L21 8" /><path d="M21 3v5h-5" /><path d="M12 8v5l3 2" />
  </svg>
);

const SYSTEMS: Record<string, string> = { linux: 'Linux', windows: 'Windows', macos: 'macOS' };

/** A computer's Lunaris agent: its version, and how to update it to this
 *  server's version (an administrator's view). */
export default function AgentUpdateDialog({ name, build, serverVersion, origin, fetch, onClose }: Props) {
  const state = agentUpdateState(build, serverVersion);
  const how = build ? agentUpdateHow(build, origin, { fetch }) : null;
  const status = {
    older: t('An update is available: %1 → %2.', build?.version ?? '', serverVersion ?? ''),
    current: t('It has the same version as this server.'),
    newer: t('It is newer than this server (%1): update the server.', serverVersion ?? ''),
    unknown: build ? t('This server\'s version is not known.') : t('Its agent has not said its version: agents from before 0.7.7 do not. Update it with the install command from the Install agents page.'),
  }[state];
  return (
    <div className="host-confirm-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="host-confirm-card agent-update-card" role="dialog" aria-modal="true" aria-labelledby="agentUpdateTitle">
        <div className={`host-confirm-icon${state === 'older' ? ' attention' : ''}`} aria-hidden="true">{ICON_UPDATE}</div>
        <div className="host-confirm-content">
          <h2 id="agentUpdateTitle">{t('Lunaris agent on %1', name)}</h2>
          {build && (
            <dl className="agent-update-info">
              <div><dt>{t('Version')}</dt><dd>{build.version}</dd></div>
              <div><dt>{t('System')}</dt><dd>{SYSTEMS[build.os] ?? build.os}</dd></div>
              <div><dt>{t('Installed as')}</dt><dd>{t(installKindLabel(build.install))}</dd></div>
              {serverVersion && <div><dt>{t('This server')}</dt><dd>{serverVersion}</dd></div>}
            </dl>
          )}
          <p className={state === 'older' ? 'agent-update-status attention' : 'agent-update-status'}>{status}</p>
          {how && state !== 'newer' && ('command' in how ? (
            <>
              <p className="install-hint">
                {how.shell === 'root'
                  ? t('Run on that computer, in a terminal (needs sudo). It keeps the agent\'s settings and its place on this server:')
                  : t('Run on that computer, in PowerShell opened as Administrator. It keeps the agent\'s settings and its place on this server:')}
              </p>
              <div className="install-command small">
                <code>{how.command}</code>
                <CopyButton text={how.command} label={t('Copy command')} />
              </div>
            </>
          ) : (
            <p className="install-hint">{t(how.howTo)}</p>
          ))}
        </div>
        <div className="host-confirm-actions">
          <button className="btn-secondary" autoFocus onClick={onClose}>{t('Close')}</button>
        </div>
      </div>
    </div>
  );
}
