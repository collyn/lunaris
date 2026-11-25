import { useEffect, useRef, useState } from 'react';
import { t } from '../i18n';
import { UPDATE_RETURN_MS, agentUpdateHow, agentUpdateState, canUpdateFromWeb, installKindLabel, updateRefusalText, type AgentBuild } from '../agentUpdate';
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
  /** For "Update now": the computer, whether it is connected, the
   *  administrator's token, and how the list of computers is read again. */
  hostId: string;
  online: boolean;
  token: string | null;
  onRefresh: () => void;
  onClose: () => void;
}

/** "Update now": nothing yet; asked (the computer downloads and checks the
 *  release); its installer runs (it comes back as the new version); done; or why not. */
type Phase = 'idle' | 'asking' | 'installing' | 'done' | 'failed';

const ICON_UPDATE = (
  <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M21 12a9 9 0 1 1-3-6.7L21 8" /><path d="M21 3v5h-5" /><path d="M12 8v5l3 2" />
  </svg>
);

const SYSTEMS: Record<string, string> = { linux: 'Linux', windows: 'Windows', macos: 'macOS' };

/** A computer's Lunaris agent: its version, and how to update it to this
 *  server's version (an administrator's view). */
export default function AgentUpdateDialog({ name, build, serverVersion, origin, fetch, hostId, online, token, onRefresh, onClose }: Props) {
  const state = agentUpdateState(build, serverVersion);
  const [phase, setPhase] = useState<Phase>('idle');
  const [failure, setFailure] = useState('');
  const live = useRef(true);
  useEffect(() => () => { live.current = false; }, []);
  const refresh = useRef(onRefresh);
  refresh.current = onRefresh;

  const updateNow = async () => {
    if (!token) return;
    setPhase('asking');
    setFailure('');
    try {
      const r = await window.fetch(`${origin}/api/admin/hosts/${encodeURIComponent(hostId)}/update`, { method: 'POST', headers: { Authorization: `Bearer ${token}` } });
      const body = await r.json().catch(() => null);
      if (!live.current) return;
      if (r.ok) { setPhase('installing'); return; }
      setFailure(r.status === 404 || r.status === 405
        ? t('This server is from before updates from the web: update the server first.')
        : t(updateRefusalText(body?.code, body?.error)));
      setPhase('failed');
    } catch {
      if (!live.current) return;
      setFailure(t('Cannot reach the server'));
      setPhase('failed');
    }
  };

  // While its installer runs the computer goes away and comes back: the
  // list is read again until its agent says the new version.
  useEffect(() => {
    if (phase !== 'installing') return;
    const poll = window.setInterval(() => refresh.current(), 3000);
    const giveUp = window.setTimeout(() => {
      setFailure(t('The installer was started, but the computer has not come back as the new version yet. Look again in a few minutes; if it stays as it is, run the command below there.'));
      setPhase('failed');
    }, UPDATE_RETURN_MS);
    return () => { window.clearInterval(poll); window.clearTimeout(giveUp); };
  }, [phase]);
  useEffect(() => {
    if (phase === 'installing' && state !== 'older' && state !== 'unknown') setPhase('done');
  }, [phase, state]);

  const offered = canUpdateFromWeb(build, serverVersion, online);
  const busy = phase === 'asking' || phase === 'installing';
  const how = build ? agentUpdateHow(build, origin, { fetch }) : null;
  const status = {
    older: t('An update is available: %1 → %2.', build?.version ?? '', serverVersion ?? ''),
    current: t('It has the same version as this server.'),
    newer: t('It is newer than this server (%1): update the server.', serverVersion ?? ''),
    unknown: build ? t('This server\'s version is not known.') : t('Its agent has not said its version: agents from before 0.7.7 do not. Update it with the install command from Add computer (Install the agent there).'),
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
          {(offered || phase !== 'idle') && (
            <div className="agent-update-now" aria-live="polite">
              {phase === 'idle' && (
                <p className="install-hint">{t('The computer downloads release %1 itself and installs it only when the release\'s signature is right. Its sessions end while it restarts (about a minute).', serverVersion ?? '')}</p>
              )}
              {phase === 'asking' && <p className="agent-update-progress"><span className="source-setup-spinner" aria-hidden="true" />{t('The computer is downloading and checking the release…')}</p>}
              {phase === 'installing' && <p className="agent-update-progress"><span className="source-setup-spinner" aria-hidden="true" />{t('Checked. Its installer is running: the computer comes back as the new version in about a minute…')}</p>}
              {phase === 'done' && <p className="agent-update-done">{t('Updated: the computer now runs version %1.', build?.version ?? '')}</p>}
              {phase === 'failed' && <div className="add-host-error" role="alert">{failure}</div>}
              {(phase === 'idle' || phase === 'failed') && offered && (
                <button type="button" className="btn-primary" onClick={updateNow}>{phase === 'failed' ? t('Try again') : t('Update now')}</button>
              )}
            </div>
          )}
          {how && state !== 'newer' && !busy && phase !== 'done' && ('command' in how ? (
            <>
              <p className="install-hint">
                {offered && `${t('Or by hand:')} `}
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
          <button className="btn-secondary" autoFocus={!offered} onClick={onClose}>{t('Close')}</button>
        </div>
      </div>
    </div>
  );
}
