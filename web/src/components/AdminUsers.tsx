import React, { useState, useEffect } from 'react';

interface AdminUsersProps {
  token: string;
  getBackendHost: () => string;
  getBackendProtocol: () => { http: string; ws: string };
}

interface User {
  id: number;
  username: string;
  role: string;
  is_active: boolean;
  groups?: { id: number; name: string }[];
  created_at?: string;
}

interface Group {
  id: number;
  name: string;
}

export function AdminUsers({ token, getBackendHost, getBackendProtocol }: AdminUsersProps) {
  const [users, setUsers] = useState<User[]>([]);
  const [groups, setGroups] = useState<Group[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Create modal
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [createUsername, setCreateUsername] = useState('');
  const [createPassword, setCreatePassword] = useState('');
  const [createRole, setCreateRole] = useState('user');
  const [createLoading, setCreateLoading] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // Edit modal
  const [editUser, setEditUser] = useState<User | null>(null);
  const [editRole, setEditRole] = useState('user');
  const [editIsActive, setEditIsActive] = useState(true);
  const [editPassword, setEditPassword] = useState('');
  const [editGroupIds, setEditGroupIds] = useState<number[]>([]);
  const [editLoading, setEditLoading] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);

  // Delete confirm
  const [deleteUser, setDeleteUser] = useState<User | null>(null);
  const [deleteLoading, setDeleteLoading] = useState(false);

  const apiBase = () => {
    const serverHost = getBackendHost();
    return `${getBackendProtocol().http}//${serverHost}`;
  };

  const fetchUsers = async () => {
    try {
      const response = await fetch(`${apiBase()}/api/admin/users`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        const data = await response.json();
        setUsers(data);
      } else {
        setError('Failed to fetch users');
      }
    } catch (err) {
      setError('Connection error while fetching users');
    } finally {
      setLoading(false);
    }
  };

  const fetchGroups = async () => {
    try {
      const response = await fetch(`${apiBase()}/api/admin/groups`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        const data = await response.json();
        setGroups(data);
      }
    } catch (err) {
      console.error('Failed to fetch groups:', err);
    }
  };

  useEffect(() => {
    fetchUsers();
    fetchGroups();
  }, []);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setCreateError(null);
    if (!createUsername.trim() || !createPassword.trim()) {
      setCreateError('Username and password are required');
      return;
    }
    setCreateLoading(true);
    try {
      const response = await fetch(`${apiBase()}/api/admin/users`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`
        },
        body: JSON.stringify({
          username: createUsername,
          password: createPassword,
          role: createRole
        })
      });
      if (response.ok) {
        setShowCreateModal(false);
        setCreateUsername('');
        setCreatePassword('');
        setCreateRole('user');
        fetchUsers();
      } else {
        const data = await response.json();
        setCreateError(data.error || 'Failed to create user');
      }
    } catch (err) {
      setCreateError('Connection error');
    } finally {
      setCreateLoading(false);
    }
  };

  const openEditModal = (user: User) => {
    setEditUser(user);
    setEditRole(user.role);
    setEditIsActive(user.is_active);
    setEditPassword('');
    setEditGroupIds(user.groups?.map(g => g.id) || []);
    setEditError(null);
  };

  const handleEdit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!editUser) return;
    setEditError(null);
    setEditLoading(true);
    try {
      const body: Record<string, unknown> = {
        role: editRole,
        is_active: editIsActive,
        group_ids: editGroupIds
      };
      if (editPassword.trim()) {
        body.password = editPassword;
      }
      const response = await fetch(`${apiBase()}/api/admin/users/${editUser.id}`, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`
        },
        body: JSON.stringify(body)
      });
      if (response.ok) {
        setEditUser(null);
        fetchUsers();
      } else {
        const data = await response.json();
        setEditError(data.error || 'Failed to update user');
      }
    } catch (err) {
      setEditError('Connection error');
    } finally {
      setEditLoading(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteUser) return;
    setDeleteLoading(true);
    try {
      const response = await fetch(`${apiBase()}/api/admin/users/${deleteUser.id}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        setDeleteUser(null);
        fetchUsers();
      }
    } catch (err) {
      console.error('Failed to delete user:', err);
    } finally {
      setDeleteLoading(false);
    }
  };

  const toggleGroupId = (gid: number) => {
    setEditGroupIds(prev =>
      prev.includes(gid) ? prev.filter(id => id !== gid) : [...prev, gid]
    );
  };

  if (loading) {
    return (
      <div className="loading-card">
        <div className="tech-loader"></div>
        <div>Loading users...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="error-card">
        <div className="error-title">⚠️ Error</div>
        <div className="error-desc">{error}</div>
        <button onClick={() => { setError(null); setLoading(true); fetchUsers(); }} className="btn-secondary">Retry</button>
      </div>
    );
  }

  return (
    <div className="admin-page-container">
      <div className="section-header">
        <div>
          <h1 className="section-title">User Management</h1>
          <p className="section-subtitle">Create, edit, and manage user accounts and their permissions</p>
        </div>
        <button onClick={() => setShowCreateModal(true)} className="btn-primary" style={{ padding: '0.6rem 1.2rem', fontSize: '0.9rem' }}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <line x1="12" y1="5" x2="12" y2="19" />
            <line x1="5" y1="12" x2="19" y2="12" />
          </svg>
          Add User
        </button>
      </div>

      <div className="admin-table-container">
        <table className="admin-table">
          <thead>
            <tr>
              <th>Username</th>
              <th>Role</th>
              <th>Status</th>
              <th>Groups</th>
              <th>Created</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {users.map(user => (
              <tr key={user.id}>
                <td>
                  <span style={{ fontWeight: 600, color: 'var(--text-main)' }}>{user.username}</span>
                </td>
                <td>
                  <span className={`role-badge ${user.role}`}>{user.role}</span>
                </td>
                <td>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
                    <span className={`status-dot ${user.is_active ? 'active' : 'inactive'}`}></span>
                    <span style={{ fontSize: '0.85rem' }}>{user.is_active ? 'Active' : 'Inactive'}</span>
                  </div>
                </td>
                <td>
                  <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.35rem' }}>
                    {user.groups && user.groups.length > 0 ? (
                      user.groups.map(g => (
                        <span key={g.id} className="group-chip">{g.name}</span>
                      ))
                    ) : (
                      <span style={{ color: 'var(--text-muted)', fontSize: '0.8rem' }}>—</span>
                    )}
                  </div>
                </td>
                <td>
                  <span style={{ fontSize: '0.8rem', color: 'var(--text-muted)', fontFamily: 'monospace' }}>
                    {user.created_at ? new Date(user.created_at).toLocaleDateString() : '—'}
                  </span>
                </td>
                <td>
                  <div style={{ display: 'flex', gap: '0.5rem' }}>
                    <button
                      onClick={() => openEditModal(user)}
                      className="admin-action-btn edit"
                      title="Edit User"
                    >
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
                        <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
                      </svg>
                    </button>
                    <button
                      onClick={() => setDeleteUser(user)}
                      className="admin-action-btn delete"
                      title="Delete User"
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
            {users.length === 0 && (
              <tr>
                <td colSpan={6} style={{ textAlign: 'center', padding: '3rem', color: 'var(--text-muted)' }}>
                  No users found. Click "Add User" to create one.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* Create User Modal */}
      {showCreateModal && (
        <div className="admin-modal-overlay" onClick={() => setShowCreateModal(false)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()}>
            <div className="admin-modal-header">
              <h3>Create New User</h3>
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
                  <label>Username</label>
                  <input
                    type="text"
                    value={createUsername}
                    onChange={e => setCreateUsername(e.target.value)}
                    placeholder="Enter username"
                    required
                  />
                </div>
                <div className="form-group">
                  <label>Password</label>
                  <input
                    type="password"
                    value={createPassword}
                    onChange={e => setCreatePassword(e.target.value)}
                    placeholder="Enter password (min 6 chars)"
                    required
                  />
                </div>
                <div className="form-group">
                  <label>Role</label>
                  <select
                    value={createRole}
                    onChange={e => setCreateRole(e.target.value)}
                    className="admin-select"
                  >
                    <option value="user">User</option>
                    <option value="admin">Admin</option>
                  </select>
                </div>
              </div>
              <div className="admin-modal-footer">
                <button type="button" onClick={() => setShowCreateModal(false)} className="btn-secondary">Cancel</button>
                <button type="submit" disabled={createLoading} className="btn-primary">
                  {createLoading ? <div className="inline-loader"></div> : 'Create User'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Edit User Modal */}
      {editUser && (
        <div className="admin-modal-overlay" onClick={() => setEditUser(null)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()}>
            <div className="admin-modal-header">
              <h3>Edit User — {editUser.username}</h3>
              <button onClick={() => setEditUser(null)} className="btn-close-modal">
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
                  <label>Role</label>
                  <select
                    value={editRole}
                    onChange={e => setEditRole(e.target.value)}
                    className="admin-select"
                  >
                    <option value="user">User</option>
                    <option value="admin">Admin</option>
                  </select>
                </div>
                <div className="form-group">
                  <label style={{ marginBottom: '0.25rem' }}>Status</label>
                  <label className="admin-toggle-label">
                    <input
                      type="checkbox"
                      checked={editIsActive}
                      onChange={e => setEditIsActive(e.target.checked)}
                    />
                    <span>{editIsActive ? 'Active' : 'Inactive'}</span>
                  </label>
                </div>
                <div className="form-group">
                  <label>Reset Password (leave blank to keep current)</label>
                  <input
                    type="password"
                    value={editPassword}
                    onChange={e => setEditPassword(e.target.value)}
                    placeholder="New password"
                  />
                </div>
                {groups.length > 0 && (
                  <div className="form-group">
                    <label>Groups</label>
                    <div className="membership-panel" style={{ maxHeight: '160px' }}>
                      {groups.map(g => (
                        <label key={g.id} className="membership-item">
                          <input
                            type="checkbox"
                            checked={editGroupIds.includes(g.id)}
                            onChange={() => toggleGroupId(g.id)}
                          />
                          <span>{g.name}</span>
                        </label>
                      ))}
                    </div>
                  </div>
                )}
              </div>
              <div className="admin-modal-footer">
                <button type="button" onClick={() => setEditUser(null)} className="btn-secondary">Cancel</button>
                <button type="submit" disabled={editLoading} className="btn-primary">
                  {editLoading ? <div className="inline-loader"></div> : 'Save Changes'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Delete Confirm Modal */}
      {deleteUser && (
        <div className="admin-modal-overlay" onClick={() => setDeleteUser(null)}>
          <div className="admin-modal" onClick={e => e.stopPropagation()} style={{ maxWidth: '420px' }}>
            <div className="admin-modal-header">
              <h3>Confirm Delete</h3>
              <button onClick={() => setDeleteUser(null)} className="btn-close-modal">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
            <div className="admin-modal-body">
              <p style={{ color: 'var(--text-muted)', fontSize: '0.95rem', lineHeight: 1.6 }}>
                Are you sure you want to delete user <strong style={{ color: '#f43f5e' }}>{deleteUser.username}</strong>? This action cannot be undone.
              </p>
            </div>
            <div className="admin-modal-footer">
              <button onClick={() => setDeleteUser(null)} className="btn-secondary">Cancel</button>
              <button
                onClick={handleDelete}
                disabled={deleteLoading}
                className="btn-primary"
                style={{ background: 'linear-gradient(135deg, #ef4444, #dc2626)', boxShadow: '0 4px 15px rgba(239, 68, 68, 0.2)' }}
              >
                {deleteLoading ? <div className="inline-loader"></div> : 'Delete User'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
