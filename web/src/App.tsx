import React, { useState, useEffect } from 'react';
import { StreamPlayer } from './components/StreamPlayer';
import { AdminUsers } from './components/AdminUsers';
import { AdminGroups } from './components/AdminGroups';
import { AdminTurnServers } from './components/AdminTurnServers';
import './App.css';

interface Host {
  id: string;
  name: string;
  status: 'Online' | 'Offline' | 'Busy';
  ip_address: string | null;
  server_codec_mode_support?: number;
  agent_connected?: boolean;
}

type HostStreamSettings = {
  resolution: string;
  fps: number;
  codec: string;
  bitrate: number;
  mouseQueueLimit: number;
  inputProtocol: string;
  useNativeClient: boolean;
  useCanvasRenderer: boolean;
  encoder: string;
  display: string;
  virtualDisplay: boolean;
};

const loadHostStreamSettings = (): HostStreamSettings => ({
  resolution: localStorage.getItem('lunaris_stream_res') || '1080p',
  fps: Number(localStorage.getItem('lunaris_stream_fps') || '60'),
  codec: localStorage.getItem('lunaris_stream_codec') || 'h264',
  bitrate: Number(localStorage.getItem('lunaris_stream_bitrate') || '8000'),
  mouseQueueLimit: Number(localStorage.getItem('lunaris_mouse_queue_limit') || '256'),
  inputProtocol: localStorage.getItem('lunaris_input_protocol') || 'webrtc',
  useNativeClient: localStorage.getItem('lunaris_tauri_use_native') === 'true',
  useCanvasRenderer: localStorage.getItem('lunaris_canvas_renderer') !== 'false',
  encoder: localStorage.getItem('lunaris_stream_encoder') || 'auto',
  display: localStorage.getItem('lunaris_stream_display') || 'default',
  virtualDisplay: localStorage.getItem('lunaris_stream_virtual_display') === 'true'
});

const saveHostStreamSettings = (settings: HostStreamSettings) => {
  localStorage.setItem('lunaris_stream_res', settings.resolution);
  localStorage.setItem('lunaris_stream_fps', String(settings.fps));
  localStorage.setItem('lunaris_stream_bitrate', String(settings.bitrate));
  localStorage.setItem('lunaris_stream_codec', settings.codec);
  localStorage.setItem('lunaris_mouse_queue_limit', String(settings.mouseQueueLimit));
  localStorage.setItem('lunaris_input_protocol', settings.inputProtocol);
  localStorage.setItem('lunaris_tauri_use_native', String(settings.useNativeClient));
  localStorage.setItem('lunaris_canvas_renderer', String(settings.useCanvasRenderer));
  localStorage.setItem('lunaris_stream_encoder', settings.encoder);
  localStorage.setItem('lunaris_stream_display', settings.display);
  localStorage.setItem('lunaris_stream_virtual_display', String(settings.virtualDisplay));
};

const getCurrentBrowserServerUrl = () => {
  if (window.location.hostname === 'tauri.localhost' || window.location.protocol.startsWith('tauri')) {
    return 'http://localhost:8080';
  }

  if (window.location.origin && window.location.origin !== 'null') {
    return window.location.origin;
  }

  const protocol = window.location.protocol === 'https:' ? 'https:' : 'http:';
  return `${protocol}//${window.location.host || 'localhost:8080'}`;
};

const getBackendHost = () => {
  const savedHost = localStorage.getItem('lunaris_server_host');
  if (savedHost) {
    return savedHost.replace(/^(https?:\/\/)?/, '').replace(/\/$/, '');
  }
  if (window.location.port === '5173' || window.location.port === '3000') {
    return `${window.location.hostname}:8080`;
  }
  if (window.location.hostname === 'tauri.localhost' || window.location.protocol.startsWith('tauri')) {
    return 'localhost:8080';
  }
  return window.location.host;
};

const getBackendProtocol = () => {
  const savedHost = localStorage.getItem('lunaris_server_host') || '';
  if (savedHost.startsWith('https://')) {
    return { http: 'https:', ws: 'wss:' };
  }
  if (savedHost.startsWith('http://')) {
    return { http: 'http:', ws: 'ws:' };
  }
  return {
    http: window.location.protocol === 'https:' ? 'https:' : 'http:',
    ws: window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  };
};

