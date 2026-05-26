import React, { useState, useEffect } from 'react';

interface AdminGroupsProps {
  token: string;
  getBackendHost: () => string;
  getBackendProtocol: () => { http: string; ws: string };
}

interface Group {
  id: number;
  name: string;
  note?: string;
  users?: { id: number; username: string }[];
  hosts?: { id: string; name: string }[];
  user_count?: number;
  host_count?: number;
}

interface UserItem {
  id: number;
  username: string;
}

interface HostItem {
  id: string;
  name: string;
  status: string;
}

export function AdminGroups({ token, getBackendHost, getBackendProtocol }: AdminGroupsProps) {
  const [groups, setGroups] = useState<Group[]>([]);
  const [allUsers, setAllUsers] = useState<UserItem[]>([]);
  const [allHosts, setAllHosts] = useState<HostItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Create modal
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [createName, setCreateName] = useState('');
  const [createNote, setCreateNote] = useState('');
  const [createLoading, setCreateLoading] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // Detail view
  const [selectedGroup, setSelectedGroup] = useState<Group | null>(null);
  const [detailUserIds, setDetailUserIds] = useState<number[]>([]);
  const [detailHostIds, setDetailHostIds] = useState<string[]>([]);
  const [detailSaving, setDetailSaving] = useState(false);
  const [detailSaved, setDetailSaved] = useState(false);

  // Edit modal
  const [editGroup, setEditGroup] = useState<Group | null>(null);
  const [editName, setEditName] = useState('');
  const [editNote, setEditNote] = useState('');
  const [editLoading, setEditLoading] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);

  // Delete confirm
  const [deleteGroup, setDeleteGroup] = useState<Group | null>(null);
  const [deleteLoading, setDeleteLoading] = useState(false);

  const apiBase = () => {
    const serverHost = getBackendHost();
    return `${getBackendProtocol().http}//${serverHost}`;
  };

  const fetchGroups = async () => {
    try {
      const response = await fetch(`${apiBase()}/api/admin/groups`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        const data = await response.json();
        setGroups(data);
      } else {
        setError('Failed to fetch groups');
      }
    } catch (err) {
      setError('Connection error while fetching groups');
    } finally {
      setLoading(false);
    }
  };

  const fetchAllUsers = async () => {
    try {
      const response = await fetch(`${apiBase()}/api/admin/users`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        const data = await response.json();
        setAllUsers(data);
      }
    } catch (err) {
      console.error('Failed to fetch users:', err);
    }
  };

  const fetchAllHosts = async () => {
    try {
      const response = await fetch(`${apiBase()}/api/hosts`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        const data = await response.json();
        setAllHosts(data);
      }
    } catch (err) {
      console.error('Failed to fetch hosts:', err);
    }
  };

  useEffect(() => {
    fetchGroups();
    fetchAllUsers();
    fetchAllHosts();
  }, []);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setCreateError(null);
    if (!createName.trim()) {
      setCreateError('Group name is required');
      return;
    }
    setCreateLoading(true);
    try {
      const response = await fetch(`${apiBase()}/api/admin/groups`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`
        },
        body: JSON.stringify({ name: createName, note: createNote })
      });
      if (response.ok) {
        setShowCreateModal(false);
        setCreateName('');
        setCreateNote('');
        fetchGroups();
      } else {
        const data = await response.json();
        setCreateError(data.error || 'Failed to create group');
      }
    } catch (err) {
      setCreateError('Connection error');
    } finally {
      setCreateLoading(false);
    }
  };

  const openDetail = (group: Group) => {
    setSelectedGroup(group);
    setDetailUserIds(group.users?.map(u => u.id) || []);
    setDetailHostIds(group.hosts?.map(h => h.id) || []);
    setDetailSaved(false);
  };

  const handleSaveMembership = async () => {
    if (!selectedGroup) return;
    setDetailSaving(true);
    setDetailSaved(false);
    try {
      const response = await fetch(`${apiBase()}/api/admin/groups/${selectedGroup.id}`, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`
        },
        body: JSON.stringify({
          name: selectedGroup.name,
          note: selectedGroup.note || '',
          user_ids: detailUserIds,
          host_ids: detailHostIds
        })
      });
      if (response.ok) {
        setDetailSaved(true);
        fetchGroups();
        setTimeout(() => setDetailSaved(false), 2000);
      }
    } catch (err) {
      console.error('Failed to save membership:', err);
    } finally {
      setDetailSaving(false);
    }
  };

  const openEditModal = (group: Group, e: React.MouseEvent) => {
    e.stopPropagation();
    setEditGroup(group);
    setEditName(group.name);
    setEditNote(group.note || '');
    setEditError(null);
  };

  const handleEdit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!editGroup) return;
    setEditError(null);
    setEditLoading(true);
    try {
      const response = await fetch(`${apiBase()}/api/admin/groups/${editGroup.id}`, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`
        },
        body: JSON.stringify({ name: editName, note: editNote })
      });
      if (response.ok) {
        setEditGroup(null);
        fetchGroups();
        if (selectedGroup?.id === editGroup.id) {
          setSelectedGroup({ ...selectedGroup, name: editName, note: editNote });
        }
      } else {
        const data = await response.json();
        setEditError(data.error || 'Failed to update group');
      }
    } catch (err) {
      setEditError('Connection error');
    } finally {
      setEditLoading(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteGroup) return;
    setDeleteLoading(true);
    try {
      const response = await fetch(`${apiBase()}/api/admin/groups/${deleteGroup.id}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        if (selectedGroup?.id === deleteGroup.id) {
          setSelectedGroup(null);
        }
        setDeleteGroup(null);
        fetchGroups();
      }
    } catch (err) {
      console.error('Failed to delete group:', err);
    } finally {
      setDeleteLoading(false);
    }
  };

  const toggleDetailUser = (uid: number) => {
    setDetailUserIds(prev =>
      prev.includes(uid) ? prev.filter(id => id !== uid) : [...prev, uid]
    );
  };

  const toggleDetailHost = (hid: string) => {
    setDetailHostIds(prev =>
      prev.includes(hid) ? prev.filter(id => id !== hid) : [...prev, hid]
    );
  };

  if (loading) {
    return (
      <div className="loading-card">
        <div className="tech-loader"></div>
        <div>Loading groups...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="error-card">
        <div className="error-title">⚠️ Error</div>
        <div className="error-desc">{error}</div>
        <button onClick={() => { setError(null); setLoading(true); fetchGroups(); }} className="btn-secondary">Retry</button>
      </div>
    );
  }

  return (
    <div className="admin-page-container">
      <div className="section-header">
        <div>
          <h1 className="section-title">Group Management</h1>
          <p className="section-subtitle">Organize users and hosts into access groups</p>
        </div>
        <button onClick={() => setShowCreateModal(true)} className="btn-primary" style={{ padding: '0.6rem 1.2rem', fontSize: '0.9rem' }}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <line x1="12" y1="5" x2="12" y2="19" />
            <line x1="5" y1="12" x2="19" y2="12" />
          </svg>
          Add Group
        </button>
      </div>

      <div className="admin-groups-layout">
        {/* Groups List */}
        <div className="admin-groups-list">
          {groups.length === 0 ? (
            <div className="empty-card" style={{ padding: '3rem' }}>
              <div className="empty-icon">📁</div>
              <h3>No Groups</h3>
              <p>Create a group to organize users and hosts.</p>
            </div>
          ) : (
            <div className="admin-group-cards">
              {groups.map(group => (
                <div
                  key={group.id}
                  className={`admin-group-card ${selectedGroup?.id === group.id ? 'selected' : ''}`}
                  onClick={() => openDetail(group)}
                >
                  <div className="admin-group-card-glow"></div>
                  <div className="admin-group-card-header">
                    <div>
                      <h3 className="admin-group-name">{group.name}</h3>
                      {group.note && <p className="admin-group-note">{group.note}</p>}
                    </div>
                    <div className="admin-group-card-actions" onClick={e => e.stopPropagation()}>
                      <button
                        onClick={(e) => openEditModal(group, e)}
                        className="admin-action-btn edit"
                        title="Edit Group"
                      >
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                          <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
                          <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
                        </svg>
                      </button>
                      <button
                        onClick={(e) => { e.stopPropagation(); setDeleteGroup(group); }}
                        className="admin-action-btn delete"
                        title="Delete Group"
                      >
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                          <polyline points="3 6 5 6 21 6" />
                          <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                        </svg>
                      </button>
                    </div>
                  </div>
                  <div className="admin-group-card-stats">
                    <div className="admin-group-stat">
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/>
                        <circle cx="12" cy="7" r="4"/>
                      </svg>
                      <span>{group.users?.length ?? group.user_count ?? 0} users</span>
                    </div>
                    <div className="admin-group-stat">
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <rect x="2" y="3" width="20" height="14" rx="2" ry="2"/>
                        <line x1="8" y1="21" x2="16" y2="21"/>
                        <line x1="12" y1="17" x2="12" y2="21"/>
                      </svg>
                      <span>{group.hosts?.length ?? group.host_count ?? 0} hosts</span>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Detail Panel */}
        {selectedGroup && (
          <div className="group-detail-panel">
            <div className="group-detail-header">
              <div>
                <h3>{selectedGroup.name}</h3>
                {selectedGroup.note && <p style={{ color: 'var(--text-muted)', fontSize: '0.85rem', marginTop: '0.25rem' }}>{selectedGroup.note}</p>}
              </div>
              <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center' }}>
                {detailSaved && (
                  <span style={{ color: '#10b981', fontSize: '0.85rem', fontWeight: 600 }}>✓ Saved</span>
                )}
                <button
                  onClick={handleSaveMembership}
                  disabled={detailSaving}
                  className="btn-primary"
                  style={{ padding: '0.5rem 1rem', fontSize: '0.85rem' }}
                >
                  {detailSaving ? <div className="inline-loader"></div> : 'Save Membership'}
                </button>
              </div>
            </div>
            <div className="group-detail-columns">
              <div className="membership-column">
                <h4 className="membership-column-title">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/>
                    <circle cx="12" cy="7" r="4"/>
                  </svg>
                  Users ({detailUserIds.length})
                </h4>
                <div className="membership-panel">
                  {allUsers.length === 0 ? (
                    <div style={{ color: 'var(--text-muted)', fontSize: '0.85rem', padding: '1rem', textAlign: 'center' }}>No users available</div>
                  ) : allUsers.map(u => (
                    <label key={u.id} className="membership-item">
                      <input
                        type="checkbox"
                        checked={detailUserIds.includes(u.id)}
                        onChange={() => toggleDetailUser(u.id)}
                      />
                      <span>{u.username}</span>
                    </label>
                  ))}
                </div>
              </div>
              <div className="membership-column">
                <h4 className="membership-column-title">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <rect x="2" y="3" width="20" height="14" rx="2" ry="2"/>
                    <line x1="8" y1="21" x2="16" y2="21"/>
                    <line x1="12" y1="17" x2="12" y2="21"/>
                  </svg>
                  Hosts ({detailHostIds.length})
                </h4>
                <div className="membership-panel">
                  {allHosts.length === 0 ? (
                    <div style={{ color: 'var(--text-muted)', fontSize: '0.85rem', padding: '1rem', textAlign: 'center' }}>No hosts available</div>
                  ) : allHosts.map(h => (
                    <label key={h.id} className="membership-item">
                      <input
                        type="checkbox"
                        checked={detailHostIds.includes(h.id)}
                        onChange={() => toggleDetailHost(h.id)}
                      />
                      <span>{h.name}</span>
                    </label>
                  ))}
                </div>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Create Group Modal */}
      {showCreateModal && (
        <div className="admin-modal-overlay" onClick={() => setShowCreateModal(false)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()}>
            <div className="admin-modal-header">
              <h3>Create New Group</h3>
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
                  <label>Group Name</label>
                  <input
                    type="text"
                    value={createName}
                    onChange={e => setCreateName(e.target.value)}
                    placeholder="Enter group name"
                    required
                  />
                </div>
                <div className="form-group">
                  <label>Note (optional)</label>
                  <input
                    type="text"
                    value={createNote}
                    onChange={e => setCreateNote(e.target.value)}
                    placeholder="Short description"
                  />
                </div>
              </div>
              <div className="admin-modal-footer">
                <button type="button" onClick={() => setShowCreateModal(false)} className="btn-secondary">Cancel</button>
                <button type="submit" disabled={createLoading} className="btn-primary">
                  {createLoading ? <div className="inline-loader"></div> : 'Create Group'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Edit Group Modal */}
      {editGroup && (
        <div className="admin-modal-overlay" onClick={() => setEditGroup(null)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()}>
            <div className="admin-modal-header">
              <h3>Edit Group — {editGroup.name}</h3>
              <button onClick={() => setEditGroup(null)} className="btn-close-modal">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
            <form onSubmit={handleEdit}>
              <div className="admin-modal-body">
                {editError && (
                  <div className="auth-error-banner">
                    <span className="error-icon">⚠️</span>
                    <span>{editError}</span>
                  </div>
                )}
                <div className="form-group">
                  <label>Group Name</label>
                  <input
                    type="text"
                    value={editName}
                    onChange={e => setEditName(e.target.value)}
                    placeholder="Enter group name"
                    required
                  />
                </div>
                <div className="form-group">
                  <label>Note</label>
                  <input
                    type="text"
                    value={editNote}
                    onChange={e => setEditNote(e.target.value)}
                    placeholder="Short description"
                  />
                </div>
              </div>
              <div className="admin-modal-footer">
                <button type="button" onClick={() => setEditGroup(null)} className="btn-secondary">Cancel</button>
                <button type="submit" disabled={editLoading} className="btn-primary">
                  {editLoading ? <div className="inline-loader"></div> : 'Save Changes'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Delete Confirm Modal */}
      {deleteGroup && (
        <div className="admin-modal-overlay" onClick={() => setDeleteGroup(null)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()} style={{ maxWidth: '420px' }}>
            <div className="admin-modal-header">
              <h3>Confirm Delete</h3>
              <button onClick={() => setDeleteGroup(null)} className="btn-close-modal">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
            <div className="admin-modal-body">
              <p style={{ color: 'var(--text-muted)', fontSize: '0.95rem', lineHeight: 1.6 }}>
                Are you sure you want to delete group <strong style={{ color: '#f43f5e' }}>{deleteGroup.name}</strong>? All membership associations will be removed.
              </p>
            </div>
            <div className="admin-modal-footer">
              <button onClick={() => setDeleteGroup(null)} className="btn-secondary">Cancel</button>
              <button
                onClick={handleDelete}
                disabled={deleteLoading}
                className="btn-primary"
                style={{ background: 'linear-gradient(135deg, #ef4444, #dc2626)', boxShadow: '0 4px 15px rgba(239, 68, 68, 0.2)' }}
              >
                {deleteLoading ? <div className="inline-loader"></div> : 'Delete Group'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
