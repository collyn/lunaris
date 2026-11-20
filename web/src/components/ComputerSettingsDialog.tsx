import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { t } from '../i18n';
import { portProblem, settingsChanges, type ComputerSettings } from '../computerSettings';

/**
 * A Lunaris computer's own settings (its agent's: direct access, how
 * connections are accepted, clipboard and files, RustDesk compatibility,
 * Wayland capture), for server administrators. They are asked of the
 * computer's agent each time (server/src/host_settings.rs); nothing is kept
 * here. The person at that computer decides whether they can be changed
 * from here ("Allow the server to change this computer's settings"); when
 * not, they are shown read-only. The access password is shown only when
 * asked for.
 */

const TOO_OLD = "That computer did not answer: its Lunaris agent is probably too old for this. Update it first.";

type Category = 'security' | 'network' | 'capture';

const CATEGORIES: { key: Category; label: string; icon: ReactNode }[] = [
  { key: 'security', label: 'Security', icon: <><rect x="5" y="11" width="14" height="10" rx="2" /><path d="M8 11V7a4 4 0 0 1 8 0v4" /></> },
  { key: 'network', label: 'Network', icon: <><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></> },
  { key: 'capture', label: 'Capture', icon: <><rect x="3" y="4" width="18" height="12" rx="1.5" /><path d="M8 20h8M12 16v4" /></> },
];

const Icon = ({ children }: { children: ReactNode }) => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{children}</svg>
);

const Row = ({ title, hint, children }: { title: string; hint?: string; children: ReactNode }) => (
  <div className="ssd-row">
    <div className="ssd-row-text">
      <div className="ssd-row-title">{title}</div>
      {hint && <div className="ssd-row-hint">{hint}</div>}
    </div>
    <div className="ssd-row-control">{children}</div>
  </div>
);

const Switch = ({ checked, disabled, onChange, label }: { checked: boolean; disabled?: boolean; onChange: (v: boolean) => void; label: string }) => (
  <label className="ssd-switch">
    <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} aria-label={label} />
    <span aria-hidden="true" />
  </label>
);

type Props = {
  hostId: string;
  hostName: string;
  token: string;
  apiBase: () => string;
  onClose: () => void;
};

