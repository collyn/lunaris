import React, { useState, useEffect } from 'react';
import { t } from '../i18n';

interface AdminTurnServersProps {
  token: string;
  getBackendHost: () => string;
  getBackendProtocol: () => { http: string; ws: string };
}

interface TurnServer {
  id: string;
  urls: string;
  username?: string;
  credential?: string;
  created_at?: string;
}

/** The relay the server runs itself (server/src/turn.rs). */
interface ActiveRelay {
  client: string;
  relay: string;
  bytes: number;
  since: number;
}

interface BuiltinTurn {
  enabled: boolean;
  off_reason?: string | null;
  port?: number | null;
  relay_ports?: [number, number] | null;
  public_ip?: string | null;
  urls: string[];
  started_at?: number | null;
  total_allocations: number;
  total_bytes: number;
  active: ActiveRelay[];
}

/** turn:host:port without the transport suffix (the relay is UDP only). */
const shortUrl = (url: string) => url.replace(/\?transport=udp$/, '');

const formatBytes = (n: number) => {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
};

const formatSince = (unix: number) => {
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (secs < 60) return t('%1 s', String(secs));
  if (secs < 3600) return t('%1 min', String(Math.floor(secs / 60)));
  return t('%1 h %2 min', String(Math.floor(secs / 3600)), String(Math.floor((secs % 3600) / 60)));
};

