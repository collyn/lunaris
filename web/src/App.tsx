import React, { useState, useEffect } from 'react';
import { StreamPlayer } from './components/StreamPlayer';
import './App.css';

interface Host {
  id: string;
  name: string;
  status: 'Online' | 'Offline' | 'Busy';
  ip_address: string | null;
  server_codec_mode_support?: number;
  agent_connected?: boolean;
}

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
  const [isRegister, setIsRegister] = useState<boolean>(false);
  const [authUsername, setAuthUsername] = useState<string>('');
  const [authPassword, setAuthPassword] = useState<string>('');
  const [authConfirmPassword, setAuthConfirmPassword] = useState<string>('');
  const [authServerHost, setAuthServerHost] = useState<string>(() => localStorage.getItem('lunaris_server_host') || 'http://localhost:8080');
  const [authError, setAuthError] = useState<string | null>(null);
  const [authLoading, setAuthLoading] = useState<boolean>(false);

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

  // Sunshine Host Configuration Modal State
  const [showSettingsModal, setShowSettingsModal] = useState<boolean>(false);
  const [showPairingPage, setShowPairingPage] = useState<boolean>(false);
  const [settingsHost, setSettingsHost] = useState<Host | null>(null);
  const [modalEncoder, setModalEncoder] = useState<string>('default');
  const [modalPreset, setModalPreset] = useState<string>('default');
  const [modalPort, setModalPort] = useState<string>('47989');
  const [modalRawConfig, setModalRawConfig] = useState<string>('');
  const [modalLoading, setModalLoading] = useState<boolean>(false);
  const [modalError, setModalError] = useState<string | null>(null);
  const [modalSuccess, setModalSuccess] = useState<boolean>(false);

  const openHostSettings = (host: Host) => {
    setSettingsHost(host);
    setShowSettingsModal(true);
    
    if (host.agent_connected === false) {
      setModalLoading(false);
      setModalError("This host is registered as a direct Sunshine host (no agent running). remote Sunshine configuration is only supported for hosts running the Lunaris agent.");
      setModalSuccess(false);
      return;
    }

    setModalLoading(true);
    setModalError(null);
    setModalSuccess(false);

    const protocol = getBackendProtocol().ws;
    const serverHost = getBackendHost();
    const wsUrl = `${protocol}//${serverHost}/ws/client?token=${encodeURIComponent(token || '')}`;

    const ws = new WebSocket(wsUrl);
    
    ws.onopen = () => {
      // Send GetSunshineConfig request
      const req = {
        event: "Signaling",
        data: {
          type: "GetSunshineConfig",
          payload: {
            target_id: host.id
          }
        }
      };
      ws.send(JSON.stringify(req));
    };

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.event === "Signaling" && msg.data) {
          const type = msg.data.type;
          const payload = msg.data.payload;
          
          if (type === "SunshineConfigResponse") {
            setModalLoading(false);
            const configText = payload.config;
            let configMap: Record<string, string> = {};
            try {
              configMap = JSON.parse(configText);
            } catch (e) {
              console.error("Failed to parse config as JSON:", e);
            }
            
            // Set fields
            setModalEncoder(configMap.encoder || 'default');
            setModalPreset(configMap.preset || 'default');
            setModalPort(configMap.port || '47989');
            
            // Extract raw fields
            let rawLines = [];
            for (const [k, v] of Object.entries(configMap)) {
              if (k !== 'encoder' && k !== 'preset' && k !== 'port') {
                rawLines.push(`${k} = ${v}`);
              }
            }
            setModalRawConfig(rawLines.join('\n'));
          } else if (type === "UpdateSunshineConfigResponse") {
            setModalLoading(false);
            if (payload.success) {
              setModalSuccess(true);
              setTimeout(() => {
                closeHostSettings();
              }, 1500);
            } else {
              setModalError(payload.error || "Failed to update Sunshine configuration");
            }
          } else if (type === "Error") {
            setModalLoading(false);
            setModalError(payload.message || "An error occurred");
          }
        }
      } catch (e) {
        console.error("Error processing WebSocket message:", e);
      }
    };

    ws.onerror = (e) => {
      console.error("WebSocket error:", e);
      setModalLoading(false);
      setModalError("Failed to connect to signaling server");
    };

    ws.onclose = () => {
      console.log("Modal WebSocket connection closed");
    };

    (window as any)._modalWs = ws;
  };

  const closeHostSettings = () => {
    if ((window as any)._modalWs) {
      (window as any)._modalWs.close();
      (window as any)._modalWs = null;
    }
    setShowSettingsModal(false);
    setSettingsHost(null);
    setModalError(null);
    setModalSuccess(false);
  };

  const saveHostSettings = () => {
    if (!settingsHost) return;
    const ws = (window as any)._modalWs;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      setModalError("WebSocket connection is not open");
      return;
    }
    
    setModalLoading(true);
    setModalError(null);
    setModalSuccess(false);

    // Reconstruct configuration map
    const configMap: Record<string, string> = {};
    if (modalEncoder && modalEncoder !== 'default') {
      configMap.encoder = modalEncoder;
    }
    if (modalPreset && modalPreset !== 'default') {
      configMap.preset = modalPreset;
    }
    if (modalPort) {
      configMap.port = modalPort;
    }

    // Parse raw options
    const lines = modalRawConfig.split('\n');
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      const pos = trimmed.indexOf('=');
      if (pos !== -1) {
        const k = trimmed.substring(0, pos).trim();
        const v = trimmed.substring(pos + 1).trim();
        if (k) {
          configMap[k] = v;
        }
      }
    }

    // Send update request
    const req = {
      event: "Signaling",
      data: {
        type: "UpdateSunshineConfig",
        payload: {
          target_id: settingsHost.id,
          config: JSON.stringify(configMap)
        }
      }
    };
    ws.send(JSON.stringify(req));
  };


  // Pairing Form State
  const [pairName, setPairName] = useState<string>('');
  const [pairIp, setPairIp] = useState<string>('');
  const [sunshineUsername, setSunshineUsername] = useState<string>('');
  const [sunshinePassword, setSunshinePassword] = useState<string>('');
  const [pairLoading, setPairLoading] = useState<boolean>(false);
  const [pairError, setPairError] = useState<string | null>(null);
  const [pairSuccess, setPairSuccess] = useState<boolean>(false);

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

  const downloadAgentConfig = () => {
    if (!agentToken) return;
    
    // Construct default ws/wss URL
    const protocol = getBackendProtocol().ws;
    const serverHost = getBackendHost();
    const serverUrl = `${protocol}//${serverHost}`;

    const configObj = {
      client_unique_id: "",
      client_private_key: "",
      client_certificate: "",
      server_certificate: "",
      server_url: serverUrl,
      server_token: agentToken
    };

    const dataStr = "data:text/json;charset=utf-8," + encodeURIComponent(JSON.stringify(configObj, null, 2));
    const downloadAnchor = document.createElement('a');
    downloadAnchor.setAttribute("href",     dataStr);
    downloadAnchor.setAttribute("download", "agent_config.json");
    document.body.appendChild(downloadAnchor);
    downloadAnchor.click();
    downloadAnchor.remove();
  };

  useEffect(() => {
    if (token) {
      fetchHosts();
      fetchAgentToken();
      // Periodically refresh host list
      const interval = setInterval(fetchHosts, 5000);
      return () => clearInterval(interval);
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

    ws.onopen = () => {
      if (!isMounted) return;
      ws.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "GetAppList",
          payload: { target_id: viewingHost.id }
        }
      }));
    };

    ws.onmessage = (event) => {
      if (!isMounted) return;
      try {
        const msg = JSON.parse(event.data);
        if (msg.event === "Signaling" && msg.data) {
          const type = msg.data.type;
          const payload = msg.data.payload;

          if (type === "AppListResponse") {
            setViewingApps(payload.apps);
            setViewingAppsLoading(false);
            ws.close();
          } else if (type === "Error") {
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
      setViewingAppsError("Connection error while fetching applications");
      setViewingAppsLoading(false);
    };

    ws.onclose = () => {
      if (!isMounted) return;
      setViewingAppsLoading(false);
    };

    return () => {
      isMounted = false;
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
    if (isRegister && authPassword !== authConfirmPassword) {
      setAuthError("Passwords do not match");
      return;
    }

    setAuthLoading(true);
    const endpoint = isRegister ? '/api/auth/register' : '/api/auth/login';
    
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
        setToken(data.token);
        setUsername(data.username);
        // Clear fields
        setAuthUsername('');
        setAuthPassword('');
        setAuthConfirmPassword('');
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
    setToken(null);
    setUsername(null);
    setHosts([]);
    setSelectedHost(null);
  };

  const handlePairHost = async (e: React.FormEvent) => {
    e.preventDefault();
    setPairError(null);
    setPairSuccess(false);

    if (!pairName.trim() || !pairIp.trim() || !sunshineUsername.trim() || !sunshinePassword.trim()) {
      setPairError("All fields are required");
      return;
    }

    setPairLoading(true);
    try {
      const serverHost = getBackendHost();
      const response = await fetch(`${getBackendProtocol().http}//${serverHost}/api/hosts/pair`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`
        },
        body: JSON.stringify({
          name: pairName,
          ip_address: pairIp,
          sunshine_username: sunshineUsername,
          sunshine_password: sunshinePassword
        })
      });

      const data = await response.json();

      if (response.ok) {
        setPairSuccess(true);
        setPairName('');
        setPairIp('');
        setSunshineUsername('');
        setSunshinePassword('');
        fetchHosts();
      } else {
        setPairError(data.error || "Failed to pair with host");
      }
    } catch (err) {
      setPairError("Connection to server failed during pairing");
    } finally {
      setPairLoading(false);
    }
  };

  const handleUnpairHost = async (hostId: string) => {
    if (!window.confirm("Are you sure you want to unpair and remove this host?")) {
      return;
    }

    try {
      const serverHost = getBackendHost();
      const response = await fetch(`${getBackendProtocol().http}//${serverHost}/api/hosts/${hostId}`, {
        method: 'DELETE',
        headers: {
          'Authorization': `Bearer ${token}`
        }
      });

      if (response.ok) {
        fetchHosts();
      } else {
        alert("Failed to unpair host");
      }
    } catch (err) {
      alert("Failed to connect to server to unpair host");
    }
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
          <span className="badge-tech">v0.1.0</span>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: '1rem' }}>
          {token && username && (
            <div className="nav-user-panel">
              <div className="user-info" style={{ alignItems: 'flex-end' }}>
                <span className="user-label">SERVER:</span>
                <span className="username" style={{ fontSize: '0.85rem', color: 'var(--accent-cyan)' }}>
                  {localStorage.getItem('lunaris_server_host') || 'http://localhost:8080'}
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
                <h2>{isRegister ? 'Create Account' : 'Welcome Back'}</h2>
                <p>{isRegister ? 'Register your Lunaris account' : 'Access your remote desktop network'}</p>
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
                    autoComplete={isRegister ? "new-password" : "current-password"}
                  />
                </div>

                {isRegister && (
                  <div className="form-group">
                    <label htmlFor="confirmPassword">Confirm Password</label>
                    <input
                      type="password"
                      id="confirmPassword"
                      value={authConfirmPassword}
                      onChange={(e) => setAuthConfirmPassword(e.target.value)}
                      placeholder="Verify password"
                      required
                      autoComplete="new-password"
                    />
                  </div>
                )}

                <button type="submit" disabled={authLoading} className="btn-primary auth-submit-btn">
                  {authLoading ? (
                    <div className="inline-loader"></div>
                  ) : (
                    isRegister ? 'Register' : 'Connect Account'
                  )}
                </button>
              </form>

              <div className="auth-switch-prompt">
                <span>{isRegister ? 'Already have an account?' : 'New to Lunaris?'}</span>
                <button 
                  type="button" 
                  onClick={() => {
                    setIsRegister(!isRegister);
                    setAuthError(null);
                  }}
                  className="btn-link"
                >
                  {isRegister ? 'Login' : 'Create an account'}
                </button>
              </div>
            </div>
          </div>
        ) : showPairingPage ? (
          /* Dedicated Setup Page View */
          <div className="dashboard-grid setup-layout">
            <div className="dashboard-main">
              <div className="apps-navigation">
                <button 
                  onClick={() => setShowPairingPage(false)} 
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
                  <span className="breadcrumb-item active">Pair & Setup</span>
                </div>
              </div>

              <div className="section-header">
                <div>
                  <h1 className="section-title">Host Pairing & Setup</h1>
                  <p className="section-subtitle">Pair with local Sunshine devices or configure agents</p>
                </div>
              </div>

              {/* Pair New Host Card */}
              <div className="sidebar-card pair-host-card">
                <h3>Pair New Host</h3>
                <p className="sidebar-desc">Pair directly with a device running Sunshine on your network.</p>
                
                {pairError && (
                  <div className="auth-error-banner" style={{ marginBottom: '15px' }}>
                    <span className="error-icon">⚠️</span>
                    <span>{pairError}</span>
                  </div>
                )}
                {pairSuccess && (
                  <div className="success-banner" style={{ marginBottom: '15px', color: '#10b981', background: 'rgba(16, 185, 129, 0.1)', padding: '10px', borderRadius: '6px', fontSize: '0.875rem' }}>
                    <span>✅ Host paired successfully!</span>
                  </div>
                )}

                <form onSubmit={handlePairHost} className="auth-form">
                  <div className="form-group">
                    <label htmlFor="pair-name">Host Name</label>
                    <input
                      type="text"
                      id="pair-name"
                      value={pairName}
                      onChange={(e) => setPairName(e.target.value)}
                      placeholder="e.g. Gaming PC"
                      required
                    />
                  </div>

                  <div className="form-group">
                    <label htmlFor="pair-ip">IP Address</label>
                    <input
                      type="text"
                      id="pair-ip"
                      value={pairIp}
                      onChange={(e) => setPairIp(e.target.value)}
                      placeholder="e.g. 192.168.1.100"
                      required
                    />
                  </div>

                  <div className="form-group">
                    <label htmlFor="sunshine-username">Sunshine Username</label>
                    <input
                      type="text"
                      id="sunshine-username"
                      value={sunshineUsername}
                      onChange={(e) => setSunshineUsername(e.target.value)}
                      placeholder="e.g. admin"
                      required
                    />
                  </div>

                  <div className="form-group">
                    <label htmlFor="sunshine-password">Sunshine Password</label>
                    <input
                      type="password"
                      id="sunshine-password"
                      value={sunshinePassword}
                      onChange={(e) => setSunshinePassword(e.target.value)}
                      placeholder="Enter password"
                      required
                    />
                  </div>

                  <button type="submit" disabled={pairLoading} className="btn-primary auth-submit-btn">
                    {pairLoading ? (
                      <div className="inline-loader"></div>
                    ) : (
                      'Pair & Add Host'
                    )}
                  </button>
                </form>
              </div>
            </div>

            {/* Right side - Host Agent Setup */}
            <div className="dashboard-sidebar">
              <div className="sidebar-card">
                <h3>Host Agent Setup</h3>
                <p className="sidebar-desc">Install and configure the Lunaris Agent on your remote host machine.</p>
                
                {agentToken && (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: '10px', marginTop: '12px' }}>
                    <div className="form-group" style={{ margin: 0 }}>
                      <label style={{ fontSize: '11px' }}>Signaling Server URL</label>
                      <input 
                        type="text" 
                        readOnly 
                        value={`${getBackendProtocol().ws}//${getBackendHost()}`}
                        style={{ fontFamily: 'monospace', fontSize: '11px', padding: '6px', background: 'rgba(255,255,255,0.05)', color: 'var(--text-secondary)' }}
                      />
                    </div>
                    <div className="form-group" style={{ margin: 0 }}>
                      <label style={{ fontSize: '11px' }}>Agent Connection Token</label>
                      <div style={{ display: 'flex', gap: '6px' }}>
                        <input 
                          type="password" 
                          readOnly 
                          value={agentToken}
                          id="client-agent-token-field"
                          style={{ fontFamily: 'monospace', fontSize: '11px', padding: '6px', background: 'rgba(255,255,255,0.05)', color: 'var(--text-secondary)', flex: 1 }}
                        />
                        <button 
                          type="button"
                          className="btn-secondary" 
                          style={{ padding: '6px 10px', fontSize: '11px', border: '1px solid var(--border-color)' }}
                          onClick={() => {
                            const el = document.getElementById('client-agent-token-field') as HTMLInputElement;
                            if (el) {
                              el.type = el.type === 'password' ? 'text' : 'password';
                            }
                          }}
                        >
                          👁️
                        </button>
                      </div>
                    </div>
                    <button 
                      type="button"
                      className="btn-secondary"
                      onClick={downloadAgentConfig}
                      style={{ marginTop: '6px', padding: '8px', fontSize: '0.85rem', width: '100%' }}
                    >
                      📥 Download agent_config.json
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
        ) : (
          /* Main Dashboard - Full Width */
          <div className="dashboard-full-width">
            <div className="dashboard-main full-width">
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
                      <p className="section-subtitle">Select and stream Sunshine-configured apps from <strong>{viewingHost.name}</strong></p>
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
                      <p>There are no Sunshine applications configured on {viewingHost.name}.</p>
                      <p className="empty-hint">Please open Sunshine Web UI on the host and add applications first.</p>
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

                              {/* Title overlay in Moonlight style */}
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
                                    const codec = localStorage.getItem('lunaris_stream_codec') || 'h264';
                                    const mouseQueueLimit = localStorage.getItem('lunaris_mouse_queue_limit') || '256';
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
                                        hostName: hostToUse.name
                                      }).catch((err: any) => {
                                        console.error("Failed to launch native client:", err);
                                        alert("Failed to launch native client: " + err);
                                      });
                                    } else {
                                      const wsProtocol = getBackendProtocol().ws;
                                      const wsServerUrl = `${wsProtocol}//${serverHost}`;
                                      window.location.href = `lunaris://connect?host_id=${hostToUse.id}&server=${wsServerUrl}&token=${token}&res=${resStr}&fps=${fps}&bitrate=${bitrate}&codec=${codec}&mouse_queue_limit=${mouseQueueLimit}&host_name=${encodeURIComponent(hostToUse.name)}&app_id=${app.id}`;
                                    }
                                  }}
                                  className="overlay-btn btn-launch-app"
                                  title="Launch in Moonlight Native Client"
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
                      <button 
                        onClick={() => setShowPairingPage(true)} 
                        className="btn-primary"
                        style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', padding: '0.6rem 1.2rem', fontSize: '0.9rem' }}
                        title="Pair a new Sunshine host or setup an agent"
                      >
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                          <line x1="12" y1="5" x2="12" y2="19" />
                          <line x1="5" y1="12" x2="19" y2="12" />
                        </svg>
                        Add Device
                      </button>
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
                      <h3>No host devices paired</h3>
                      <p>There are no paired host devices registered with the server.</p>
                      <p className="empty-hint" style={{ marginBottom: '1.25rem' }}>Click the button below to pair and setup a new remote device.</p>
                      <button 
                        onClick={() => setShowPairingPage(true)} 
                        className="btn-primary"
                        style={{ display: 'inline-flex', alignItems: 'center', gap: '0.5rem' }}
                      >
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                          <line x1="12" y1="5" x2="12" y2="19" />
                          <line x1="5" y1="12" x2="19" y2="12" />
                        </svg>
                        Pair & Setup Device
                      </button>
                    </div>
                  ) : (
                    <div className="hosts-card-grid">
                      {hosts.map((host) => {
                        const isOnline = host.status === 'Online';
                        const isBusy = host.status === 'Busy';
                        
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

                            <div className="host-card-footer" style={{ display: 'flex', justifyContent: 'flex-end', alignItems: 'center', width: '100%', gap: '8px' }}>
                              {(isOnline || isBusy) && (
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    openHostSettings(host);
                                  }}
                                  className="btn-settings-icon"
                                  title="Sunshine Configuration"
                                  style={{
                                    padding: '8px',
                                    borderRadius: '6px',
                                    background: 'rgba(0, 240, 255, 0.1)',
                                    border: '1px solid rgba(0, 240, 255, 0.2)',
                                    color: 'var(--accent-cyan)',
                                    cursor: 'pointer',
                                    display: 'flex',
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    transition: 'all 0.2s',
                                    height: '42px',
                                    width: '42px'
                                  }}
                                >
                                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                                    <circle cx="12" cy="12" r="3" />
                                    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
                                  </svg>
                                </button>
                              )}
                              <button
                                onClick={(e) => {
                                  e.stopPropagation();
                                  handleUnpairHost(host.id);
                                }}
                                className="btn-danger-icon"
                                title="Unpair & Remove Host"
                                style={{
                                  padding: '8px',
                                  borderRadius: '6px',
                                  background: 'rgba(239, 68, 68, 0.1)',
                                  border: '1px solid rgba(239, 68, 68, 0.2)',
                                  color: '#ef4444',
                                  cursor: 'pointer',
                                  display: 'flex',
                                  alignItems: 'center',
                                  justifyContent: 'center',
                                  transition: 'all 0.2s',
                                  height: '42px',
                                  width: '42px'
                                }}
                              >
                                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                                  <polyline points="3 6 5 6 21 6" />
                                  <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                                  <line x1="10" y1="11" x2="10" y2="17" />
                                  <line x1="14" y1="11" x2="14" y2="17" />
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
            </div>
          </div>
        )}
      </main>

      {showSettingsModal && settingsHost && (
        <div className="host-settings-overlay">
          <div className="host-settings-card">
            <div className="host-settings-header">
              <h3 className="host-settings-title">Configure Sunshine — {settingsHost.name}</h3>
              <button onClick={closeHostSettings} className="btn-close-modal" title="Close Settings">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                  <line x1="18" y1="6" x2="6" y2="18"></line>
                  <line x1="6" y1="6" x2="18" y2="18"></line>
                </svg>
              </button>
            </div>
            
            <div className="host-settings-body">
              {modalError && (
                <div className="host-settings-alert error">
                  <span>⚠️</span>
                  <span>{modalError}</span>
                </div>
              )}
              {modalSuccess && (
                <div className="host-settings-alert success">
                  <span>✅</span>
                  <span>Configuration saved successfully! Sunshine is restarting...</span>
                </div>
              )}
              {modalLoading && (
                <div className="host-settings-alert info">
                  <div className="inline-loader" style={{ marginRight: '8px' }}></div>
                  <span>Processing... Please wait...</span>
                </div>
              )}
              
              <div className="settings-grid" style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1.25rem' }}>
                <div className="settings-group" style={{ display: 'flex', flexDirection: 'column', gap: '0.5rem' }}>
                  <label htmlFor="modal-encoder" style={{ fontSize: '0.85rem', fontWeight: 600, color: 'var(--text-muted)' }}>Video Encoder</label>
                  <select
                    id="modal-encoder"
                    value={modalEncoder}
                    onChange={(e) => setModalEncoder(e.target.value)}
                    style={{
                      background: 'rgba(30, 36, 56, 0.5)',
                      border: '1px solid rgba(255, 255, 255, 0.1)',
                      color: '#fff',
                      padding: '0.75rem 1rem',
                      borderRadius: '8px',
                      fontSize: '0.95rem',
                      outline: 'none'
                    }}
                  >
                    <option value="default">Auto / Default</option>
                    <option value="nvenc">NVIDIA NVENC</option>
                    <option value="amdvce">AMD AMF</option>
                    <option value="vaapi">VA-API (Intel/AMD)</option>
                    <option value="software">Software (x264)</option>
                  </select>
                </div>

                <div className="settings-group" style={{ display: 'flex', flexDirection: 'column', gap: '0.5rem' }}>
                  <label htmlFor="modal-preset" style={{ fontSize: '0.85rem', fontWeight: 600, color: 'var(--text-muted)' }}>Encoder Preset</label>
                  <select
                    id="modal-preset"
                    value={modalPreset}
                    onChange={(e) => setModalPreset(e.target.value)}
                    style={{
                      background: 'rgba(30, 36, 56, 0.5)',
                      border: '1px solid rgba(255, 255, 255, 0.1)',
                      color: '#fff',
                      padding: '0.75rem 1rem',
                      borderRadius: '8px',
                      fontSize: '0.95rem',
                      outline: 'none'
                    }}
                  >
                    <option value="default">Default</option>
                    <option value="ultrafast">Ultrafast</option>
                    <option value="superfast">Superfast</option>
                    <option value="veryfast">Veryfast</option>
                    <option value="faster">Faster</option>
                    <option value="fast">Fast</option>
                    <option value="medium">Medium</option>
                    <option value="slow">Slow</option>
                  </select>
                </div>

                <div className="settings-group full-width" style={{ gridColumn: 'span 2', display: 'flex', flexDirection: 'column', gap: '0.5rem' }}>
                  <label htmlFor="modal-port" style={{ fontSize: '0.85rem', fontWeight: 600, color: 'var(--text-muted)' }}>Port (HTTPS Web/API)</label>
                  <input
                    type="text"
                    id="modal-port"
                    value={modalPort}
                    onChange={(e) => setModalPort(e.target.value)}
                    placeholder="47989"
                    style={{
                      background: 'rgba(30, 36, 56, 0.5)',
                      border: '1px solid rgba(255, 255, 255, 0.1)',
                      color: '#fff',
                      padding: '0.75rem 1rem',
                      borderRadius: '8px',
                      fontSize: '0.95rem',
                      outline: 'none'
                    }}
                  />
                </div>

                <div className="settings-group full-width" style={{ gridColumn: 'span 2', display: 'flex', flexDirection: 'column', gap: '0.5rem' }}>
                  <label htmlFor="modal-raw" style={{ fontSize: '0.85rem', fontWeight: 600, color: 'var(--text-muted)' }}>Additional Options (key = value)</label>
                  <textarea
                    id="modal-raw"
                    value={modalRawConfig}
                    onChange={(e) => setModalRawConfig(e.target.value)}
                    placeholder="e.g.&#10;min_port = 47990&#10;max_port = 48010"
                    className="textarea-config"
                    style={{
                      fontFamily: 'monospace',
                      fontSize: '0.85rem',
                      minHeight: '120px',
                      resize: 'vertical',
                      backgroundColor: 'var(--bg-tertiary)',
                      border: '1px solid var(--border-color)',
                      color: '#a5b4fc',
                      padding: '0.75rem',
                      borderRadius: '8px',
                      outline: 'none'
                    }}
                  />
                </div>
              </div>

              <div className="settings-actions" style={{ display: 'flex', gap: '1rem', marginTop: '2rem' }}>
                <button
                  onClick={closeHostSettings}
                  className="btn-secondary"
                  style={{ flex: 1, padding: '0.85rem', fontWeight: 600, borderRadius: '8px' }}
                >
                  Cancel
                </button>
                <button
                  onClick={saveHostSettings}
                  disabled={modalLoading}
                  className="btn-primary"
                  style={{ flex: 1, padding: '0.85rem', fontWeight: 600, borderRadius: '8px' }}
                >
                  Save & Apply
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
