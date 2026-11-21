import { useEffect, useRef, useState } from 'react';
import { t } from '../i18n';
import {
  accountErrorMessage,
  groupSecret,
  isCompleteTotp,
  isPlausibleRecoveryCode,
  normalizeRecoveryInput,
  normalizeTotpInput,
  recoveryCodesFileName,
  recoveryCodesText,
} from '../mfa';

// Two-factor sign-in for the signed-in user (server/src/mfa.rs): turn it on
// with an authenticator app (QR code or the key typed in), see the recovery
// codes once, make new ones, turn it off.

interface Props {
  token: string;
  username: string;
  server: string;
  apiBase: () => string;
  onClose: () => void;
  /** The server ended the other sessions and gave this one a new token. */
  onToken: (token: string) => void;
  /** The token is no longer accepted (401). */
  onSessionEnded: () => void;
}

interface Setup {
  secret: string;
  otpauth_url: string;
  qr: string | null;
}

type View =
  | { kind: 'loading' }
  | { kind: 'off' }
  | { kind: 'setup'; setup: Setup }
  | { kind: 'codes'; codes: string[]; fresh: 'enabled' | 'regenerated' }
  | { kind: 'on'; left: number }
  | { kind: 'disable'; left: number }
  | { kind: 'regenerate'; left: number }
  /** Asking before every session of this account is signed out. */
  | { kind: 'logoutAll'; back: View };

const ShieldIcon = () => (
  <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10Z" />
    <path d="m9 12 2 2 4-4" />
  </svg>
);

