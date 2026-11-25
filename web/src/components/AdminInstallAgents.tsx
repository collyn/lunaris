import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { getLang, t } from '../i18n';
import {
  VALIDITY_CHOICES,
  enrollCommand,
  installCommand,
  installScriptUrl,
  parseMaxUses,
  parseTimestamp,
  psQuote,
  relativeParts,
  tokenStatus,
  type EnrollKind,
  type InstallOs,
  type InstallToken,
  type TokenStatus,
} from '../installCommand';
import { installTokensApi, type CreatedInstallToken } from '../installTokensApi';

interface AdminInstallAgentsProps {
  token: string;
  getBackendHost: () => string;
  getBackendProtocol: () => { http: string; ws: string };
  /** Shown inside "Add computer" (no page title of its own). */
  embedded?: boolean;
  /** The command joins an agent that is already installed
   *  (`lunaris-agent --enroll`) instead of installing one. */
  enroll?: boolean;
}

/** Copies text; falls back to a hidden textarea where the Clipboard API is
 *  missing (plain http on a LAN address is not a secure context). */
const copyText = async (text: string): Promise<boolean> => {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch { /* fall through */ }
  try {
    const area = document.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(area);
    return ok;
  } catch {
    return false;
  }
};

const locale = () => (getLang() === 'vi' ? 'vi-VN' : undefined);

const absoluteTime = (ms: number | null) => (ms === null ? '' : new Date(ms).toLocaleString(locale()));

const relativeTime = (ms: number, now: number) => {
  const { amount, unit, future } = relativeParts(ms, now);
  if (amount === 0) return t('just now');
  const key = {
    minute: future ? 'in %1 min' : '%1 min ago',
    hour: future ? 'in %1 h' : '%1 h ago',
    day: future ? 'in %1 d' : '%1 d ago',
  }[unit];
  return t(key, amount);
};

const STATUS_LABEL: Record<TokenStatus, string> = {
  active: 'Active',
  expired: 'Expired',
  used_up: 'Used up',
  revoked: 'Revoked',
};

const CopyIcon = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <rect x="9" y="9" width="13" height="13" rx="2" /><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
  </svg>
);
const CheckIcon = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="20 6 9 17 4 12" />
  </svg>
);

/** A copy button that shows "Copied" for a moment. */
export function CopyButton({ text, label, className }: { text: string; label: string; className?: string }) {
  const [state, setState] = useState<'idle' | 'copied' | 'failed'>('idle');
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const onClick = async () => {
    const ok = await copyText(text);
    setState(ok ? 'copied' : 'failed');
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState('idle'), 2000);
  };
  return (
    <button type="button" className={`install-copy-btn ${state} ${className ?? ''}`} onClick={onClick} aria-label={label} title={label}>
      {state === 'copied' ? <CheckIcon /> : <CopyIcon />}
      <span aria-live="polite">{state === 'copied' ? t('Copied') : state === 'failed' ? t('Select and copy by hand') : t('Copy')}</span>
    </button>
  );
}