export function AdminTurnServers({ token, getBackendHost, getBackendProtocol }: AdminTurnServersProps) {
  const [servers, setServers] = useState<TurnServer[]>([]);
  const [builtin, setBuiltin] = useState<BuiltinTurn | null>(null);
  const [showBuiltin, setShowBuiltin] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Create modal
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [createUrls, setCreateUrls] = useState('');
  const [createUsername, setCreateUsername] = useState('');
  const [createCredential, setCreateCredential] = useState('');
  const [createLoading, setCreateLoading] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // Delete confirm
  const [deleteServer, setDeleteServer] = useState<TurnServer | null>(null);
  const [deleteLoading, setDeleteLoading] = useState(false);

  const apiBase = () => {
    const serverHost = getBackendHost();
    return `${getBackendProtocol().http}//${serverHost}`;
  };

  const fetchServers = async () => {
    try {
      const response = await fetch(`${apiBase()}/api/admin/turn-servers`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        const data = await response.json();
        setServers(data);
      } else {
        setError(t('Failed to fetch TURN servers'));
      }
    } catch (err) {
      setError(t('Connection error while fetching TURN servers'));
    } finally {
      setLoading(false);
    }
  };

  const fetchBuiltin = async () => {
    try {
      const response = await fetch(`${apiBase()}/api/admin/turn-servers/builtin`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) setBuiltin(await response.json());
    } catch {
      // An older server has no built-in status: the row is left out.
    }
  };

  useEffect(() => {
    fetchServers();
    fetchBuiltin();
  }, []);

  // Live while the details are open, otherwise every 15 s for the row.
  useEffect(() => {
    const timer = window.setInterval(fetchBuiltin, showBuiltin ? 3000 : 15000);
    return () => window.clearInterval(timer);
  }, [showBuiltin]);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setCreateError(null);
    if (!createUrls.trim()) {
      setCreateError(t('Server URL is required'));
      return;
    }
    
    // Simple validation for stun: or turn: schemes
    const urls = createUrls.trim();
    if (!urls.startsWith('stun:') && !urls.startsWith('turn:') && !urls.startsWith('turns:')) {
      setCreateError(t('The address must start with stun:, turn: or turns: (e.g. turn:turn.example.com:3478)'));
      return;
    }

    setCreateLoading(true);
    try {
      const response = await fetch(`${apiBase()}/api/admin/turn-servers`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`
        },
        body: JSON.stringify({
          urls,
          username: createUsername.trim() || undefined,
          credential: createCredential.trim() || undefined
        })
      });
      if (response.ok) {
        setShowCreateModal(false);
        setCreateUrls('');
        setCreateUsername('');
        setCreateCredential('');
        fetchServers();
      } else {
        const data = await response.json();
        setCreateError(t(data.error || 'Failed to add TURN server'));
      }
    } catch (err) {
      setCreateError(t('Connection error'));
    } finally {
      setCreateLoading(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteServer) return;
    setDeleteLoading(true);
    try {
      const response = await fetch(`${apiBase()}/api/admin/turn-servers/${deleteServer.id}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        setDeleteServer(null);
        fetchServers();
      } else {
        alert(t('Failed to delete TURN server'));
      }
    } catch (err) {
      console.error('Failed to delete TURN server:', err);
    } finally {
      setDeleteLoading(false);
    }
  };

  if (loading) {
    return (
      <div className="loading-card">
        <div className="tech-loader"></div>
        <div>{t('Loading TURN servers...')}</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="error-card">
        <div className="error-title">⚠️ Error</div>
        <div className="error-desc">{error}</div>
        <button onClick={() => { setError(null); setLoading(true); fetchServers(); }} className="btn-secondary">Retry</button>
      </div>
    );
  }

  return (
    <div className="admin-page-container">
      <div className="section-header">
        <div>
          <h1 className="section-title">{t('TURN/STUN Server Configuration')}</h1>
          <p className="section-subtitle">{t('Manage external relay servers used to negotiate connection traversal across Symmetric NATs')}</p>
        </div>
        <button onClick={() => setShowCreateModal(true)} className="btn-primary" style={{ padding: '0.6rem 1.2rem', fontSize: '0.9rem' }}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <line x1="12" y1="5" x2="12" y2="19" />
            <line x1="5" y1="12" x2="19" y2="12" />
          </svg>
          {t('Add server')}
        </button>
      </div>

      <div className="admin-table-container">
        <table className="admin-table">
          <thead>
            <tr>
              <th>{t('Server URL')}</th>
              <th>{t('Username')}</th>
              <th>{t('Credential / Secret')}</th>
              <th>{t('Added On')}</th>
              <th>{t('Actions')}</th>
            </tr>
          </thead>
          <tbody>
            {builtin && (
              <tr>
                <td>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', flexWrap: 'wrap' }}>
                    <span className="badge-tech">{t('Built-in')}</span>
                    {builtin.enabled
                      ? builtin.urls.map(u => (
                        <span key={u} style={{ fontWeight: 600, color: 'var(--text-main)' }}>{shortUrl(u)}</span>
                      ))
                      : <span style={{ fontWeight: 600, color: 'var(--text-main)' }}>{t('Relay on this server')}</span>}
                  </div>
                  <div className="host-status-badge" style={{ marginTop: '0.35rem', fontSize: '0.8rem', color: 'var(--text-muted)' }}>
                    <span className={`status-indicator ${builtin.enabled ? 'online' : 'offline'}`} />
                    {builtin.enabled
                      ? (builtin.active.length > 0
                        ? t('Running · %1 relaying now', String(builtin.active.length))
                        : t('Running · idle'))
                      : t('Off')}
                  </div>
                </td>
                <td>
                  <span style={{ fontSize: '0.8rem', color: 'var(--text-muted)' }}>{t('Automatic')}</span>
                </td>
                <td>
                  <span style={{ fontSize: '0.8rem', color: 'var(--text-muted)' }}>{t('Temporary, per session')}</span>
                </td>
                <td>
                  <span style={{ fontSize: '0.8rem', color: 'var(--text-muted)', fontFamily: 'monospace' }}>
                    {builtin.started_at ? new Date(builtin.started_at * 1000).toLocaleString() : '—'}
                  </span>
                </td>
                <td>
                  <button onClick={() => { setShowBuiltin(true); fetchBuiltin(); }} className="btn-secondary" style={{ padding: '0.35rem 0.8rem', fontSize: '0.8rem', whiteSpace: 'nowrap' }}>
                    {t('Details')}
                  </button>
                </td>
              </tr>
            )}
            {servers.map(server => (
              <tr key={server.id}>
                <td>
                  <span style={{ fontWeight: 600, color: 'var(--text-main)' }}>{server.urls}</span>
                </td>
                <td>
                  <span style={{ fontSize: '0.9rem' }}>{server.username || <span style={{ color: 'var(--text-muted)', fontSize: '0.8rem' }}>—</span>}</span>
                </td>
                <td>
                  <span style={{ fontSize: '0.9rem', fontFamily: 'monospace' }}>
                    {server.credential ? '••••••••' : <span style={{ color: 'var(--text-muted)', fontSize: '0.8rem' }}>—</span>}
                  </span>
                </td>
                <td>
                  <span style={{ fontSize: '0.8rem', color: 'var(--text-muted)', fontFamily: 'monospace' }}>
                    {server.created_at ? new Date(server.created_at).toLocaleString() : '—'}
                  </span>
                </td>
                <td>
                  <div style={{ display: 'flex', gap: '0.5rem' }}>
                    <button
                      onClick={() => setDeleteServer(server)}
                      className="admin-action-btn delete"
                      title={t('Delete Server')}
                    >
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <polyline points="3 6 5 6 21 6" />
                        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                      </svg>
                    </button>
                  </div>
                </td>
              </tr>
            ))}
            {servers.length === 0 && !builtin?.enabled && (
              <tr>
                <td colSpan={5} style={{ textAlign: 'center', padding: '3rem', color: 'var(--text-muted)' }}>
                  {t('No TURN servers. Connections use public STUN only, which can fail behind strict networks.')}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* Built-in relay details */}
      {showBuiltin && builtin && (
        <div className="admin-modal-overlay" onClick={() => setShowBuiltin(false)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()} style={{ maxWidth: '680px' }}>
            <div className="admin-modal-header">
              <h3>{t('Built-in TURN relay')}</h3>
              <button onClick={() => setShowBuiltin(false)} className="btn-close-modal">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
            <div className="admin-modal-body" style={{ display: 'flex', flexDirection: 'column', gap: '1rem', maxHeight: 'calc(100vh - 220px)', overflowY: 'auto' }}>
              <p style={{ color: 'var(--text-muted)', fontSize: '0.9rem', lineHeight: 1.6, margin: 0 }}>
                {t('The server relays the stream itself when the two computers cannot reach each other directly. Each session gets temporary credentials; nothing needs to be set up here.')}
              </p>
              <div className="host-status-badge" style={{ fontWeight: 600 }}>
                <span className={`status-indicator ${builtin.enabled ? 'online' : 'offline'}`} />
                {builtin.enabled ? t('Running') : t('Off')}
              </div>
              {!builtin.enabled && builtin.off_reason && (
                <div className="auth-error-banner">
                  <span className="error-icon">⚠️</span>
                  <span>{builtin.off_reason}</span>
                </div>
              )}
              {builtin.enabled && (
                <>
                  <table className="admin-table" style={{ fontSize: '0.85rem' }}>
                    <tbody>
                      <tr><td>{t('Addresses handed to peers')}</td><td style={{ fontFamily: 'monospace' }}>{builtin.urls.map(u => <div key={u}>{shortUrl(u)}</div>)}</td></tr>
                      <tr><td>{t('Listening port')}</td><td style={{ fontFamily: 'monospace' }}>UDP {builtin.port}</td></tr>
                      <tr><td>{t('Relay ports')}</td><td style={{ fontFamily: 'monospace' }}>UDP {builtin.relay_ports?.[0]}–{builtin.relay_ports?.[1]}</td></tr>
                      <tr><td>{t('Public address')}</td><td style={{ fontFamily: 'monospace' }}>{builtin.public_ip || t('Not set (every local address listens)')}</td></tr>
                      <tr><td>{t('Running since')}</td><td>{builtin.started_at ? new Date(builtin.started_at * 1000).toLocaleString() : '—'}</td></tr>
                      <tr><td>{t('Relay sessions since start')}</td><td>{builtin.total_allocations}</td></tr>
                      <tr><td>{t('Data relayed since start')}</td><td>{formatBytes(builtin.total_bytes)}</td></tr>
                    </tbody>
                  </table>
                  <div>
                    <div style={{ fontWeight: 600, marginBottom: '0.5rem' }}>
                      {t('Relaying now (%1)', String(builtin.active.length))}
                    </div>
                    {builtin.active.length === 0 ? (
                      <div style={{ color: 'var(--text-muted)', fontSize: '0.85rem' }}>
                        {t('Nobody is relaying: current connections go directly.')}
                      </div>
                    ) : (
                      <table className="admin-table" style={{ fontSize: '0.85rem' }}>
                        <thead>
                          <tr>
                            <th>{t('Peer')}</th>
                            <th>{t('Relay address')}</th>
                            <th>{t('Data')}</th>
                            <th>{t('For')}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {builtin.active.map(a => (
                            <tr key={`${a.client}-${a.relay}`}>
                              <td style={{ fontFamily: 'monospace' }}>{a.client}</td>
                              <td style={{ fontFamily: 'monospace' }}>{a.relay}</td>
                              <td>{formatBytes(a.bytes)}</td>
                              <td>{a.since ? formatSince(a.since) : '—'}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
                  </div>
                  <p style={{ color: 'var(--text-muted)', fontSize: '0.8rem', lineHeight: 1.6, margin: 0 }}>
                    {t('Peers reach the relay on these UDP ports: open them in the firewall (and forward them on the router for connections from the internet).')}
                  </p>
                </>
              )}
            </div>
            <div className="admin-modal-footer">
              <button onClick={() => setShowBuiltin(false)} className="btn-secondary">{t('Close')}</button>
            </div>
          </div>
        </div>
      )}

      {/* Create TURN Server Modal */}
      {showCreateModal && (
        <div className="admin-modal-overlay" onClick={() => setShowCreateModal(false)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()}>
            <div className="admin-modal-header">
              <h3>{t('Add TURN/STUN Server')}</h3>
              <button onClick={() => setShowCreateModal(false)} className="btn-close-modal">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
            <form onSubmit={handleCreate}>
              <div className="admin-modal-body">
                {createError && (
                  <div className="auth-error-banner">
                    <span className="error-icon">⚠️</span>
                    <span>{createError}</span>
                  </div>
                )}
                <div className="form-group">
                  <label>{t('Server URL')}</label>
                  <input
                    type="text"
                    value={createUrls}
                    onChange={e => setCreateUrls(e.target.value)}
                    placeholder={t('e.g. turn:my-turn-server.com:3478')}
                    required
                  />
                  <small style={{ color: 'var(--text-muted)', display: 'block', marginTop: '4px' }}>
                    {t('Scheme must be')} <code>stun:</code>, <code>turn:</code>, <code>turns:</code>.
                  </small>
                </div>
                <div className="form-group">
                  <label>{t('Username (Optional)')}</label>
                  <input
                    type="text"
                    value={createUsername}
                    onChange={e => setCreateUsername(e.target.value)}
                    placeholder={t('Enter username if required')}
                  />
                </div>
                <div className="form-group">
                  <label>{t('Credential / Password (Optional)')}</label>
                  <input
                    type="password"
                    value={createCredential}
                    onChange={e => setCreateCredential(e.target.value)}
                    placeholder={t('Enter password/secret if required')}
                  />
                </div>
              </div>
              <div className="admin-modal-footer">
                <button type="button" onClick={() => setShowCreateModal(false)} className="btn-secondary">{t('Cancel')}</button>
                <button type="submit" disabled={createLoading} className="btn-primary">
                  {createLoading ? <div className="inline-loader"></div> : t('Add Server')}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Delete Confirm Modal */}
      {deleteServer && (
        <div className="admin-modal-overlay" onClick={() => setDeleteServer(null)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()} style={{ maxWidth: '420px' }}>
            <div className="admin-modal-header">
              <h3>{t('Confirm Delete')}</h3>
              <button onClick={() => setDeleteServer(null)} className="btn-close-modal">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
            <div className="admin-modal-body">
              <p style={{ color: 'var(--text-muted)', fontSize: '0.95rem', lineHeight: 1.6 }}>
                {t('%1 will no longer be offered to new connections.', deleteServer.urls)}
              </p>
            </div>
            <div className="admin-modal-footer">
              <button onClick={() => setDeleteServer(null)} className="btn-secondary">{t('Cancel')}</button>
              <button
                onClick={handleDelete}
                disabled={deleteLoading}
                className="btn-primary"
                style={{ background: 'linear-gradient(135deg, #ef4444, #dc2626)', boxShadow: '0 4px 15px rgba(239, 68, 68, 0.2)' }}
              >
                {deleteLoading ? <div className="inline-loader"></div> : t('Delete Server')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