function App() {
  const [token, setToken] = useState<string | null>(localStorage.getItem('lunaris_token'));
  const [username, setUsername] = useState<string | null>(localStorage.getItem('lunaris_username'));

  // Check for deep link query parameters or local storage auto-launch flags on load
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    let qToken = params.get('token') || localStorage.getItem('lunaris_token');
    let qHostId = params.get('host_id') || localStorage.getItem('lunaris_auto_launch_host_id');
    let qHostName = params.get('host_name') || localStorage.getItem('lunaris_auto_launch_host_name') || 'Remote Host';
    let qCodecSupport = params.get('codec_support') || localStorage.getItem('lunaris_auto_launch_codec_support');

    // Clear temporary auto-launch keys so they don't trigger again on reload
    localStorage.removeItem('lunaris_auto_launch_host_id');
    localStorage.removeItem('lunaris_auto_launch_host_name');
    localStorage.removeItem('lunaris_auto_launch_codec_support');

    const qRes = params.get('res') || localStorage.getItem('lunaris_stream_res');
    const qFps = params.get('fps') || localStorage.getItem('lunaris_stream_fps');
    const qBitrate = params.get('bitrate') || localStorage.getItem('lunaris_stream_bitrate');
    const qCodec = params.get('codec') || localStorage.getItem('lunaris_stream_codec');

    if (qToken && qHostId) {
      localStorage.setItem('lunaris_token', qToken);
      setToken(qToken);

      if (qRes) localStorage.setItem('lunaris_stream_res', qRes);
      if (qFps) localStorage.setItem('lunaris_stream_fps', qFps);
      if (qBitrate) localStorage.setItem('lunaris_stream_bitrate', qBitrate);
      if (qCodec) localStorage.setItem('lunaris_stream_codec', qCodec);

      let codecSupportNum: number | undefined = undefined;
      if (qCodecSupport) {
        const parsed = parseInt(qCodecSupport, 10);
        if (!isNaN(parsed)) {
          codecSupportNum = parsed;
        }
      }

      setSelectedHost({
        id: qHostId,
        name: qHostName,
        status: 'Online',
        ip_address: null,
        server_codec_mode_support: codecSupportNum
      });
    }
  }, []);

  // Auth Form State
  const [authUsername, setAuthUsername] = useState<string>('');
  const [authPassword, setAuthPassword] = useState<string>('');
  const [authServerHost, setAuthServerHost] = useState<string>(() => getCurrentBrowserServerUrl());
  const [authError, setAuthError] = useState<string | null>(null);
  const [authLoading, setAuthLoading] = useState<boolean>(false);

  // Admin State
  const [userRole, setUserRole] = useState<string | null>(localStorage.getItem('lunaris_role'));
  const [activeTab, setActiveTab] = useState<'devices' | 'users' | 'groups' | 'turn_servers'>('devices');

  // Dashboard State
  const [hosts, setHosts] = useState<Host[]>([]);
  const [hostsLoading, setHostsLoading] = useState<boolean>(false);
  const [hostsError, setHostsError] = useState<string | null>(null);
  const [selectedHost, setSelectedHost] = useState<Host | null>(null);
  const [agentToken, setAgentToken] = useState<string | null>(null);


  // Application view navigation states
  const [viewingHost, setViewingHost] = useState<Host | null>(null);
  const [viewingApps, setViewingApps] = useState<{ id: number; title: string; icon_base64?: string | null }[] | null>(null);
  const [viewingAppsLoading, setViewingAppsLoading] = useState<boolean>(false);
  const [viewingAppsError, setViewingAppsError] = useState<string | null>(null);
  const [selectedAppId, setSelectedAppId] = useState<number | null>(null);
  const [settingsHost, setSettingsHost] = useState<Host | null>(null);
  const [draftHostSettings, setDraftHostSettings] = useState<HostStreamSettings>(() => loadHostStreamSettings());
  const [deleteHostLoading, setDeleteHostLoading] = useState<string | null>(null);
  const [pendingDeleteHost, setPendingDeleteHost] = useState<Host | null>(null);
  const [hostAvailableDisplays, setHostAvailableDisplays] = useState<{id: string; name: string; width: number; height: number; refresh_rate: number; is_primary: boolean}[]>([]);



  // Fetch hosts list
  const fetchHosts = async () => {
    if (!token) return;
    setHostsLoading(true);
    setHostsError(null);
    try {
      const serverHost = getBackendHost();
      const response = await fetch(`${getBackendProtocol().http}//${serverHost}/api/hosts`, {
        headers: {
          'Authorization': `Bearer ${token}`
        }
      });
      if (response.ok) {
        const data = await response.json();
        setHosts(data);
      } else if (response.status === 401) {
        // Token expired/invalid
        handleLogout();
      } else {
        setHostsError("Failed to fetch host list");
      }
    } catch (err) {
      setHostsError("Failed to connect to signaling server");
    } finally {
      setHostsLoading(false);
    }
  };





  useEffect(() => {
    if (token) {
      fetchHosts();
      // Periodically refresh host list
      const interval = setInterval(fetchHosts, 5000);
      return () => clearInterval(interval);
    }
  }, [token]);

  // Fetch user role on mount
  useEffect(() => {
    if (token) {
      const fetchMe = async () => {
        try {
          const serverHost = getBackendHost();
          const response = await fetch(`${getBackendProtocol().http}//${serverHost}/api/auth/me`, {
            headers: { 'Authorization': `Bearer ${token}` }
          });
          if (response.ok) {
            const data = await response.json();
            setUserRole(data.role);
            localStorage.setItem('lunaris_role', data.role);
          }
        } catch (err) {
          console.error('Failed to fetch user info:', err);
        }
      };
      fetchMe();
    }
  }, [token]);

  // Fetch Agent Connection Token
  const fetchAgentToken = async () => {
    if (!token) return;
    try {
      const serverHost = getBackendHost();
      const response = await fetch(`${getBackendProtocol().http}//${serverHost}/api/agent/token`, {
        headers: {
          'Authorization': `Bearer ${token}`
        }
      });
      if (response.ok) {
        const data = await response.json();
        setAgentToken(data.token);
      }
    } catch (err) {
      console.error("Failed to fetch agent token:", err);
    }
  };

  useEffect(() => {
    if (token) {
      fetchAgentToken();
    }
  }, [token]);

  useEffect(() => {
    if (!viewingHost || !token) {
      setViewingApps(null);
      setViewingAppsError(null);
      return;
    }

    setViewingAppsLoading(true);
    setViewingAppsError(null);

    const protocol = getBackendProtocol().ws;
    const serverHost = getBackendHost();
    const wsUrl = `${protocol}//${serverHost}/ws/client?token=${encodeURIComponent(token)}`;
    const ws = new WebSocket(wsUrl);

    let isMounted = true;
    let timeoutId: ReturnType<typeof setTimeout> | null = null;

    ws.onopen = () => {
      if (!isMounted) return;
      ws.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "GetAppList",
          payload: { target_id: viewingHost.id }
        }
      }));
      // Timeout: close after 10s if no response
      timeoutId = setTimeout(() => {
        if (isMounted) {
          console.warn('[AppList] Timeout - no response received');
          setViewingAppsError("Timeout: agent did not respond");
          setViewingAppsLoading(false);
          ws.close();
        }
      }, 10000);
    };

    ws.onmessage = (event) => {
      if (!isMounted) return;
      try {
        const msg = JSON.parse(event.data);
        if (msg.event === "Signaling" && msg.data) {
          const type = msg.data.type;
          const payload = msg.data.payload;

          if (type === "AppListResponse") {
            if (timeoutId) clearTimeout(timeoutId);
            setViewingApps(payload.apps);
            setViewingAppsLoading(false);
            ws.close();
          } else if (type === "Error") {
            if (timeoutId) clearTimeout(timeoutId);
            setViewingAppsError(payload.message || "Failed to load applications");
            setViewingAppsLoading(false);
            ws.close();
          }
        }
      } catch (e) {
        console.error("Error parsing WebSocket message for app list:", e);
      }
    };

    ws.onerror = () => {
      if (!isMounted) return;
      if (timeoutId) clearTimeout(timeoutId);
      setViewingAppsError("Connection error while fetching applications");
      setViewingAppsLoading(false);
    };

    ws.onclose = () => {
      if (!isMounted) return;
      if (timeoutId) clearTimeout(timeoutId);
      setViewingAppsLoading(false);
    };

    return () => {
      isMounted = false;
      if (timeoutId) clearTimeout(timeoutId);
      if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
        ws.close();
      }
    };
  }, [viewingHost, token]);

  const handleAuth = async (e: React.FormEvent) => {
    e.preventDefault();
    setAuthError(null);

    if (!authUsername.trim()) {
      setAuthError("Username is required");
      return;
    }
    if (authPassword.length < 6) {
      setAuthError("Password must be at least 6 characters long");
      return;
    }

    setAuthLoading(true);
    const endpoint = '/api/auth/login';

    // Save to localStorage so getBackendHost/getBackendProtocol resolves correctly
    localStorage.setItem('lunaris_server_host', authServerHost);

    const serverHost = getBackendHost();
    const protocol = getBackendProtocol().http;
    const serverUrl = `${protocol}//${serverHost}${endpoint}`;

    try {
      const response = await fetch(serverUrl, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({
          username: authUsername,
          password: authPassword
        })
      });

      const data = await response.json();

      if (response.ok) {
        // Success
        localStorage.setItem('lunaris_token', data.token);
        localStorage.setItem('lunaris_username', data.username);
        localStorage.setItem('lunaris_server_host', authServerHost);
        if (data.role) {
          localStorage.setItem('lunaris_role', data.role);
          setUserRole(data.role);
        }
        setToken(data.token);
        setUsername(data.username);
        // Clear fields
        setAuthUsername('');
        setAuthPassword('');
      } else {
        setAuthError(data.error || "Authentication failed");
      }
    } catch (err) {
      setAuthError("Connection to authentication server failed");
    } finally {
      setAuthLoading(false);
    }
  };

  const handleLogout = () => {
    localStorage.removeItem('lunaris_token');
    localStorage.removeItem('lunaris_username');
    localStorage.removeItem('lunaris_role');
    setToken(null);
    setUsername(null);
    setUserRole(null);
    setActiveTab('devices');
    setHosts([]);
    setSelectedHost(null);
  };



  const openHostSettings = (host: Host) => {
    setSettingsHost(host);
    setDraftHostSettings(loadHostStreamSettings());
  };

  const applyHostSettings = () => {
    saveHostStreamSettings(draftHostSettings);
    setSettingsHost(null);
  };

  // Fetch display list from agent when host settings modal opens
  useEffect(() => {
    if (!settingsHost || !token) return;
    setHostAvailableDisplays([]); // reset on new host

    const protocol = getBackendProtocol().ws;
    const serverHost = getBackendHost();
    const wsUrl = `${protocol}//${serverHost}/ws/client?token=${encodeURIComponent(token)}`;
    console.log(`[Capabilities] Connecting to ${wsUrl}`);

    let gotResponse = false;
    let timeoutId: ReturnType<typeof setTimeout> | null = null;
    let isMounted = true;
    const ws = new WebSocket(wsUrl);
    ws.onopen = () => {
      if (!isMounted) return;
      console.log('[Capabilities] WebSocket opened, sending GetCapabilities');
      ws.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "GetCapabilities",
          payload: { target_id: settingsHost.id }
        }
      }));
      // Timeout: close after 5s if no response
      timeoutId = setTimeout(() => {
        if (!gotResponse && isMounted) {
          console.log('[Capabilities] Timeout - no response received');
          ws.close();
        }
      }, 5000);
    };
    ws.onmessage = (event) => {
      if (!isMounted) return;
      try {
        const msg = JSON.parse(event.data);
        // Skip non-signaling messages (SDP, ICE, etc from other sessions)
        if (msg.event !== 'Signaling' || msg.data?.type !== 'CapabilitiesResponse') return;
        console.log('[Capabilities] Got CapabilitiesResponse!');
        const payload = msg.data.payload;
        if (payload?.displays) {
          console.log('[Capabilities] Displays:', JSON.stringify(payload.displays));
          setHostAvailableDisplays(payload.displays);
        }
        gotResponse = true;
        if (timeoutId) clearTimeout(timeoutId);
        ws.close();
      } catch (e) {
        console.error('[Capabilities] Parse error:', e);
      }
    };
    ws.onerror = (e) => {
      if (!isMounted) return;
      console.error('[Capabilities] WebSocket error:', e);
      if (timeoutId) clearTimeout(timeoutId);
    };
    ws.onclose = (e) => {
      if (!isMounted) return;
      console.log(`[Capabilities] WebSocket closed: code=${e.code} reason=${e.reason} wasClean=${e.wasClean}`);
      if (!gotResponse && timeoutId) {
        // Unexpected close before response - don't log timeout
        clearTimeout(timeoutId);
      }
    };

    return () => {
      isMounted = false;
      if (timeoutId) clearTimeout(timeoutId);
      if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
        ws.close();
      }
    };
  }, [settingsHost, token]);

  const handleDeleteHost = async (host: Host) => {
    if (!token || deleteHostLoading) return;

    setDeleteHostLoading(host.id);
    try {
      const serverHost = getBackendHost();
      const response = await fetch(`${getBackendProtocol().http}//${serverHost}/api/hosts/${encodeURIComponent(host.id)}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${token}` }
      });

      if (response.ok) {
        setPendingDeleteHost(null);
        setHosts((current) => current.filter((h) => h.id !== host.id));
        if (viewingHost?.id === host.id) {
          setViewingHost(null);
          setViewingApps(null);
        }
        if (selectedHost?.id === host.id) {
          setSelectedHost(null);
        }
      } else if (response.status === 401) {
        handleLogout();
      } else {
        setHostsError('Failed to delete host');
      }
    } catch (err) {
      setHostsError('Failed to connect to signaling server');
    } finally {
      setDeleteHostLoading(null);
    }
  };

  const updateDraftHostSettings = <K extends keyof HostStreamSettings>(key: K, value: HostStreamSettings[K]) => {
    setDraftHostSettings((current) => ({ ...current, [key]: value }));
  };

  const handleStopStream = (hostId: string) => {
    if (!token) return;
    const protocol = getBackendProtocol().ws;
    const serverHost = getBackendHost();
    const wsUrl = `${protocol}//${serverHost}/ws/client?token=${encodeURIComponent(token)}`;
    const ws = new WebSocket(wsUrl);
    ws.onopen = () => {
      ws.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "StopActiveStream",
          payload: { target_id: hostId }
        }
      }));
    };
    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.event === "Signaling" && msg.data) {
          if (msg.data.type === "StopActiveStreamResponse") {
            ws.close();
            fetchHosts();
          }
        }
      } catch (e) {
        console.error("Error stopping stream:", e);
      }
    };
    // Auto close after 3 seconds in case of no response
    setTimeout(() => {
      if (ws.readyState === WebSocket.OPEN) ws.close();
    }, 3000);
  };

  const currentViewingHost = viewingHost ? (hosts.find(h => h.id === viewingHost.id) || viewingHost) : null;

  // If inside streaming session, render full screen stream viewer
  if (token && selectedHost) {
    return (
      <StreamPlayer
        hostId={selectedHost.id}
        hostName={selectedHost.name}
        token={token}
        serverCodecModeSupport={selectedHost.server_codec_mode_support}
        appId={selectedAppId}
        onBack={() => {
          setSelectedHost(null);
          setSelectedAppId(null);
          fetchHosts();
        }}
      />
    );
  }

  return (
    <div className="app-layout">
      {/* Background decoration elements */}
      <div className="glow-orb bg-glow-blue"></div>
      <div className="glow-orb bg-glow-purple"></div>

      {/* Header bar */}
      <header className="navbar">
        <div className="nav-brand">
          <div className="brand-logo">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor">
              <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
            </svg>
          </div>
          <span className="brand-name">Lunaris</span>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: '1rem' }}>
          {token && username && (
            <div className="nav-user-panel">
              <div className="user-info" style={{ alignItems: 'flex-end' }}>
                <span className="user-label">SERVER:</span>
                <span className="username" style={{ fontSize: '0.85rem', color: 'var(--accent-cyan)' }}>
                  {localStorage.getItem('lunaris_server_host') || getCurrentBrowserServerUrl()}
                </span>
              </div>
              <div className="user-info">
                <span className="user-label">USER:</span>
                <span className="username">{username}</span>
              </div>
              <button onClick={handleLogout} className="btn-logout" title="Log Out">
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                  <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9" />
                </svg>
              </button>
            </div>
          )}
        </div>
      </header>

      {/* Main Content Area */}
      <main className="main-viewport">
        {!token ? (
          /* Authentication Screen */
          <div className="auth-card-wrapper">
            <div className="auth-card">
              <div className="auth-header">
                <h2>Welcome Back</h2>
                <p>Access your remote desktop network</p>
              </div>

              {authError && (
                <div className="auth-error-banner">
                  <span className="error-icon">⚠️</span>
                  <span>{authError}</span>
                </div>
              )}

              <form onSubmit={handleAuth} className="auth-form">
                <div className="form-group">
                  <label htmlFor="serverHost">Server URL</label>
                  <input
                    type="text"
                    id="serverHost"
                    value={authServerHost}
                    onChange={(e) => setAuthServerHost(e.target.value)}
                    placeholder="e.g. http://localhost:8080"
                    required
                  />
                </div>

                <div className="form-group">
                  <label htmlFor="username">Username</label>
                  <input
                    type="text"
                    id="username"
                    value={authUsername}
                    onChange={(e) => setAuthUsername(e.target.value)}
                    placeholder="Enter username"
                    required
                    autoComplete="username"
                  />
                </div>

                <div className="form-group">
                  <label htmlFor="password">Password</label>
                  <input
                    type="password"
                    id="password"
                    value={authPassword}
                    onChange={(e) => setAuthPassword(e.target.value)}
                    placeholder="Enter password (min 6 chars)"
                    required
                    autoComplete="current-password"
                  />
                </div>

                <button type="submit" disabled={authLoading} className="btn-primary auth-submit-btn">
                  {authLoading ? (
                    <div className="inline-loader"></div>
                  ) : (
                    'Sign In'
                  )}
                </button>
              </form>
            </div>
          </div>
        ) : (
          /* Main Dashboard - Full Width */
          <div className="dashboard-full-width">
            <div className="dashboard-main full-width">
              {/* Navigation Tabs */}
              {userRole === 'admin' && !viewingHost && (
                <div className="admin-nav-tabs">
                  <button
                    className={`nav-tab ${activeTab === 'devices' ? 'active' : ''}`}
                    onClick={() => setActiveTab('devices')}
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
                      <line x1="8" y1="21" x2="16" y2="21" />
                      <line x1="12" y1="17" x2="12" y2="21" />
                    </svg>
                    Devices
                  </button>
                  <button
                    className={`nav-tab ${activeTab === 'users' ? 'active' : ''}`}
                    onClick={() => setActiveTab('users')}
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
                      <circle cx="12" cy="7" r="4" />
                    </svg>
                    Users
                  </button>
                  <button
                    className={`nav-tab ${activeTab === 'groups' ? 'active' : ''}`}
                    onClick={() => setActiveTab('groups')}
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                      <circle cx="9" cy="7" r="4" />
                      <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                      <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                    </svg>
                    Groups
                  </button>
                  <button
                    className={`nav-tab ${activeTab === 'turn_servers' ? 'active' : ''}`}
                    onClick={() => setActiveTab('turn_servers')}
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <polygon points="12 2 2 7 12 12 22 7 12 2" />
                      <polyline points="2 17 12 22 22 17" />
                      <polyline points="2 12 12 17 22 12" />
                    </svg>
                    TURN Servers
                  </button>
                </div>
              )}

              {activeTab === 'devices' && (
                <>
                  {viewingHost ? (
                    <div className="apps-directory-view">
                      <div className="apps-navigation">
                        <button
                          onClick={() => setViewingHost(null)}
                          className="btn-back-nav"
                          title="Back to Device Directory"
                        >
                          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" style={{ marginRight: '6px' }}>
                            <line x1="19" y1="12" x2="5" y2="12"></line>
                            <polyline points="12 19 5 12 12 5"></polyline>
                          </svg>
                          Back to Directory
                        </button>
                        <div className="breadcrumbs">
                          <span className="breadcrumb-item">Device Directory</span>
                          <span className="breadcrumb-separator">/</span>
                          <span className="breadcrumb-item active">{viewingHost.name}</span>
                        </div>
                      </div>

                      <div className="section-header">
                        <div>
                          <h1 className="section-title">Applications Directory</h1>
                          <p className="section-subtitle">Select and stream apps from <strong>{viewingHost.name}</strong></p>
                        </div>
                      </div>

                      {viewingAppsLoading ? (
                        <div className="loading-card">
                          <div className="tech-loader"></div>
                          <div>Scanning applications on {viewingHost.name}...</div>
                        </div>
                      ) : viewingAppsError ? (
                        <div className="error-card">
                          <div className="error-title">⚠️ Connection Error</div>
                          <div className="error-desc">{viewingAppsError}</div>
                          <button
                            onClick={() => {
                              const h = viewingHost;
                              setViewingHost(null);
                              setTimeout(() => setViewingHost(h), 50);
                            }}
                            className="btn-secondary"
                          >
                            Retry Scan
                          </button>
                        </div>
                      ) : viewingApps && viewingApps.length === 0 ? (
                        <div className="empty-card">
                          <div className="empty-icon">🎮</div>
                          <h3>No Applications Found</h3>
                          <p>There are no applications configured on {viewingHost.name}.</p>
                          <p className="empty-hint">Please configure applications on the host first.</p>
                        </div>
                      ) : viewingApps ? (
                        <div className="apps-card-grid">
                          {viewingApps.map((app) => (
                            <div key={app.id} className="app-portrait-card">
                              <div className="app-card-glow"></div>

                              {/* Box Art Cover */}
                              <div className="app-cover-wrapper">
                                {app.icon_base64 ? (
                                  <img
                                    src={`data:image/png;base64,${app.icon_base64}`}
                                    alt={app.title}
                                    className="app-cover-image"
                                  />
                                ) : (
                                  <div className="app-cover-placeholder">
                                    <div className="fallback-card-gradient">
                                      <div className="fallback-icon">
                                        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                                          <rect x="2" y="6" width="20" height="12" rx="3" />
                                          <line x1="6" y1="12" x2="10" y2="12" />
                                          <line x1="8" y1="10" x2="8" y2="14" />
                                          <line x1="15" y1="13" x2="15.01" y2="13" />
                                          <line x1="18" y1="11" x2="18.01" y2="11" />
                                        </svg>
                                      </div>
                                      <span className="fallback-title">{app.title}</span>
                                    </div>
                                  </div>
                                )}
                              </div>

                              {/* Hover Overlay */}
                              <div className="app-hover-overlay">
                                <div className="overlay-content">
                                  {/* Play Button - Launch Stream in browser */}
                                  <button
                                    onClick={() => {
                                      setSelectedAppId(app.id);
                                      setSelectedHost(currentViewingHost || viewingHost);
                                    }}
                                    className="overlay-btn btn-play"
                                    title="Launch Web Stream (In Browser)"
                                  >
                                    <svg width="24" height="24" viewBox="0 0 24 24" fill="currentColor">
                                      <polygon points="6 4 20 12 6 20 6 4" />
                                    </svg>
                                  </button>

                                  {/* Title overlay */}
                                  <div className="overlay-app-title" title={app.title}>
                                    {app.title}
                                  </div>

                                  {/* Action Row */}
                                  <div className="overlay-bottom-actions">
                                    {/* Launch Native App */}
                                    <button
                                      onClick={() => {
                                        const protocol = getBackendProtocol().http;
                                        const serverHost = getBackendHost();
                                        const serverUrl = `${protocol}//${serverHost}`;
                                        const rawRes = localStorage.getItem('lunaris_stream_res') || '1080p';
                                        const resStr = rawRes === '720p' ? '1280x720' : rawRes === '540p' ? '960x540' : (rawRes.includes('x') ? rawRes : '1920x1080');
                                        const fps = localStorage.getItem('lunaris_stream_fps') || '60';
                                        const bitrate = localStorage.getItem('lunaris_stream_bitrate') || '8000';
                                        const codec = localStorage.getItem('lunaris_stream_codec') || 'auto';
                                        const mouseQueueLimit = localStorage.getItem('lunaris_mouse_queue_limit') || '256';
                                        const encoder = localStorage.getItem('lunaris_stream_encoder') || 'auto';
                                        const display = localStorage.getItem('lunaris_stream_display') || 'default';
                                        const virtualDisplay = localStorage.getItem('lunaris_stream_virtual_display') === 'true';
                                        const inputProtocol = localStorage.getItem('lunaris_input_protocol') || 'webrtc';
                                        const hostToUse = currentViewingHost || viewingHost;

                                        if (!hostToUse) return;

                                        const tauri = (window as any).__TAURI__;
                                        if (tauri) {
                                          tauri.core.invoke('launch_native_client', {
                                            hostId: hostToUse.id,
                                            serverUrl,
                                            token,
                                            res: resStr,
                                            fps: String(fps),
                                            bitrate: String(bitrate),
                                            codec,
                                            appId: app.id,
                                            mouseQueueLimit: String(mouseQueueLimit),
                                            hostName: hostToUse.name,
                                            encoder,
                                            displayId: display,
                                            virtualDisplay,
                                            inputProtocol
                                          }).catch((err: any) => {
                                            console.error("Failed to launch native client:", err);
                                            alert("Failed to launch native client: " + err);
                                          });
                                        } else {
                                          const wsProtocol = getBackendProtocol().ws;
                                          const wsServerUrl = `${wsProtocol}//${serverHost}`;
                                          window.location.href = `lunaris://connect?host_id=${hostToUse.id}&server=${wsServerUrl}&token=${token}&res=${resStr}&fps=${fps}&bitrate=${bitrate}&codec=${codec}&mouse_queue_limit=${mouseQueueLimit}&host_name=${encodeURIComponent(hostToUse.name)}&app_id=${app.id}&encoder=${encodeURIComponent(encoder)}&display=${encodeURIComponent(display)}&virtual_display=${virtualDisplay}&input_protocol=${encodeURIComponent(inputProtocol)}`;
                                        }
                                      }}
                                      className="overlay-btn btn-launch-app"
                                      title="Launch in Native Client"
                                    >
                                      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                                        <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
                                        <polyline points="15 3 21 3 21 9" />
                                        <line x1="10" y1="14" x2="21" y2="3" />
                                      </svg>
                                    </button>

                                    {/* Stop active stream (if host is Busy) */}
                                    {currentViewingHost?.status === "Busy" && (
                                      <button
                                        onClick={() => handleStopStream(currentViewingHost.id)}
                                        className="overlay-btn btn-stop-stream"
                                        title="Terminate Active Stream Session"
                                      >
                                        <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
                                          <rect x="4" y="4" width="16" height="16" rx="2" />
                                        </svg>
                                      </button>
                                    )}
                                  </div>
                                </div>
                              </div>
                            </div>
                          ))}
                        </div>
                      ) : null}
                    </div>
                  ) : (
                    <>
                      <div className="section-header">
                        <div>
                          <h1 className="section-title">Device Directory</h1>
                          <p className="section-subtitle">Manage and connect to active remote agent streams</p>
                        </div>
                        <div style={{ display: 'flex', gap: '0.75rem' }}>
                          <button
                            onClick={fetchHosts}
                            disabled={hostsLoading}
                            className="btn-secondary refresh-btn"
                            title="Refresh device status"
                          >
                            <svg
                              width="16"
                              height="16"
                              viewBox="0 0 24 24"
                              fill="none"
                              stroke="currentColor"
                              strokeWidth="2"
                              className={hostsLoading ? 'spin' : ''}
                            >
                              <path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67" />
                            </svg>
                            Sync Devices
                          </button>
                        </div>
                      </div>

                      {/* Agent Setup & Connection Info Panel */}
                      <div className="agent-setup-panel" style={{
                        background: 'rgba(255, 255, 255, 0.02)',
                        border: '1px solid rgba(255, 255, 255, 0.08)',
                        borderRadius: '12px',
                        padding: '1.25rem',
                        marginBottom: '2rem',
                        display: 'flex',
                        flexDirection: 'column',
                        gap: '1rem',
                        position: 'relative',
                        overflow: 'hidden',
                        boxShadow: '0 8px 32px 0 rgba(0, 0, 0, 0.2)'
                      }}>
                        <div style={{
                          position: 'absolute',
                          top: 0,
                          left: 0,
                          width: '100%',
                          height: '2px',
                          background: 'linear-gradient(90deg, var(--accent-cyan, #00f0ff), var(--accent-purple, #ab3bf2))'
                        }}></div>
                        
                        <div>
                          <h3 style={{ margin: 0, fontSize: '1.1rem', fontWeight: 600, color: '#ffffff', display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
                            <span>🔑</span> Host Agent Setup Credentials
                          </h3>
                          <p style={{ margin: '0.25rem 0 0 0', fontSize: '0.85rem', color: 'var(--text-secondary, #94a3b8)' }}>
                            Use these credentials to register and configure new Host Agents to stream to this server.
                          </p>
                        </div>

                        <div style={{
                          display: 'grid',
                          gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
                          gap: '1rem',
                          marginTop: '0.25rem'
                        }}>
                          {/* Server URL */}
                          <div style={{
                            background: 'rgba(0, 0, 0, 0.2)',
                            padding: '0.75rem 1rem',
                            borderRadius: '8px',
                            border: '1px solid rgba(255, 255, 255, 0.04)',
                            display: 'flex',
                            flexDirection: 'column',
                            gap: '0.25rem'
                          }}>
                            <span style={{ fontSize: '0.7rem', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', color: 'var(--accent-cyan, #00f0ff)' }}>Signaling Server URL</span>
                            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '0.5rem' }}>
                              <code style={{ fontFamily: 'monospace', fontSize: '0.85rem', color: '#e2e8f0', wordBreak: 'break-all' }}>
                                {`ws://${getBackendHost()}`}
                              </code>
                              <button
                                onClick={() => {
                                  navigator.clipboard.writeText(`ws://${getBackendHost()}`);
                                  alert("Server URL copied!");
                                }}
                                style={{
                                  background: 'transparent',
                                  border: 'none',
                                  color: '#94a3b8',
                                  cursor: 'pointer',
                                  padding: '4px',
                                  fontSize: '1rem',
                                  display: 'flex',
                                  alignItems: 'center',
                                  justifyContent: 'center'
                                }}
                                title="Copy Server URL"
                              >
                                📋
                              </button>
                            </div>
                          </div>

                          {/* Connection Token */}
                          <div style={{
                            background: 'rgba(0, 0, 0, 0.2)',
                            padding: '0.75rem 1rem',
                            borderRadius: '8px',
                            border: '1px solid rgba(255, 255, 255, 0.04)',
                            display: 'flex',
                            flexDirection: 'column',
                            gap: '0.25rem'
                          }}>
                            <span style={{ fontSize: '0.7rem', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', color: 'var(--accent-purple, #ab3bf2)' }}>Agent Connection Token</span>
                            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '0.5rem' }}>
                              <code style={{ fontFamily: 'monospace', fontSize: '0.85rem', color: '#e2e8f0', wordBreak: 'break-all' }}>
                                {agentToken || "Loading..."}
                              </code>
                              <button
                                onClick={() => {
                                  if (agentToken) {
                                    navigator.clipboard.writeText(agentToken);
                                    alert("Connection Token copied!");
                                  }
                                }}
                                disabled={!agentToken}
                                style={{
                                  background: 'transparent',
                                  border: 'none',
                                  color: '#94a3b8',
                                  cursor: 'pointer',
                                  padding: '4px',
                                  fontSize: '1rem',
                                  display: 'flex',
                                  alignItems: 'center',
                                  justifyContent: 'center',
                                  opacity: agentToken ? 1 : 0.5
                                }}
                                title="Copy Connection Token"
                              >
                                📋
                              </button>
                            </div>
                          </div>
                        </div>
                      </div>

                      {hostsError && (
                        <div className="error-card">
                          <div className="error-title">⚠️ Sync Error</div>
                          <div className="error-desc">{hostsError}</div>
                          <button onClick={fetchHosts} className="btn-secondary">Retry Sync</button>
                        </div>
                      )}

                      {hostsLoading && hosts.length === 0 ? (
                        <div className="loading-card">
                          <div className="tech-loader"></div>
                          <div>Syncing active host agent list...</div>
                        </div>
                      ) : hosts.length === 0 ? (
                        <div className="empty-card">
                          <div className="empty-icon">🖥️</div>
                          <h3>No agents connected</h3>
                          <p>There are no host agents registered with the server.</p>
                          <p className="empty-hint">Agents will appear here automatically once they connect.</p>
                        </div>
                      ) : (
                        <div className="hosts-card-grid">
                          {hosts.map((host) => {
                            const isOnline = host.status === 'Online';

                            return (
                              <div
                                key={host.id}
                                className={`host-card ${host.status.toLowerCase()} ${isOnline ? 'clickable-host-card' : ''}`}
                                onClick={() => {
                                  if (isOnline) {
                                    setViewingHost(host);
                                  }
                                }}
                              >
                                <div className="host-card-glow"></div>
                                <div className="host-card-header">
                                  <div>
                                    <h3 className="host-name">{host.name}</h3>
                                    <span className="host-id">ID: {host.id.slice(0, 8)}...</span>
                                  </div>
                                </div>

                                <div className="host-card-body">
                                  <div className="host-meta-item">
                                    <span className="meta-label">IP Address</span>
                                    <span className="meta-value">{host.ip_address || 'Signaling Tunnel'}</span>
                                  </div>
                                  <div className="host-meta-item">
                                    <span className="meta-label">Status</span>
                                    <div className="host-status-badge">
                                      <span className={`status-indicator ${host.status.toLowerCase()}`}></span>
                                      <span className="status-label">{host.status}</span>
                                    </div>
                                  </div>
                                </div>

                                <div className="host-card-actions" onClick={(e) => e.stopPropagation()}>
                                  <button
                                    type="button"
                                    className="host-action-btn"
                                    title="Stream settings"
                                    onClick={() => openHostSettings(host)}
                                  >
                                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                      <circle cx="12" cy="12" r="3" />
                                      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06A1.65 1.65 0 0 0 15 19.4a1.65 1.65 0 0 0-1 .6 1.65 1.65 0 0 0-.4 1.08V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 8.6 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-.6-1 1.65 1.65 0 0 0-1.08-.4H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 8.6a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-.6 1.65 1.65 0 0 0 .4-1.08V3a2 2 0 1 1 4 0v.09A1.65 1.65 0 0 0 15.4 4.6a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9c.38.16.72.4 1 .7.29.29.6.9.6 1.3V12a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
                                    </svg>
                                  </button>
                                  <button
                                    type="button"
                                    className="host-action-btn danger"
                                    title="Delete host"
                                    disabled={deleteHostLoading === host.id}
                                    onClick={() => setPendingDeleteHost(host)}
                                  >
                                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                      <polyline points="3 6 5 6 21 6" />
                                      <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
                                      <path d="M10 11v6" />
                                      <path d="M14 11v6" />
                                      <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
                                    </svg>
                                  </button>
                                </div>

                              </div>
                            );
                          })}
                        </div>
                      )}
                    </>
                  )}
                </>
              )}
              {activeTab === 'users' && (
                <AdminUsers token={token!} getBackendHost={getBackendHost} getBackendProtocol={getBackendProtocol} />
              )}
              {activeTab === 'groups' && (
                <AdminGroups token={token!} getBackendHost={getBackendHost} getBackendProtocol={getBackendProtocol} />
              )}
              {activeTab === 'turn_servers' && (
                <AdminTurnServers token={token!} getBackendHost={getBackendHost} getBackendProtocol={getBackendProtocol} />
              )}
            </div>
          </div>
        )}
      {pendingDeleteHost && (
        <div className="host-confirm-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !deleteHostLoading) setPendingDeleteHost(null); }}>
          <div className="host-confirm-card" role="dialog" aria-modal="true" aria-labelledby="deleteHostTitle">
            <div className="host-confirm-icon danger">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <polyline points="3 6 5 6 21 6" />
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
                <path d="M10 11v6" />
                <path d="M14 11v6" />
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
              </svg>
            </div>
            <div className="host-confirm-content">
              <h2 id="deleteHostTitle">Delete Host</h2>
              <p>Delete <strong>{pendingDeleteHost.name}</strong> from this server? Active sessions for this host will be stopped.</p>
            </div>
            <div className="host-confirm-actions">
              <button className="btn-secondary" disabled={!!deleteHostLoading} onClick={() => setPendingDeleteHost(null)}>Cancel</button>
              <button className="btn-danger" disabled={!!deleteHostLoading} onClick={() => handleDeleteHost(pendingDeleteHost)}>
                {deleteHostLoading === pendingDeleteHost.id ? 'Deleting...' : 'Delete Host'}
              </button>
            </div>
          </div>
        </div>
      )}

      {settingsHost && (
        <div className="stream-settings-overlay" style={{ zIndex: 1200 }} onMouseDown={(e) => { if (e.target === e.currentTarget) setSettingsHost(null); }}>
          <div className="stream-settings-card">
            <h2>Stream Settings</h2>
            <p className="subtitle">Defaults used when connecting to {settingsHost.name}</p>
            <div className="settings-grid">
              <div className="settings-group">
                <label htmlFor="hostResolution">Resolution</label>
                <select id="hostResolution" value={draftHostSettings.resolution} onChange={(e) => updateDraftHostSettings('resolution', e.target.value)}>
                  <option value="1080p">1080p (1920x1080)</option>
                  <option value="720p">720p (1280x720)</option>
                  <option value="540p">540p (960x540)</option>
                </select>
              </div>
              <div className="settings-group">
                <label htmlFor="hostFps">Frame Rate</label>
                <select id="hostFps" value={draftHostSettings.fps} onChange={(e) => updateDraftHostSettings('fps', Number(e.target.value))}>
                  <option value={240}>240 FPS</option>
                  <option value={144}>144 FPS</option>
                  <option value={120}>120 FPS</option>
                  <option value={90}>90 FPS</option>
                  <option value={60}>60 FPS</option>
                  <option value={30}>30 FPS</option>
                </select>
              </div>
              <div className="settings-group">
                <label htmlFor="hostCodec">Video Codec</label>
                <select id="hostCodec" value={draftHostSettings.codec} onChange={(e) => updateDraftHostSettings('codec', e.target.value)}>
                  <option value="h264">H.264</option>
                  <option value="h265">H.265 (HEVC)</option>
                  <option value="av1">AV1</option>
                  <option value="auto">Auto</option>
                </select>
              </div>
              <div className="settings-group">
                <label htmlFor="hostEncoder">Encoder Backend</label>
                <select id="hostEncoder" value={draftHostSettings.encoder} onChange={(e) => updateDraftHostSettings('encoder', e.target.value)}>
                  <option value="auto">Auto (Recommended)</option>
                  <option value="native">Native GPU</option>
                  <option value="ffmpeg">FFmpeg GPU</option>
                  <option value="nvenc">NVENC</option>
                  <option value="amf">AMF</option>
                  <option value="qsv">QSV</option>
                  <option value="vaapi">VAAPI</option>
                  <option value="software">Software</option>
                </select>
              </div>
              <div className="settings-group full-width">
                <label htmlFor="hostDisplay">Display</label>
                <select id="hostDisplay" value={draftHostSettings.display} onChange={(e) => updateDraftHostSettings('display', e.target.value)}>
                  <option value="default">Default</option>
                  {hostAvailableDisplays.map(d => (
                    <option key={d.id} value={d.id}>
                      {d.name} ({d.width}x{d.height} @ {d.refresh_rate.toFixed(0)}Hz){d.is_primary ? ' ★' : ''}
                    </option>
                  ))}
                </select>
              </div>
              <div className="settings-group full-width">
                <label htmlFor="hostBitrate">Bitrate (Kbps)</label>
                <div style={{ display: 'flex', alignItems: 'center', gap: '1rem' }}>
                  <input id="hostBitrate" type="range" min={1000} max={150000} step={500} value={draftHostSettings.bitrate} onChange={(e) => updateDraftHostSettings('bitrate', Number(e.target.value))} style={{ flex: 1 }} />
                  <span style={{ minWidth: '70px', textAlign: 'right', fontWeight: 'bold', color: 'var(--accent-cyan)' }}>{(draftHostSettings.bitrate / 1000).toFixed(1)} Mbps</span>
                </div>
              </div>
              <div className="settings-group full-width">
                <label htmlFor="hostMouseQueue">Mouse Queue Limit</label>
                <select id="hostMouseQueue" value={draftHostSettings.mouseQueueLimit} onChange={(e) => updateDraftHostSettings('mouseQueueLimit', Number(e.target.value))}>
                  <option value={0}>0 B (Strict No Queue)</option>
                  <option value={64}>64 B (Ultra Low Buffer)</option>
                  <option value={256}>256 B (Recommended)</option>
                  <option value={1024}>1024 B (Moderate Buffer)</option>
                  <option value={4096}>4096 B (High Buffer)</option>
                  <option value={16384}>16384 B (Previous Default)</option>
                </select>
              </div>
              <div className="settings-group full-width">
                <label htmlFor="hostInputProtocol">Input Protocol</label>
                <select id="hostInputProtocol" value={draftHostSettings.inputProtocol} onChange={(e) => updateDraftHostSettings('inputProtocol', e.target.value)}>
                  <option value="webrtc">WebRTC Data Channels (SCTP)</option>
                  <option value="webtransport" disabled={typeof (window as any).WebTransport === 'undefined'}>WebTransport QUIC Datagrams {typeof (window as any).WebTransport === 'undefined' ? '(Unsupported)' : '(Experimental)'}</option>
                </select>
              </div>
              {!!(window as any).__TAURI__ && (
                <div className="settings-checkbox-group">
                  <input id="hostNativeClient" type="checkbox" checked={draftHostSettings.useNativeClient} onChange={(e) => updateDraftHostSettings('useNativeClient', e.target.checked)} />
                  <label htmlFor="hostNativeClient">Use native client binary</label>
                </div>
              )}
              <div className="settings-checkbox-group">
                <input id="hostCanvasRenderer" type="checkbox" checked={draftHostSettings.useCanvasRenderer} disabled={typeof (window as any).MediaStreamTrackProcessor === 'undefined'} onChange={(e) => updateDraftHostSettings('useCanvasRenderer', e.target.checked)} />
                <label htmlFor="hostCanvasRenderer">Use Canvas Renderer {typeof (window as any).MediaStreamTrackProcessor === 'undefined' ? '(Unsupported)' : ''}</label>
              </div>
              <div className="settings-checkbox-group">
                <input id="hostVirtualDisplay" type="checkbox" checked={draftHostSettings.virtualDisplay} onChange={(e) => updateDraftHostSettings('virtualDisplay', e.target.checked)} />
                <label htmlFor="hostVirtualDisplay">Create Virtual Display</label>
              </div>
            </div>
            <div className="settings-actions">
              <button onClick={() => setSettingsHost(null)} className="btn-secondary">Cancel</button>
              <button onClick={applyHostSettings} className="btn-primary">Save Settings</button>
            </div>
          </div>
        </div>
      )}

      </main>


    </div>
  );
}

export default App;