export function AdminInstallAgents({ token, getBackendHost, getBackendProtocol, embedded, enroll }: AdminInstallAgentsProps) {
  const [enrollKind, setEnrollKind] = useState<EnrollKind>('windows');
  const origin = `${getBackendProtocol().http}//${getBackendHost()}`;
  const api = useMemo(() => installTokensApi(origin, token), [origin, token]);

  const [tokens, setTokens] = useState<InstallToken[]>([]);
  const [loading, setLoading] = useState(true);
  const [listError, setListError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  // Create form
  const [label, setLabel] = useState('');
  const [validHours, setValidHours] = useState(24);
  const [maxUsesText, setMaxUsesText] = useState('');
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // The token just created: shown once, kept only in memory.
  const [created, setCreated] = useState<CreatedInstallToken | null>(null);
  // A new token's command is what comes next: brought into view (in the
  // "Add computer" dialog it is below the fold).
  const revealRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (created) revealRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, [created]);
  const [os, setOs] = useState<InstallOs>(() => (navigator.userAgent.includes('Windows') ? 'windows' : 'linux'));
  const [computerName, setComputerName] = useState('');
  const [insecure, setInsecure] = useState(false);

  // Revoke
  const [pendingRevoke, setPendingRevoke] = useState<InstallToken | null>(null);
  const [revoking, setRevoking] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);

  const load = useCallback(() => api.list().then(
    (list) => { setTokens(list); setListError(null); },
    (err) => { setListError(err instanceof Error ? t(err.message) : t('Connection error')); },
  ).finally(() => { setLoading(false); setNow(Date.now()); }), [api]);

  useEffect(() => {
    load();
  }, [load]);
  // A server on HTTPS with its own certificate says so: then the commands
  // start with the self-signed option on (the person can still turn it off).
  useEffect(() => {
    if (!origin.startsWith('https:')) return;
    let gone = false;
    fetch(`${origin}/api/server/ports`)
      .then((r) => (r.ok ? r.json() : null))
      .then((p) => { if (!gone && p && p.https_self_signed === true) setInsecure(true); })
      .catch(() => {});
    return () => { gone = true; };
  }, [origin]);
  // Keep "expires in ..." and the status fresh.
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);

  const maxUses = parseMaxUses(maxUsesText);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setCreateError(null);
    if (maxUses === undefined) {
      setCreateError(t('Max computers must be a whole number of 1 or more, or empty for no limit'));
      return;
    }
    setCreating(true);
    try {
      const tok = await api.create({ label: label.trim().slice(0, 64), valid_hours: validHours, max_uses: maxUses });
      setCreated(tok);
      setComputerName('');
      setLabel('');
      setMaxUsesText('');
      load();
    } catch (err) {
      setCreateError(err instanceof Error ? t(err.message) : t('Connection error'));
    } finally {
      setCreating(false);
    }
  };

  const handleRevoke = async () => {
    if (!pendingRevoke) return;
    setRevoking(true);
    setRevokeError(null);
    try {
      await api.revoke(pendingRevoke.id);
      if (created && created.id === pendingRevoke.id) setCreated(null);
      setPendingRevoke(null);
      load();
    } catch (err) {
      setRevokeError(err instanceof Error ? t(err.message) : t('Connection error'));
    } finally {
      setRevoking(false);
    }
  };

  // The server's own HTTPS listener, pinned: no certificate option needed.
  const pinned = !!created?.install?.spkiSha256;
  const scriptOrigin = created?.install?.scriptOrigin ?? origin;
  const overHttps = scriptOrigin.startsWith('https:');
  const commandOptions = { name: computerName, insecure: !pinned && overHttps && os === 'linux' && insecure, fetch: created?.install ?? null };
  const command = !created ? ''
    : enroll ? enrollCommand(enrollKind, origin, created.token, { name: computerName, fetch: created.install })
    : installCommand(os, origin, created.token, commandOptions);
  const localPsCommand = created
    ? `$env:LUNARIS_INSTALL_TOKEN=${psQuote(created.token)}; powershell -ExecutionPolicy Bypass -File .\\agent.ps1${computerName.trim() ? ` -Name ${psQuote(computerName.trim())}` : ''}`
    : '';

  return (
    <div className={embedded ? 'install-page install-embedded' : 'admin-page-container install-page'}>
      {!embedded && (
        <div className="section-header">
          <div>
            <h1 className="section-title">{t('Install agents')}</h1>
            <p className="section-subtitle">{t('Create an install token, then paste one command on each computer. It installs the agent and connects it to this server.')}</p>
          </div>
        </div>
      )}

      <form className="install-card install-form" onSubmit={handleCreate}>
        <h2 className="install-card-title">{t('New install token')}</h2>
        <div className="install-form-grid">
          <div className="form-group">
            <label htmlFor="install-label">{t('Label')}</label>
            <input
              id="install-label"
              type="text"
              maxLength={64}
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={t('e.g. Office PCs, 3rd floor')}
            />
          </div>
          <div className="form-group">
            <span className="install-field-label" id="install-valid-label">{t('Valid for')}</span>
            <div className="install-segmented" role="radiogroup" aria-labelledby="install-valid-label">
              {VALIDITY_CHOICES.map((c) => (
                <button
                  key={c.hours}
                  type="button"
                  role="radio"
                  aria-checked={validHours === c.hours}
                  className={validHours === c.hours ? 'active' : ''}
                  onClick={() => setValidHours(c.hours)}
                >
                  {t(c.label)}
                </button>
              ))}
            </div>
          </div>
          <div className="form-group">
            <label htmlFor="install-max">{t('Max computers')}</label>
            <input
              id="install-max"
              type="text"
              inputMode="numeric"
              value={maxUsesText}
              onChange={(e) => setMaxUsesText(e.target.value)}
              placeholder={t('Unlimited')}
              aria-invalid={maxUses === undefined}
            />
          </div>
          <div className="install-form-submit">
            <button type="submit" className="btn-primary" disabled={creating}>
              {creating ? <div className="inline-loader"></div> : t('Create')}
            </button>
          </div>
        </div>
        {createError && <div className="install-error" role="alert">{createError}</div>}
      </form>

      {created && (
        <section ref={revealRef} className="install-card install-reveal" aria-labelledby="install-reveal-title">
          <div className="install-reveal-head">
            <h2 className="install-card-title" id="install-reveal-title">
              {created.label ? t('Install token "%1" created', created.label) : t('Install token created')}
            </h2>
            <button type="button" className="btn-secondary install-done" onClick={() => setCreated(null)}>{t('Done')}</button>
          </div>

          <div className="install-warning" role="note">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
              <line x1="12" y1="9" x2="12" y2="13" /><line x1="12" y1="17" x2="12.01" y2="17" />
            </svg>
            <span>{t('This token is shown only once. Anyone who has it can add computers to this server until it expires or is revoked.')}</span>
          </div>

          {enroll ? (
            <>
              <div className="install-os-tabs" role="tablist" aria-label={t('Operating system')}>
                {([['windows', 'Windows'], ['linux-desktop', 'Linux (desktop)'], ['linux-headless', 'Linux (no desktop)']] as [EnrollKind, string][]).map(([kind, name]) => (
                  <button key={kind} type="button" role="tab" aria-selected={enrollKind === kind} className={enrollKind === kind ? 'active' : ''} onClick={() => setEnrollKind(kind)}>
                    {t(name)}
                  </button>
                ))}
              </div>
              <p className="install-hint">
                {enrollKind === 'windows'
                  ? t('Paste into PowerShell opened as Administrator on that computer:')
                  : enrollKind === 'linux-desktop'
                    ? t('Paste into a terminal on that computer, as the person who uses its desktop (no sudo):')
                    : t('Paste into a terminal on that computer (needs sudo):')}
              </p>
            </>
          ) : (
            <>
              <div className="install-os-tabs" role="tablist" aria-label={t('Operating system')}>
                {(['linux', 'windows'] as InstallOs[]).map((o) => (
                  <button
                    key={o}
                    type="button"
                    role="tab"
                    aria-selected={os === o}
                    className={os === o ? 'active' : ''}
                    onClick={() => setOs(o)}
                  >
                    {o === 'linux' ? 'Linux' : 'Windows'}
                  </button>
                ))}
              </div>

              <p className="install-hint">
                {os === 'linux'
                  ? t('Paste into a terminal on each computer (needs sudo):')
                  : t('Paste into PowerShell opened as Administrator on each computer:')}
              </p>
            </>
          )}
          <div className="install-command">
            <code>{command}</code>
            <CopyButton text={command} label={t('Copy command')} />
          </div>

          <div className="install-options">
            <div className="form-group">
              <label htmlFor="install-name">{t('Computer name (optional)')}</label>
              <input
                id="install-name"
                type="text"
                maxLength={64}
                value={computerName}
                onChange={(e) => setComputerName(e.target.value)}
                placeholder={t('Leave empty to use the hostname')}
              />
              <small className="install-small">{t('For installing many computers, leave this empty: each agent uses its own hostname.')}</small>
            </div>
            {/* Only HTTPS has a certificate to skip; a pinned one needs no option. */}
            {!enroll && overHttps && !pinned && (
              <label className="admin-toggle-label install-toggle">
                <input type="checkbox" checked={insecure} onChange={(e) => setInsecure(e.target.checked)} />
                <span>{t('Self-signed certificate')}</span>
              </label>
            )}
          </div>

          {enroll && (
            <p className="install-note">
              {t('Nothing is downloaded or installed: the agent there is told this server\'s address and joins it with the token. An agent that was on another server leaves it.')}
              {created.install?.certSha256 ? ` ${t('It accepts only this server\'s certificate.')}` : ''}
            </p>
          )}
          {enroll && !overHttps && (
            <p className="install-note">{t('This server has no HTTPS: the agent connects unprotected. Turn HTTPS on (LUNARIS_HTTPS_PORT) for anything but a network you trust.')}</p>
          )}
          {!enroll && pinned && (
            <p className="install-note">{t('The command downloads the script over HTTPS from %1 and accepts only this server\'s certificate, even though it is self-signed.', scriptOrigin)}</p>
          )}
          {!enroll && !overHttps && (
            <div className="install-warning" role="note">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                <line x1="12" y1="9" x2="12" y2="13" /><line x1="12" y1="17" x2="12.01" y2="17" />
              </svg>
              <span>{t('This server has no HTTPS: the script is downloaded unprotected and runs as administrator, so anyone on the network could change it. Turn HTTPS on (LUNARIS_HTTPS_PORT) or use this only on a network you trust.')}</span>
            </div>
          )}
          {!enroll && !pinned && overHttps && insecure && os === 'linux' && (
            <p className="install-note">{t('Adds -k to curl so the script can be downloaded from a server whose certificate is not trusted. Use it only on a network you trust.')}</p>
          )}
          {!enroll && !pinned && overHttps && insecure && os === 'windows' && (
            <div className="install-note">
              <p>{t('PowerShell on Windows cannot skip the certificate check for this download. Give the server a trusted certificate (for example from Let\'s Encrypt), or add its certificate to "Trusted Root Certification Authorities" on each computer.')}</p>
              <p>{t('Or download %1 in a browser, then run this as Administrator in the folder where it was saved:', installScriptUrl('windows', scriptOrigin))}</p>
              <div className="install-command small">
                <code>{localPsCommand}</code>
                <CopyButton text={localPsCommand} label={t('Copy command')} />
              </div>
            </div>
          )}
        </section>
      )}

      <div className="install-list-head">
        <h2 className="install-card-title">{t('Install tokens')}</h2>
      </div>

      {loading ? (
        <div className="loading-card">
          <div className="tech-loader"></div>
          <div>{t('Loading...')}</div>
        </div>
      ) : listError ? (
        <div className="error-card">
          <div className="error-desc">{listError}</div>
          <button onClick={() => { setLoading(true); load(); }} className="btn-secondary">{t('Retry')}</button>
        </div>
      ) : (
        <div className="admin-table-container install-table-container">
          <table className="admin-table install-table">
            <thead>
              <tr>
                <th>{t('Label')}</th>
                <th>{t('Created')}</th>
                <th>{t('Expires')}</th>
                <th>{t('Computers')}</th>
                <th>{t('Status')}</th>
                <th><span className="sr-only">{t('Actions')}</span></th>
              </tr>
            </thead>
            <tbody>
              {tokens.map((tok) => {
                const status = tokenStatus(tok, now);
                const createdMs = parseTimestamp(tok.created_at);
                const expiresMs = parseTimestamp(tok.expires_at);
                return (
                  <tr key={tok.id} className={status === 'active' ? '' : 'inactive'}>
                    <td data-label={t('Label')}>
                      <span className="install-token-label">{tok.label || <span className="install-muted">{t('(no label)')}</span>}</span>
                    </td>
                    <td data-label={t('Created')}>
                      <span className="install-time" title={absoluteTime(createdMs)}>
                        {createdMs === null ? '—' : relativeTime(createdMs, now)}
                      </span>
                    </td>
                    <td data-label={t('Expires')}>
                      <span className="install-time" title={absoluteTime(expiresMs)}>
                        {expiresMs === null ? t('Never') : relativeTime(expiresMs, now)}
                      </span>
                    </td>
                    <td data-label={t('Computers')}>
                      <span className="install-uses">{tok.uses} / {tok.max_uses ?? '∞'}</span>
                    </td>
                    <td data-label={t('Status')}>
                      <span className={`install-status ${status}`}>{t(STATUS_LABEL[status])}</span>
                    </td>
                    <td className="install-actions">
                      {(status === 'active' || status === 'used_up') && (
                        <button type="button" className="install-revoke-btn" onClick={() => { setRevokeError(null); setPendingRevoke(tok); }}>
                          {t('Revoke')}
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
              {tokens.length === 0 && (
                <tr className="install-empty">
                  <td colSpan={6}>{t('No install tokens yet. Create one above to install agents with a single command.')}</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}

      {pendingRevoke && (
        <div className="host-confirm-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !revoking) setPendingRevoke(null); }}>
          <div className="host-confirm-card" role="alertdialog" aria-modal="true" aria-labelledby="revokeInstallTitle">
            <div className="host-confirm-icon danger">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" />
              </svg>
            </div>
            <div className="host-confirm-content">
              <h2 id="revokeInstallTitle">{t('Revoke install token')}</h2>
              <p>
                {t('No more computers can be installed with %1.', pendingRevoke.label ? `"${pendingRevoke.label}"` : t('this token'))}
              </p>
              {revokeError && <p className="install-error">{revokeError}</p>}
            </div>
            <div className="host-confirm-actions">
              <button className="btn-secondary" disabled={revoking} onClick={() => setPendingRevoke(null)}>{t('Cancel')}</button>
              <button className="btn-danger" disabled={revoking} onClick={handleRevoke}>
                {revoking ? t('Revoking...') : t('Revoke')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
