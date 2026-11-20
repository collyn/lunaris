import { useEffect, useState, type ReactNode } from 'react';
import { t } from '../i18n';
import { DEFAULT_LINKS, parseGateway, parseServerInfo, webVersion, type GatewayInfo, type ServerInfo } from '../serverInfo';

/** GET /api/server/info (server/src/server_info.rs). */
const useServerInfo = (token: string | null, apiBase: () => string) => {
  const [info, setInfo] = useState<ServerInfo | null>(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(!!token);
  const [asked, setAsked] = useState(0);
  useEffect(() => {
    if (!token) return;
    let live = true;
    fetch(`${apiBase()}/api/server/info`, { headers: { Authorization: `Bearer ${token}` } })
      .then(async (r) => {
        if (!live) return;
        if (r.status === 404) { setError(t('This server is from before this page: update it to see this.')); return; }
        if (!r.ok) { setError(t('Could not read the server information')); return; }
        setError('');
        setInfo(parseServerInfo(await r.json().catch(() => null)));
      })
      .catch(() => { if (live) setError(t('Cannot reach the server')); })
      .finally(() => { if (live) setLoading(false); });
    return () => { live = false; };
    // (apiBase is the same server for as long as the page is open.)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token, asked]);
  const reload = () => { setLoading(true); setAsked(n => n + 1); };
  return { info, error, loading, reload };
};

const Row = ({ label, children }: { label: string; children: ReactNode }) => (
  <div className="ssd-info-row">
    <dt>{label}</dt>
    <dd>{children}</dd>
  </div>
);

const Links = ({ info }: { info: ServerInfo | null }) => {
  const links = info?.links;
  if (!links) return null;
  return (
    <div className="server-info-links">
      <a href={links.website} target="_blank" rel="noreferrer noopener">{t('Website')}</a>
      <a href={links.releases} target="_blank" rel="noreferrer noopener">{t('Releases')}</a>
      <a href={links.issues} target="_blank" rel="noreferrer noopener">{t('Report a problem')}</a>
    </div>
  );
};

const checkedText = (seconds: number) => new Date(seconds * 1000).toLocaleString();

const gatewayStateText = (g: GatewayInfo): string => {
  switch (g.state) {
    case 'running': return t('Running');
    case 'starting': return t('Starting…');
    case 'disabled': return t('Off');
    case 'not_found': return t('lunaris-agent is not installed on this server');
    case 'unsupported': return t('The lunaris-agent on this server is too old: update it');
    case 'failing': return t('Not running (started again after a while)');
  }
};

/** The server gateway (server/src/gateway.rs): where it is, and its switch. */
function GatewaySection({ token, apiBase, initial }: { token: string; apiBase: () => string; initial: GatewayInfo }) {
  const [gateway, setGateway] = useState<GatewayInfo>(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const toggle = async () => {
    setBusy(true);
    setError('');
    try {
      const r = await fetch(`${apiBase()}/api/admin/server/gateway`, {
        method: 'PUT',
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled: !gateway.enabled }),
      });
      const body = await r.json().catch(() => null);
      if (!r.ok) { setError((body && typeof body.error === 'string') ? t(body.error) : t('Could not change it')); return; }
      const next = parseGateway(body);
      if (next) setGateway(next);
    } catch {
      setError(t('Cannot reach the server'));
    } finally {
      setBusy(false);
    }
  };
  const detail = gateway.state === 'disabled' || gateway.state === 'running' ? '' : gateway.error || '';
  return (
    <>
      <h3 className="ssd-section">{t('Server gateway')}</h3>
      <p className="form-hint server-info-hint">{t('A Lunaris agent this server runs itself, so RustDesk, Sunshine, RDP and VNC computers can be added and reached through the server without installing an agent anywhere else. It never shows this server\'s own screen: it only connects to the computers added through it.')}</p>
      <dl className="ssd-card ssd-info">
        <Row label={t('State')}>
          <span className={`server-gateway-state server-gateway-${gateway.state}`}>{gatewayStateText(gateway)}</span>
          {detail && <div className="server-gateway-error">{detail}</div>}
        </Row>
        {gateway.program && <Row label={t('Program')}><code className="server-info-path">{gateway.program}</code></Row>}
        <Row label={t('Switch')}>
          <button type="button" className="btn-secondary" onClick={toggle} disabled={busy || !!gateway.forced}>
            {gateway.enabled ? t('Turn off') : t('Turn on')}
          </button>
          {gateway.forced && <div className="form-hint">{t('Set by %1 in the server\'s settings.', `LUNARIS_GATEWAY=${gateway.forced}`)}</div>}
          {error && <div className="server-gateway-error">{error}</div>}
        </Row>
      </dl>
      {gateway.state === 'not_found' && (
        <p className="form-hint server-info-hint">{t('Install lunaris-agent on this machine (the lunaris-agent package, or Lunaris Agent on Windows): the server finds it beside its own program, in PATH or where the agent installs itself, and starts it. Docker: build the image with AGENT_APPIMAGE_URL (see the Dockerfile).')}</p>
      )}
      {gateway.state === 'running' && (
        <p className="form-hint server-info-hint">{t('Add a computer and choose the gateway to reach it through. Administrators see it; put it in a group to let that group use the computers added through it.')}</p>
      )}
    </>
  );
}

/** The administrators' "Server" page: version, update, ports, data folder, links. */
export function AdminServerInfo({ token, apiBase }: { token: string; apiBase: () => string }) {
  const { info, error, loading, reload } = useServerInfo(token, apiBase);
  const update = info?.update;
  return (
    <div className="admin-page-container">
      <div className="section-header">
        <div>
          <h1 className="section-title">{t('Server')}</h1>
          <p className="section-subtitle">{t('This Lunaris server: its version, whether a newer one exists, and where it keeps its data.')}</p>
        </div>
        <button type="button" className="btn-secondary refresh-btn" onClick={() => reload()} disabled={loading}>{t('Refresh')}</button>
      </div>
      {error && <div className="error-card"><div className="error-desc">{error}</div></div>}
      {info && (
        <div className="server-info">
          <h3 className="ssd-section">{t('Version')}</h3>
          <dl className="ssd-card ssd-info">
            <Row label={t('Server version')}>{info.version || '?'}</Row>
            <Row label={t('Web page version')}>{webVersion() || '?'}</Row>
            <Row label={t('Updates')}>
              {!update || !update.checks ? t('Not checked by this server (LUNARIS_NO_UPDATE_CHECK is set)')
                : update.available && update.latest ? (
                  <span className="server-update-available">
                    {t('Lunaris %1 is available', update.latest)}{' '}
                    {update.url && <a href={update.url} target="_blank" rel="noreferrer noopener">{t('What is new')}</a>}
                  </span>
                )
                : update.latest ? t('Up to date (latest: %1)', update.latest)
                : update.error ? t('The last check failed: %1', update.error)
                : t('Not checked yet (the first check is shortly after the server starts)')}
            </Row>
            {update?.checkedAt ? <Row label={t('Last checked')}>{checkedText(update.checkedAt)}</Row> : null}
          </dl>
          <p className="form-hint server-info-hint">{t('The server never updates itself: update the package, the Docker image or the service the way it was installed.')}</p>

          <h3 className="ssd-section">{t('Network')}</h3>
          <dl className="ssd-card ssd-info">
            <Row label={t('HTTP port')}>{info.ports?.http ?? '?'}</Row>
            <Row label={t('HTTPS port')}>{info.ports ? (info.ports.https ?? t('Off')) : '?'}</Row>
          </dl>

          {/* Read again (Refresh): a new section with what the server said. */}
          {info.gateway && <GatewaySection key={JSON.stringify(info.gateway)} token={token} apiBase={apiBase} initial={info.gateway} />}

          <h3 className="ssd-section">{t('Storage')}</h3>
          <dl className="ssd-card ssd-info">
            <Row label={t('Data folder')}><code className="server-info-path">{info.dataDir || '?'}</code></Row>
          </dl>
          <p className="form-hint server-info-hint">{t('The database, the keys and the connection token are in this folder: keep it safe and include it in backups.')}</p>

          <h3 className="ssd-section">{t('Project')}</h3>
          <Links info={info} />
        </div>
      )}
    </div>
  );
}

/** "About Lunaris" for everyone: the versions and the project's links. */
export function AboutDialog({ token, apiBase, onClose }: { token: string | null; apiBase: () => string; onClose: () => void }) {
  const { info } = useServerInfo(token, apiBase);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <div className="host-confirm-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="host-confirm-card about-card" role="dialog" aria-modal="true" aria-labelledby="aboutTitle">
        <div className="host-confirm-icon" aria-hidden="true">
          <img src="/favicon.svg" alt="" width={26} height={26} />
        </div>
        <div className="host-confirm-content">
          <h2 id="aboutTitle">{t('About Lunaris')}</h2>
          <p>{t('Open-source remote desktop: Lunaris computers and RustDesk, Sunshine, RDP and VNC ones, from a browser or the desktop app.')}</p>
          <dl className="ssd-card ssd-info about-versions">
            <Row label={t('Web page version')}>{webVersion() || '?'}</Row>
            {token && <Row label={t('Server version')}>{info?.version || '…'}</Row>}
          </dl>
          <Links info={info ?? { version: '', links: DEFAULT_LINKS }} />
        </div>
        <div className="host-confirm-actions">
          <button type="button" className="btn-primary" autoFocus onClick={onClose}>{t('Close')}</button>
        </div>
      </div>
    </div>
  );
}

