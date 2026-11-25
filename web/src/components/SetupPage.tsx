// First start of a server: the first admin, made with the one-time setup
// code (server/src/setup.rs). Looks like the sign-in page it replaces.
import { useState, type FormEvent } from 'react';
import { t } from '../i18n';
import { parseLoginAnswer, type LoginAnswer } from '../mfa';
import { SETUP_CODE_PLACES, setupFormError } from '../setup';

type Props = {
  /** `http(s)://host:port` of the server. */
  apiBase: string;
  /** The new admin's session (or an error to show). */
  onDone: (answer: LoginAnswer) => void;
  /** Setup was already done (by someone else): show the sign-in page. */
  onClosed: () => void;
};

export default function SetupPage({ apiBase, onDone, onClosed }: Props) {
  const [code, setCode] = useState('');
  const [username, setUsername] = useState('admin');
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const problem = setupFormError(code, username, password, confirm);
    if (problem) {
      setError(t(problem));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const response = await fetch(`${apiBase}/api/setup`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ code, username: username.trim(), password }),
      });
      const data = await response.json().catch(() => ({}));
      if (response.status === 409) {
        onClosed();
        return;
      }
      const answer = parseLoginAnswer(response.ok, data, 'Setup failed');
      if (answer.kind === 'session') {
        onDone(answer);
      } else {
        setError(t(answer.kind === 'error' ? answer.error : 'Setup failed'));
      }
    } catch {
      setError(t('Connection to authentication server failed'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="auth-card-wrapper">
      <div className="auth-card">
        <div className="auth-header">
          <img className="auth-logo" src="/favicon.svg" alt="Lunaris" width={48} height={48} />
          <h2>{t('Set up this server')}</h2>
          <p>{t('Create the first administrator. The setup code proves you installed the server.')}</p>
        </div>

        {error && (
          <div className="auth-error-banner" role="alert">
            <span className="error-icon">⚠️</span>
            <span>{error}</span>
          </div>
        )}

        <form onSubmit={submit} className="auth-form setup-form">
          <div className="form-group">
            <label htmlFor="setupCode">{t('Setup code')}</label>
            <input
              type="text"
              id="setupCode"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              autoFocus
              disabled={busy}
            />
            <div className="setup-code-hint">
              <span>{t('It is in this file on the server (administrators only):')}</span>
              <ul>
                {SETUP_CODE_PLACES.map(place => (
                  <li key={place.label}>
                    {t(place.label)}: <code>{t(place.path)}</code>
                  </li>
                ))}
              </ul>
            </div>
          </div>

          <div className="form-group">
            <label htmlFor="setupUsername">{t('Username')}</label>
            <input
              type="text"
              id="setupUsername"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoComplete="username"
              required
              disabled={busy}
            />
          </div>

          <div className="form-group">
            <label htmlFor="setupPassword">{t('Password')}</label>
            <input
              type="password"
              id="setupPassword"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder={t('At least 10 characters')}
              autoComplete="new-password"
              required
              disabled={busy}
            />
          </div>

          <div className="form-group">
            <label htmlFor="setupConfirm">{t('Confirm password')}</label>
            <input
              type="password"
              id="setupConfirm"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              autoComplete="new-password"
              required
              disabled={busy}
            />
          </div>

          <button type="submit" disabled={busy} className="btn-primary auth-submit-btn">
            {busy ? <div className="inline-loader"></div> : t('Create administrator')}
          </button>
        </form>
      </div>
    </div>
  );
}
