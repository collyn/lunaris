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

export function AdminTurnServers({ token, getBackendHost, getBackendProtocol }: AdminTurnServersProps) {
  const [servers, setServers] = useState<TurnServer[]>([]);
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

  useEffect(() => {
    fetchServers();
  }, []);

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
            {servers.length === 0 && (
              <tr>
                <td colSpan={5} style={{ textAlign: 'center', padding: '3rem', color: 'var(--text-muted)' }}>
                  {t('No TURN servers. Connections use public STUN only, which can fail behind strict networks.')}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

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