export default function ComputerSettingsDialog({ hostId, hostName, token, apiBase, onClose }: Props) {
  const [category, setCategory] = useState<Category>('security');
  const [loaded, setLoaded] = useState<ComputerSettings | null>(null);
  const [draft, setDraft] = useState<ComputerSettings | null>(null);
  const [loadError, setLoadError] = useState('');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [password, setPassword] = useState<string | null>(null);
  const [customPassword, setCustomPassword] = useState('');

  const url = `${apiBase()}/api/admin/hosts/${encodeURIComponent(hostId)}/settings`;

  /** The server's words for why not, in this language. */
  const why = async (response: Response, fallback: string) => {
    try {
      const body = await response.json();
      if (body?.code === 'agent_too_old') return t(TOO_OLD);
      if (typeof body?.error === 'string' && body.error) return t(body.error);
    } catch { /* not JSON */ }
    return t(fallback);
  };

  const take = (settings: ComputerSettings) => { setLoaded(settings); setDraft(settings); };

  const load = useCallback(async () => {
    setLoadError('');
    try {
      const response = await fetch(url, { headers: { Authorization: `Bearer ${token}` } });
      if (!response.ok) { setLoadError(await why(response, 'Could not load the settings of this computer')); return; }
      take(await response.json());
    } catch {
      setLoadError(t('Could not reach the server'));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url, token]);

  useEffect(() => { load(); }, [load]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !busy) onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, busy]);

  const editable = !!loaded?.remote_changes && !busy;
  const linked = !!loaded?.linked_by_address;
  const set = <K extends keyof ComputerSettings>(key: K, value: ComputerSettings[K]) => {
    setDraft(d => (d ? { ...d, [key]: value } : d));
    setNotice('');
  };
  const changes = loaded && draft ? settingsChanges(loaded, draft) : {};
  const dirty = Object.keys(changes).length > 0;
  const portError = (port: number) => (portProblem(port) ? t('Use a port from 1024 to 65535') : '');
  const draftError = draft
    ? (portError(draft.direct_port) || portError(draft.rustdesk_port)
      || (draft.rustdesk_enabled && draft.rustdesk_port === draft.direct_port ? t('That port is the one for direct connections') : ''))
    : '';

  const save = async () => {
    if (!dirty || draftError) return;
    setBusy(true); setError(''); setNotice('');
    try {
      const response = await fetch(url, {
        method: 'PUT',
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify(changes),
      });
      if (!response.ok) { setError(await why(response, 'Could not save the settings')); return; }
      take(await response.json());
      setNotice(t('Saved. The computer applied them; sessions on it were ended.'));
    } catch {
      setError(t('Could not reach the server'));
    } finally {
      setBusy(false);
    }
  };

  const passwordAction = async (action: 'show' | 'random' | 'set') => {
    setBusy(true); setError(''); setNotice('');
    try {
      const response = await fetch(`${url}/password`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify(action === 'set' ? { action, password: customPassword } : { action }),
      });
      if (!response.ok) { setError(await why(response, 'Could not change the password')); return; }
      const body = await response.json();
      if (action === 'show') {
        setPassword(typeof body.password === 'string' ? body.password : '');
      } else {
        // The new one is not sent back: it is shown when asked for.
        setPassword(null);
        setCustomPassword('');
        setLoaded(l => (l ? { ...l, has_password: !!body.has_password } : l));
        setNotice(action === 'random' ? t('A new random password is set. Show it to see it.') : t('The new password is set.'));
      }
    } catch {
      setError(t('Could not reach the server'));
    } finally {
      setBusy(false);
    }
  };

  const current = CATEGORIES.find(c => c.key === category)!;
  const customOk = customPassword.length >= 6 && customPassword.length <= 128 && customPassword.trim() === customPassword;

  return (
    <div className="ssd-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !busy) onClose(); }}>
      <div className="ssd csd" role="dialog" aria-modal="true" aria-label={t('Computer settings')}>
        <header className="ssd-header">
          <div className="ssd-breadcrumb">
            <span className="ssd-crumb">{t('Computer settings')}</span>
            <span className="ssd-crumb-sep" aria-hidden="true">›</span>
            <span className="ssd-crumb-current">{t(current.label)}</span>
            <span className="ssd-host"><span className="ssd-host-name" title={hostName}>{hostName}</span></span>
          </div>
          <button type="button" className="ssd-close" onClick={onClose} disabled={busy} aria-label={t('Close')} title={t('Close')}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M6 6l12 12M18 6L6 18" /></svg>
          </button>
        </header>

        <div className="ssd-body">
          <nav className="ssd-nav">
            {CATEGORIES.map(c => (
              <button key={c.key} type="button" className={`ssd-nav-item${c.key === category ? ' active' : ''}`} onClick={() => setCategory(c.key)}>
                <Icon>{c.icon}</Icon>
                <span>{t(c.label)}</span>
              </button>
            ))}
          </nav>

          <div className="ssd-content">
            {!draft && !loadError && <p className="csd-status">{t('Asking the computer…')}</p>}
            {loadError && (
              <div className="csd-status">
                <p className="csd-error" role="alert">{loadError}</p>
                <button type="button" className="btn-secondary" onClick={load}>{t('Retry')}</button>
              </div>
            )}
            {draft && loaded && !loaded.remote_changes && (
              <div className="csd-note" role="note">
                {t("This computer does not let the server change its settings, so they are shown read-only. To allow it, run lunaris-agent set server-settings allow on that computer (add --system for the system service; as root or administrator there) and restart the agent.")}
              </div>
            )}

            {draft && loaded && category === 'security' && (
              <>
                <h3 className="ssd-section">{t('Access password')}</h3>
                <div className="ssd-card">
                  <Row title={t('Password')} hint={linked
                    ? t('This computer was added by its address with this password: it can only be changed on that computer.')
                    : t('Direct and RustDesk clients sign in with it. It is sent here only when you show it.')}>
                    <div className="csd-password">
                      <code className="csd-password-value">{password ?? (loaded.has_password ? '••••••••••••' : t('(none)'))}</code>
                      {password === null
                        ? <button type="button" className="btn-secondary" disabled={!editable || !loaded.has_password} onClick={() => passwordAction('show')}>{t('Show')}</button>
                        : <button type="button" className="btn-secondary" onClick={() => setPassword(null)}>{t('Hide')}</button>}
                    </div>
                  </Row>
                  <Row title={t('New random password')} hint={t('Clients that use the old password have to be given the new one.')}>
                    <button type="button" className="btn-secondary" disabled={!editable || linked} onClick={() => passwordAction('random')}>{t('Generate')}</button>
                  </Row>
                  <Row title={t('Set a password')} hint={t('6 to 128 characters. Sent once to the computer over its secure connection.')}>
                    <div className="csd-password">
                      <input
                        type="password" autoComplete="new-password" className="csd-input"
                        value={customPassword} disabled={!editable || linked}
                        onChange={(e) => setCustomPassword(e.target.value)}
                        aria-label={t('Set a password')}
                      />
                      <button type="button" className="btn-secondary" disabled={!editable || linked || !customOk} onClick={() => passwordAction('set')}>{t('Set')}</button>
                    </div>
                  </Row>
                </div>

                <h3 className="ssd-section">{t('Accepting connections')}</h3>
                <div className="ssd-card">
                  <Row title={t('Direct connections')} hint={t('How a client connecting straight to this computer is let in.')}>
                    <select value={draft.approve_mode} disabled={!editable} onChange={(e) => set('approve_mode', e.target.value)}>
                      <option value="password-click">{t('Password, or accept on the computer')}</option>
                      <option value="password">{t('Password only')}</option>
                      <option value="click">{t('Accept on the computer only')}</option>
                    </select>
                  </Row>
                  <Row title={t('Sessions through this server')} hint={t('Trusted: whoever this server lets through connects at once (unattended access). Ask: the person at the computer accepts each session.')}>
                    <select value={draft.server_sessions} disabled={!editable} onChange={(e) => set('server_sessions', e.target.value)}>
                      <option value="trusted">{t('Trusted')}</option>
                      <option value="ask">{t('Ask on the computer')}</option>
                    </select>
                  </Row>
                </div>

                <h3 className="ssd-section">{t('Clipboard & files')}</h3>
                <div className="ssd-card">
                  <Row title={t('Allow clipboard')} hint={t('Clients may share the clipboard with this computer, both ways.')}>
                    <Switch label={t('Allow clipboard')} checked={draft.allow_clipboard} disabled={!editable} onChange={(v) => set('allow_clipboard', v)} />
                  </Row>
                  <Row title={t('Allow file transfer')} hint={t('Clients may send files to this computer and take files from it.')}>
                    <Switch label={t('Allow file transfer')} checked={draft.allow_file_transfer} disabled={!editable} onChange={(v) => set('allow_file_transfer', v)} />
                  </Row>
                  <Row title={t('File access')} hint={t('What file transfer may reach on this computer.')}>
                    <select value={draft.file_access} disabled={!editable} onChange={(e) => set('file_access', e.target.value)}>
                      <option value="home">{t('Home folder')}</option>
                      <option value="full">{t('Whole disk')}</option>
                      {draft.file_access !== 'home' && draft.file_access !== 'full' && <option value={draft.file_access}>{draft.file_access}</option>}
                    </select>
                  </Row>
                </div>
              </>
            )}

            {draft && loaded && category === 'network' && (
              <>
                <h3 className="ssd-section">{t('Direct access')}</h3>
                <div className="ssd-card">
                  <Row title={t('Direct connections')} hint={linked
                    ? t('This server reaches the computer through it: it can only be turned off or moved on that computer.')
                    : t('Clients on the LAN or a VPN connect straight to this computer with its password. The connection to this server stays either way.')}>
                    <Switch label={t('Direct connections')} checked={draft.direct_enabled} disabled={!editable || linked} onChange={(v) => set('direct_enabled', v)} />
                  </Row>
                  <Row title={t('Port')} hint={t('Port for direct connections (default 8091).')}>
                    <input
                      type="number" min={1024} max={65535} className="csd-input csd-port"
                      value={Number.isNaN(draft.direct_port) ? '' : draft.direct_port} disabled={!editable || linked}
                      onChange={(e) => set('direct_port', parseInt(e.target.value, 10))}
                      aria-label={t('Port')}
                    />
                  </Row>
                </div>

                <h3 className="ssd-section">{t('RustDesk clients')}</h3>
                <div className="ssd-card">
                  <Row title={t('Allow RustDesk clients')} hint={t('RustDesk can connect to this computer by its IP address (unencrypted: for a LAN or a VPN), with the access password.')}>
                    <Switch label={t('Allow RustDesk clients')} checked={draft.rustdesk_enabled} disabled={!editable} onChange={(v) => set('rustdesk_enabled', v)} />
                  </Row>
                  <Row title={t('RustDesk port')} hint={t("RustDesk's own is 21118.")}>
                    <input
                      type="number" min={1024} max={65535} className="csd-input csd-port"
                      value={Number.isNaN(draft.rustdesk_port) ? '' : draft.rustdesk_port} disabled={!editable || !draft.rustdesk_enabled}
                      onChange={(e) => set('rustdesk_port', parseInt(e.target.value, 10))}
                      aria-label={t('RustDesk port')}
                    />
                  </Row>
                  <Row title={t('RustDesk file manager')} hint={t('RustDesk clients may also use their file manager here, when file transfer is allowed.')}>
                    <Switch label={t('RustDesk file manager')} checked={draft.rustdesk_file_transfer} disabled={!editable || !draft.rustdesk_enabled} onChange={(v) => set('rustdesk_file_transfer', v)} />
                  </Row>
                </div>
              </>
            )}

            {draft && loaded && category === 'capture' && (
              <>
                <h3 className="ssd-section">{t('Screen capture')}</h3>
                <div className="ssd-card">
                  <Row title={t('Wayland capture')} hint={t('Linux with Wayland. Auto uses the capture service or KMS without asking; Portal always asks the person at the computer and shows that the screen is shared.')}>
                    <select value={draft.wayland_capture} disabled={!editable} onChange={(e) => set('wayland_capture', e.target.value)}>
                      <option value="auto">{t('Auto')}</option>
                      <option value="portal">{t('Portal')}</option>
                    </select>
                  </Row>
                </div>
                <h3 className="ssd-section">{t('This computer')}</h3>
                <dl className="ssd-card ssd-info">
                  <div className="ssd-info-row"><dt>{t('Remote keyboard and mouse')}</dt><dd>{loaded.input_ready ? t('Ready') : t('Not ready (/dev/uinput is not writable)')}</dd></div>
                  <div className="ssd-info-row"><dt>{t('Agent version')}</dt><dd>{loaded.version || '—'}</dd></div>
                  <div className="ssd-info-row"><dt>{t('System')}</dt><dd>{loaded.os || '—'}</dd></div>
                </dl>
              </>
            )}
          </div>
        </div>

        <footer className="ssd-footer">
          <span className={`ssd-footer-note${error || draftError ? ' csd-error' : ''}`} role={error ? 'alert' : undefined}>
            {error || (editable ? draftError : '') || notice || (loaded?.remote_changes ? t('Saving restarts the sharing on that computer: sessions on it end.') : '')}
          </span>
          <div className="ssd-actions">
            <button type="button" className="btn-secondary" disabled={busy} onClick={onClose}>{t(loaded?.remote_changes ? 'Cancel' : 'Close')}</button>
            {loaded?.remote_changes && (
              <button type="button" className="btn-primary" disabled={!dirty || !!draftError || busy} onClick={save}>{busy ? t('Saving…') : t('Save')}</button>
            )}
          </div>
        </footer>
      </div>
    </div>
  );
}