export function AccountSecurity({ token, username, server, apiBase, onClose, onToken, onSessionEnded }: Props) {
  const [view, setView] = useState<View>({ kind: 'loading' });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [code, setCode] = useState('');
  const [password, setPassword] = useState('');
  const [useRecovery, setUseRecovery] = useState(false);
  const [recovery, setRecovery] = useState('');
  const [copied, setCopied] = useState<string | null>(null);
  const [showUrl, setShowUrl] = useState(false);
  const codeRef = useRef<HTMLInputElement>(null);
  const passwordRef = useRef<HTMLInputElement>(null);

  const call = async (method: string, path: string, body?: unknown): Promise<{ ok: boolean; data: any }> => {
    const response = await fetch(`${apiBase()}${path}`, {
      method,
      headers: {
        'Authorization': `Bearer ${token}`,
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (response.status === 401) {
      onSessionEnded();
      return { ok: false, data: { error: 'Session ended. Sign in again.' } };
    }
    const data = await response.json().catch(() => ({}));
    if (response.status === 404 && path === '/api/auth/mfa') {
      return { ok: false, data: { error: 'This server does not offer two-factor sign-in yet' } };
    }
    return { ok: response.ok, data };
  };

  const resetFields = () => {
    setCode(''); setPassword(''); setRecovery(''); setUseRecovery(false); setError(null); setCopied(null);
  };

  const loadStatus = async () => {
    try {
      const { ok, data } = await call('GET', '/api/auth/mfa');
      if (!ok) { setError(t(data.error || 'Could not load the security settings')); setView({ kind: 'off' }); return; }
      setView(data.enabled ? { kind: 'on', left: Number(data.recovery_codes_left) || 0 } : { kind: 'off' });
    } catch {
      setError(t('Connection error'));
      setView({ kind: 'off' });
    }
  };

  useEffect(() => { loadStatus(); }, []);

  useEffect(() => {
    if (view.kind === 'setup' || view.kind === 'disable' || view.kind === 'regenerate') codeRef.current?.focus();
    if (view.kind === 'off') passwordRef.current?.focus();
  }, [view.kind, useRecovery]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !busy && view.kind !== 'codes') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, view.kind, onClose]);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try { await fn(); } catch { setError(t('Connection error')); } finally { setBusy(false); }
  };

  // The account password proves it is the account's owner, not someone at
  // an unlocked browser. A server that does not ask for it ignores it.
  const startSetup = (e: React.FormEvent) => {
    e.preventDefault();
    if (!password) { setError(t('Enter your password')); return; }
    run(async () => {
      const { ok, data } = await call('POST', '/api/auth/mfa/setup', { password });
      if (!ok) { setError(t(accountErrorMessage(data, 'Could not start'))); setPassword(''); return; }
      resetFields();
      setShowUrl(false);
      setView({ kind: 'setup', setup: { secret: data.secret, otpauth_url: data.otpauth_url, qr: data.qr || null } });
    });
  };

  // Ends every session of this account (other browsers, the desktop app),
  // then this one.
  const logoutAll = () => run(async () => {
    const response = await fetch(`${apiBase()}/api/auth/logout-all`, {
      method: 'POST',
      headers: { 'Authorization': `Bearer ${token}` },
    });
    if (response.ok || response.status === 401) { onSessionEnded(); return; }
    if (response.status === 404) { setError(t('This server cannot sign out every session yet')); return; }
    const data = await response.json().catch(() => ({}));
    setError(t(accountErrorMessage(data, 'Could not sign out everywhere')));
  });

  const confirmSetup = (e: React.FormEvent) => {
    e.preventDefault();
    if (!isCompleteTotp(code)) { setError(t('Enter the 6-digit code from your app')); return; }
    run(async () => {
      const { ok, data } = await call('POST', '/api/auth/mfa/enable', { code });
      if (!ok) { setError(t(data.error || 'Wrong code')); setCode(''); return; }
      resetFields();
      setView({ kind: 'codes', codes: data.recovery_codes || [], fresh: 'enabled' });
    });
  };

  const regenerate = (e: React.FormEvent) => {
    e.preventDefault();
    if (!isCompleteTotp(code)) { setError(t('Enter the 6-digit code from your app')); return; }
    run(async () => {
      const { ok, data } = await call('POST', '/api/auth/mfa/recovery-codes', { code });
      if (!ok) { setError(t(data.error || 'Wrong code')); setCode(''); return; }
      resetFields();
      setView({ kind: 'codes', codes: data.recovery_codes || [], fresh: 'regenerated' });
    });
  };

  const disable = (e: React.FormEvent) => {
    e.preventDefault();
    if (!password) { setError(t('Enter your password')); return; }
    if (useRecovery ? !isPlausibleRecoveryCode(recovery) : !isCompleteTotp(code)) {
      setError(useRecovery ? t('Enter one of your recovery codes') : t('Enter the 6-digit code from your app'));
      return;
    }
    run(async () => {
      const body = useRecovery
        ? { password, recovery_code: normalizeRecoveryInput(recovery) }
        : { password, code };
      const { ok, data } = await call('POST', '/api/auth/mfa/disable', body);
      if (!ok) { setError(t(accountErrorMessage(data, 'Wrong password or code'))); setCode(''); return; }
      if (typeof data.token === 'string' && data.token) onToken(data.token);
      resetFields();
      setView({ kind: 'off' });
    });
  };

  const copy = async (text: string, what: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(what);
      window.setTimeout(() => setCopied(c => (c === what ? null : c)), 2000);
    } catch {
      setError(t('Could not copy. Select the text and copy it yourself.'));
    }
  };

  const download = (codes: string[]) => {
    const text = recoveryCodesText(
      codes, username, server, new Date(), t('Lunaris recovery codes'),
      t('Each code signs you in once if you cannot use your authenticator app. Keep them somewhere safe.'),
    );
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = recoveryCodesFileName(username);
    document.body.appendChild(a);
    a.click();
    a.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  };

  const codeInput = (id: string, label: string) => (
    <div className="form-group">
      <label htmlFor={id}>{label}</label>
      <input
        ref={codeRef}
        id={id}
        className="mfa-code-input"
        type="text"
        inputMode="numeric"
        autoComplete="one-time-code"
        pattern="[0-9]*"
        maxLength={7}
        placeholder="123456"
        value={code}
        onChange={(e) => setCode(normalizeTotpInput(e.target.value))}
        disabled={busy}
      />
    </div>
  );

  const errorBanner = error && (
    <div className="auth-error-banner mfa-error" role="alert">
      <span className="error-icon">⚠️</span>
      <span>{error}</span>
    </div>
  );

  const closeButton = (
    <button type="button" className="btn-close-modal mfa-close" onClick={onClose} disabled={busy} aria-label={t('Close')} title={t('Close')}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" aria-hidden="true">
        <line x1="18" y1="6" x2="6" y2="18" />
        <line x1="6" y1="6" x2="18" y2="18" />
      </svg>
    </button>
  );

  const header = (title: string, subtitle?: string) => (
    <div className="mfa-header">
      <div className={`host-confirm-icon mfa-icon${view.kind === 'on' || view.kind === 'codes' ? ' on' : ''}`}><ShieldIcon /></div>
      <div className="mfa-header-text">
        <h2 id="mfaTitle">{title}</h2>
        {subtitle && <p>{subtitle}</p>}
      </div>
      {view.kind !== 'codes' && closeButton}
    </div>
  );

  const logoutAllButton = (from: View) => (
    <button type="button" className="btn-secondary" onClick={() => { resetFields(); setView({ kind: 'logoutAll', back: from }); }} disabled={busy}>
      {t('Sign out everywhere')}
    </button>
  );

  let body: React.ReactNode = null;
  switch (view.kind) {
    case 'loading':
      body = (<>{header(t('Two-factor sign-in'))}<div className="mfa-loading"><div className="inline-loader" /></div></>);
      break;
    case 'off':
      body = (
        <form onSubmit={startSetup} className="mfa-form">
          {header(t('Two-factor sign-in'), t('Off'))}
          <div className="mfa-body">
            <p>{t('Besides your password, signing in will ask for a 6-digit code from an authenticator app on your phone (Google Authenticator, Microsoft Authenticator, Aegis, 1Password…). Someone who learns your password still cannot sign in.')}</p>
            <input type="text" name="username" autoComplete="username" value={username} readOnly hidden />
            <div className="form-group">
              <label htmlFor="mfaSetupPassword">{t('Your password, to turn it on')}</label>
              <input ref={passwordRef} id="mfaSetupPassword" type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} disabled={busy} />
            </div>
            {errorBanner}
          </div>
          <div className="host-confirm-actions mfa-actions-wrap">
            {logoutAllButton(view)}
            <button type="button" className="btn-secondary" onClick={onClose} disabled={busy}>{t('Close')}</button>
            <button type="submit" className="btn-primary" disabled={busy || !password}>
              {busy ? <div className="inline-loader" /> : t('Turn on')}
            </button>
          </div>
        </form>
      );
      break;
    case 'setup': {
      const { setup } = view;
      body = (
        <form onSubmit={confirmSetup} className="mfa-form">
          {header(t('Set up your authenticator app'))}
          <div className="mfa-body">
            <ol className="mfa-steps">
              <li>{t('In the app, add an account and scan this QR code.')}</li>
            </ol>
            {setup.qr && (
              <div className="mfa-qr">
                <img src={setup.qr} alt={t('QR code to add Lunaris to your authenticator app')} width={184} height={184} />
              </div>
            )}
            <div className="mfa-secret-block">
              <span className="mfa-secret-label">{t("Can't scan it? Enter this key in the app (time-based):")}</span>
              <div className="mfa-secret-row">
                <code className="mfa-secret" aria-label={t('Setup key')}>{groupSecret(setup.secret)}</code>
                <button type="button" className="btn-secondary mfa-copy" onClick={() => copy(setup.secret, 'secret')}>
                  {copied === 'secret' ? t('Copied') : t('Copy')}
                </button>
              </div>
              <button type="button" className="btn-link mfa-small-link" onClick={() => setShowUrl(s => !s)}>
                {showUrl ? t('Hide the setup link') : t('Show the setup link')}
              </button>
              {showUrl && <code className="mfa-url">{setup.otpauth_url}</code>}
            </div>
            <ol className="mfa-steps" start={2}>
              <li>{t('Enter the 6-digit code the app shows to finish.')}</li>
            </ol>
            {codeInput('mfaSetupCode', t('Code from the app'))}
            {errorBanner}
          </div>
          <div className="host-confirm-actions">
            <button type="button" className="btn-secondary" onClick={() => { resetFields(); loadStatus(); }} disabled={busy}>{t('Cancel')}</button>
            <button type="submit" className="btn-primary" disabled={busy || !isCompleteTotp(code)}>
              {busy ? <div className="inline-loader" /> : t('Turn on')}
            </button>
          </div>
        </form>
      );
      break;
    }
    case 'codes': {
      const all = view.codes.join('\n');
      body = (
        <>
          {header(
            view.fresh === 'enabled' ? t('Two-factor sign-in is on') : t('New recovery codes'),
            t('Save these recovery codes now. They are shown only once.'),
          )}
          <div className="mfa-body">
            <p>{t('If you lose your phone, each code signs you in once instead of a code from the app.')}{view.fresh === 'regenerated' ? ` ${t('Your old recovery codes no longer work.')}` : ''}</p>
            <ul className="mfa-recovery-list" aria-label={t('Recovery codes')}>
              {view.codes.map(c => <li key={c}><code>{c}</code></li>)}
            </ul>
            <div className="mfa-recovery-actions">
              <button type="button" className="btn-secondary" onClick={() => copy(all, 'codes')}>
                {copied === 'codes' ? t('Copied') : t('Copy')}
              </button>
              <button type="button" className="btn-secondary" onClick={() => download(view.codes)}>{t('Download .txt')}</button>
            </div>
            {errorBanner}
          </div>
          <div className="host-confirm-actions">
            <button type="button" className="btn-primary" onClick={() => { resetFields(); loadStatus(); }}>{t("I've saved them")}</button>
          </div>
        </>
      );
      break;
    }
    case 'on':
      body = (
        <>
          {header(t('Two-factor sign-in'), t('On'))}
          <div className="mfa-body">
            <p>{t('Signing in asks for a code from your authenticator app.')}</p>
            <p className={view.left <= 2 ? 'mfa-warn' : 'mfa-muted'}>
              {view.left === 1 ? t('1 recovery code left') : t('%1 recovery codes left', view.left)}
            </p>
            {errorBanner}
          </div>
          <div className="host-confirm-actions mfa-actions-wrap">
            {logoutAllButton(view)}
            <button type="button" className="btn-secondary" onClick={() => { resetFields(); setView({ kind: 'regenerate', left: view.left }); }}>{t('New recovery codes')}</button>
            <button type="button" className="btn-danger" onClick={() => { resetFields(); setView({ kind: 'disable', left: view.left }); }}>{t('Turn off')}</button>
          </div>
        </>
      );
      break;
    case 'regenerate':
      body = (
        <form onSubmit={regenerate} className="mfa-form">
          {header(t('New recovery codes'), t('Your old recovery codes stop working.'))}
          <div className="mfa-body">
            {codeInput('mfaRegenCode', t('Code from the app'))}
            {errorBanner}
          </div>
          <div className="host-confirm-actions">
            <button type="button" className="btn-secondary" onClick={() => { resetFields(); setView({ kind: 'on', left: view.left }); }} disabled={busy}>{t('Back')}</button>
            <button type="submit" className="btn-primary" disabled={busy || !isCompleteTotp(code)}>
              {busy ? <div className="inline-loader" /> : t('Make new codes')}
            </button>
          </div>
        </form>
      );
      break;
    case 'disable':
      body = (
        <form onSubmit={disable} className="mfa-form">
          {header(t('Turn off two-factor sign-in?'), t('Signing in will ask for your password only. Your other sessions will be signed out.'))}
          <div className="mfa-body">
            <input type="text" name="username" autoComplete="username" value={username} readOnly hidden />
            <div className="form-group">
              <label htmlFor="mfaDisablePassword">{t('Password')}</label>
              <input id="mfaDisablePassword" type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} disabled={busy} />
            </div>
            {useRecovery ? (
              <div className="form-group">
                <label htmlFor="mfaDisableRecovery">{t('Recovery code')}</label>
                <input
                  ref={codeRef}
                  id="mfaDisableRecovery"
                  className="mfa-recovery-input"
                  type="text"
                  autoComplete="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  placeholder="xxxx-xxxx-xxxx"
                  value={recovery}
                  onChange={(e) => setRecovery(e.target.value)}
                  disabled={busy}
                />
              </div>
            ) : codeInput('mfaDisableCode', t('Code from the app'))}
            <button type="button" className="btn-link mfa-small-link" onClick={() => { setUseRecovery(u => !u); setError(null); }}>
              {useRecovery ? t('Use a code from the app') : t('Use a recovery code')}
            </button>
            {errorBanner}
          </div>
          <div className="host-confirm-actions">
            <button type="button" className="btn-secondary" onClick={() => { resetFields(); setView({ kind: 'on', left: view.left }); }} disabled={busy}>{t('Back')}</button>
            <button type="submit" className="btn-danger" disabled={busy}>
              {busy ? <div className="inline-loader" /> : t('Turn off')}
            </button>
          </div>
        </form>
      );
      break;
    case 'logoutAll': {
      const back = view.back;
      body = (
        <>
          {header(t('Sign out everywhere?'), t('Every session of this account ends: other browsers, the desktop app and this browser. Sign in again afterwards.'))}
          <div className="mfa-body">{errorBanner}</div>
          <div className="host-confirm-actions">
            <button type="button" className="btn-secondary" onClick={() => { resetFields(); setView(back); }} disabled={busy}>{t('Back')}</button>
            <button type="button" className="btn-danger" onClick={logoutAll} disabled={busy}>
              {busy ? <div className="inline-loader" /> : t('Sign out everywhere')}
            </button>
          </div>
        </>
      );
      break;
    }
  }

  return (
    <div
      className="host-confirm-overlay"
      onMouseDown={(e) => { if (e.target === e.currentTarget && !busy && view.kind !== 'codes') onClose(); }}
    >
      <div className="host-confirm-card mfa-card" role="dialog" aria-modal="true" aria-labelledby="mfaTitle">
        {body}
      </div>
    </div>
  );
}
