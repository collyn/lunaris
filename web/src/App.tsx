import React, { useState, useEffect, useRef, useMemo } from 'react';
import { LANGUAGES, getLang, setLang, t, type Lang } from './i18n';
import StreamSettingsDialog from './components/StreamSettingsDialog';
import ComputerSettingsDialog from './components/ComputerSettingsDialog';
import { resolutionString } from './streamOptions';
import {
  differences, hostPrefsKey, linkCodec, loadDefaults, measureRefresh, prefsFor, saveComputerPrefs, saveDefaults,
  sourcePrefsKey, type ComputerPrefs,
} from './streamPrefs';
import {
  accountKey, arrangeCards, loadFavourites, loadRecent, loadShow, recordRecent, saveShow, toggleFavourite, type CardShow,
} from './favourites';
import { AdminServerInfo, AboutDialog } from './components/ServerInfo';
import { StreamPlayer } from './components/StreamPlayer';
import { AdminUsers } from './components/AdminUsers';
import { AdminGroups } from './components/AdminGroups';
import { AdminTurnServers } from './components/AdminTurnServers';
import { AdminInstallAgents } from './components/AdminInstallAgents';
import { AccountSecurity } from './components/AccountSecurity';
import { isCompleteTotp, isPlausibleRecoveryCode, normalizeRecoveryInput, normalizeTotpInput, parseLoginAnswer, type LoginAnswer } from './mfa';
import FileManager from './components/FileManager';
import SetupPage from './components/SetupPage';
import { parseSetupStatus } from './setup';
import {
  browserStore, buildCards, choiceAsList, currentMethod, effectiveType, filterCards, layoutFields, loadMethods, loadTypeFilter, loadView, methodType,
  newGroup, rememberMethod, saveTypeFilter, saveView, typeShortName, typesOf, type Card, type ComputerView, type Method,
} from './computerCards';
import { typeLogo } from './typeLogos';
import './App.css';

interface Host {
  id: string;
  name: string;
  status: 'Online' | 'Offline' | 'Busy';
  ip_address: string | null;
  server_codec_mode_support?: number;
  agent_connected?: boolean;
  /** Added by address: the server connects to the computer ("host:port"). */
  address?: string;
  /** Why the server can't reach a computer added by address. */
  link_error?: string;
  /** Added without its password: each connection is accepted there. */
  needs_approval?: boolean;
  /** Its own IPv4 addresses (LAN, VPN) as its agent last said them, kept
   *  while it is offline. Not there when not known (an older agent). */
  addresses?: string[];
  /** The name its agent reports, when an administrator gave it another one. */
  agent_name?: string;
  /** The server's own gateway (server/src/gateway.rs): it has no screen of
   *  its own, only the computers added through it; not a card, managed on
   *  the Server page. */
  gateway?: boolean;
}

/** One field of a connector's form ("kind": text, port, secret, or choice:
 *  one of `options`). */
interface ConnectorField {
  key: string;
  label: string;
  kind: string;
  default?: string;
  required?: boolean;
  options?: { value: string; label: string }[];
}

/** What a choice field is chosen from, when it says so in a way that can be shown. */
const choicesOf = (field: ConnectorField): { value: string; label: string }[] =>
  (field.kind === 'choice' && Array.isArray(field.options) ? field.options : [])
    .filter(o => o && typeof o.value === 'string' && typeof o.label === 'string');

/** The mark of a type of computer: its logo when it has one, else its first
 *  letter. `id` is "lunaris" or a connector's id; `name` is for the letter. */
const TypeMark = ({ id, name, small }: { id: string; name: string; small?: boolean }) => {
  const logo = typeLogo(id);
  return (
    <span className={`type-mark${small ? ' small' : ''}${logo ? ' logo' : ''}`} aria-hidden="true">
      {logo ? <img src={logo} alt="" draggable={false} /> : (name.trim()[0] || '?').toUpperCase()}
    </span>
  );
};

/** A type of computer other than Lunaris; its form is these fields. */
interface Connector {
  id: string;
  name: string;
  fields: ConnectorField[];
  /** One or two sentences for the person, under the Type choice. */
  note?: string;
}

/** A Lunaris computer that can show other computers, and through what. */
interface ConnectorHost {
  host_id: string;
  host_name: string;
  connectors: Connector[];
}

/** A computer of another type, reached through the Lunaris computer `host_id`. */
interface Source {
  host_id: string;
  host_name: string;
  id: string;
  name: string;
  connector: string;
  connector_name: string;
  online: boolean;
  /** What its connector can do. Not there when the server or the Lunaris
   *  computer is from before this: then what was always offered is. */
  capabilities?: { files?: boolean; clipboard?: boolean; audio?: boolean; input?: boolean };
  /** Which computer it is, when that computer is listed in other ways too
   *  (computerCards.ts). Not there when it has none. */
  group?: string;
  /** Where it is: its host, with the port when one is set ("192.168.1.5:3389"). */
  address?: string;
  /** The fields of its form that are not secrets (administrators only), for
   *  the edit form. Passwords are never sent. */
  values?: Record<string, string>;
}

/** A computer's card and one of its ways to connect (computerCards.ts). */
type ComputerCard = Card<Host, Source>;
type ComputerMethod = Method<Host, Source>;

/** Up to this many choices are all shown; more are a list to open. */
const MAX_OPTION_CARDS = 4;

/** A Lunaris computer that is only a place in the list yet (added by
 *  address, never reached): it has no id of its own to group with. */
const isPlaceholderHost = (host: Host) => host.id.startsWith('address:');

/** Whether a computer of another type has files to offer: yes unless its
 *  connector says it has none. */
const sourceHasFiles = (source: Source): boolean => source.capabilities?.files !== false;

const cardIcon = (children: React.ReactNode, size = 16) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {children}
  </svg>
);
const ICON_FILES = cardIcon(<><path d="M3 6a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z" /><path d="M12 10v6M9 13l3-3 3 3" /></>);
const ICON_EDIT = cardIcon(<><path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z" /></>);
const ICON_TRASH = cardIcon(<><polyline points="3 6 5 6 21 6" /><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" /><path d="M10 11v6" /><path d="M14 11v6" /><path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" /></>);
const ICON_APP = cardIcon(<><rect x="3" y="4" width="18" height="12" rx="1.5" /><path d="M8 20h8M12 16v4" /></>);
const ICON_GEAR = cardIcon(<><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06A1.65 1.65 0 0 0 15 19.4a1.65 1.65 0 0 0-1 .6 1.65 1.65 0 0 0-.4 1.08V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 8.6 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-.6-1 1.65 1.65 0 0 0-1.08-.4H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 8.6a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-.6 1.65 1.65 0 0 0 .4-1.08V3a2 2 0 1 1 4 0v.09A1.65 1.65 0 0 0 15.4 4.6a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9c.38.16.72.4 1 .7.29.29.6.9.6 1.3V12a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" /></>);
const ICON_SLIDERS = cardIcon(<><path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0" /><circle cx="16" cy="6" r="2" /><circle cx="10" cy="12" r="2" /><circle cx="18" cy="18" r="2" /></>);
const ICON_PLUS = cardIcon(<path d="M12 5v14M5 12h14" />);
const ICON_CHEVRON = cardIcon(<polyline points="6 9 12 15 18 9" />, 14);
const ICON_STAR = cardIcon(<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />);
const ICON_MORE = cardIcon(<><circle cx="5" cy="12" r="1.2" /><circle cx="12" cy="12" r="1.2" /><circle cx="19" cy="12" r="1.2" /></>);
const ICON_KEY = cardIcon(<><circle cx="7.5" cy="15.5" r="4.5" /><path d="M10.7 12.3 21 2M16 7l3 3M18 5l2 2" /></>);
const ICON_INFO = cardIcon(<><circle cx="12" cy="12" r="10" /><path d="M12 16v-4M12 8h.01" /></>, 18);

/** "host:8091" -> "host" (the default port isn't worth showing). */
const shortAddress = (a: string) => a.replace(/:8091$/, '');

/** Where a Lunaris computer is, for its card: its own IP addresses as its
 *  agent said them, else the address it was added by; '' when not known. */
const hostAddressText = (host: Host): string =>
  (host.addresses || []).filter(Boolean).join(', ') || (host.address ? shortAddress(host.address) : '') || host.ip_address || '';

type HostStreamSettings = {
  resolution: string;
  fps: number;
  codec: string;
  /** '420' | '444' (4:4:4 is H.264 only) */
  chroma: string;
  bitrate: number;
  mouseQueueLimit: number;
  inputProtocol: string;
  useNativeClient: boolean;
  useCanvasRenderer: boolean;
  encoder: string;
  display: string;
};

/** The settings the dialog shows: `computer`'s own picture settings (else
 *  the defaults; null: the defaults themselves), and this browser's others. */
const loadHostStreamSettings = (computer: string | null = null): HostStreamSettings => ({
  ...prefsFor(localStorage, computer),
  mouseQueueLimit: Number(localStorage.getItem('lunaris_mouse_queue_limit') || '256'),
  inputProtocol: localStorage.getItem('lunaris_input_protocol') || 'webrtc',
  useNativeClient: localStorage.getItem('lunaris_tauri_use_native') === 'true',
  // Off unless chosen: drawing on a canvas costs about twice the CPU of a
  // <video> element. The key was renamed because the old one was written as
  // 'true' on every save, whether or not anyone had chosen it.
  useCanvasRenderer: localStorage.getItem('lunaris_canvas_renderer_v2') === 'true',
});

/** The picture settings of the dialog's values. */
const picturePrefs = (s: HostStreamSettings): ComputerPrefs => ({
  resolution: s.resolution, fps: s.fps, bitrate: s.bitrate, codec: s.codec, chroma: s.chroma, encoder: s.encoder, display: s.display,
});

/** Saves the dialog: the picture settings for `computer` (null: as the
 *  defaults), the others for this browser. */
const saveHostStreamSettings = (settings: HostStreamSettings, computer: string | null) => {
  if (computer) saveComputerPrefs(localStorage, computer, picturePrefs(settings));
  else saveDefaults(localStorage, picturePrefs(settings));
  localStorage.setItem('lunaris_mouse_queue_limit', String(settings.mouseQueueLimit));
  localStorage.setItem('lunaris_input_protocol', settings.inputProtocol);
  localStorage.setItem('lunaris_tauri_use_native', String(settings.useNativeClient));
  localStorage.setItem('lunaris_canvas_renderer_v2', String(settings.useCanvasRenderer));
};

const getCurrentBrowserServerUrl = () => {
  if (window.location.hostname === 'tauri.localhost' || window.location.protocol.startsWith('tauri')) {
    return 'http://localhost:8090';
  }

  if (window.location.origin && window.location.origin !== 'null') {
    return window.location.origin;
  }

  const protocol = window.location.protocol === 'https:' ? 'https:' : 'http:';
  return `${protocol}//${window.location.host || 'localhost:8090'}`;
};

const getBackendHost = () => {
  const savedHost = localStorage.getItem('lunaris_server_host');
  if (savedHost) {
    return savedHost.replace(/^(https?:\/\/)?/, '').replace(/\/$/, '');
  }
  if (window.location.port === '5173' || window.location.port === '3000') {
    return `${window.location.hostname}:8090`;
  }
  if (window.location.hostname === 'tauri.localhost' || window.location.protocol.startsWith('tauri')) {
    return 'localhost:8090';
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

type ThemeChoice = 'system' | 'light' | 'dark';

/** Apply the theme to <html>: no attribute means "follow the OS". */
const applyTheme = (choice: ThemeChoice) => {
  const root = document.documentElement;
  if (choice === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', choice);
};

const readThemeChoice = (): ThemeChoice => {
  try {
    const v = localStorage.getItem('lunaris_theme');
    return v === 'light' || v === 'dark' ? v : 'system';
  } catch {
    return 'system';
  }
};

/** A sign-in (and maybe a connection) asked for by a link. */
type LinkRequest = {
  session: string;
  user: string;
  /** Host id to remote-control right away. */
  connect: string | null;
  computer: string;
  /** Stream settings a `?token=` link carries. */
  settings: Record<string, string>;
  codecSupport: string | null;
};

/**
 * Links that carry a session: the desktop app opens the web console as
 * `<server>/#session=<token>&user=<name>[&connect=<host id>]`, and
 * `?token=<token>&host_id=<id>` does the same. The fragment never reaches
 * the server or its logs. The token is always removed from the address bar.
 * Whoever makes the link chooses the account, so it is applied without a
 * question only when this browser holds no login and no connection is asked;
 * otherwise App asks first.
 */
const takeLinkFromUrl = (): LinkRequest | null => {
  const hash = new URLSearchParams(window.location.hash.replace(/^#/, ''));
  const query = new URLSearchParams(window.location.search);
  let link: LinkRequest | null = null;
  const session = hash.get('session');
  if (session) {
    link = {
      session,
      user: hash.get('user') || '',
      connect: hash.get('connect') || null,
      computer: hash.get('name') || '',
      settings: {},
      codecSupport: null,
    };
  } else if (query.get('token') && query.get('host_id')) {
    const settings: Record<string, string> = {};
    for (const k of ['res', 'fps', 'bitrate', 'codec']) {
      const v = query.get(k);
      if (v) settings[k] = v;
    }
    link = {
      session: query.get('token') as string,
      user: '',
      connect: query.get('host_id'),
      computer: query.get('host_name') || '',
      settings,
      codecSupport: query.get('codec_support'),
    };
  }
  if (hash.has('session') || query.has('token') || query.has('host_id')) {
    for (const k of ['token', 'host_id', 'host_name', 'codec_support', 'res', 'fps', 'bitrate', 'codec']) {
      query.delete(k);
    }
    const qs = query.toString();
    window.history.replaceState(null, '', window.location.pathname + (qs ? `?${qs}` : ''));
  }
  return link;
};

/** Make the link's login this browser's login (and queue its connection). */
const applyLink = (link: LinkRequest) => {
  localStorage.setItem('lunaris_token', link.session);
  localStorage.setItem('lunaris_username', link.user);
  // The role comes from /api/auth/me; this page's own server is the server.
  localStorage.removeItem('lunaris_role');
  localStorage.removeItem('lunaris_server_host');
  // The auto-launch keys App reads on load.
  localStorage.removeItem('lunaris_auto_launch_host_id');
  localStorage.removeItem('lunaris_auto_launch_host_name');
  localStorage.removeItem('lunaris_auto_launch_codec_support');
  if (link.connect) {
    localStorage.setItem('lunaris_auto_launch_host_id', link.connect);
    localStorage.setItem('lunaris_auto_launch_host_name', link.computer || 'Remote computer');
    if (link.codecSupport) localStorage.setItem('lunaris_auto_launch_codec_support', link.codecSupport);
  }
  for (const [k, v] of Object.entries(link.settings)) localStorage.setItem(`lunaris_stream_${k}`, v);
};

let pendingLink: LinkRequest | null = takeLinkFromUrl();
if (pendingLink && !pendingLink.connect && !localStorage.getItem('lunaris_token')) {
  applyLink(pendingLink);
  pendingLink = null;
}

function App() {
  // A link that wants to sign this browser in as someone else, or to connect.
  const [linkRequest, setLinkRequest] = useState<LinkRequest | null>(pendingLink);
  const [token, setToken] = useState<string | null>(localStorage.getItem('lunaris_token'));
  const [username, setUsername] = useState<string | null>(localStorage.getItem('lunaris_username'));

  // Check for deep link query parameters or local storage auto-launch flags on load
  useEffect(() => {
    // Only this browser's own saved login and auto-launch keys: a link's
    // token is dealt with (and confirmed) before the app starts.
    const qToken = localStorage.getItem('lunaris_token');
    const qHostId = localStorage.getItem('lunaris_auto_launch_host_id');
    const qHostName = localStorage.getItem('lunaris_auto_launch_host_name') || 'Remote Host';
    const qCodecSupport = localStorage.getItem('lunaris_auto_launch_codec_support');

    // Clear temporary auto-launch keys so they don't trigger again on reload
    localStorage.removeItem('lunaris_auto_launch_host_id');
    localStorage.removeItem('lunaris_auto_launch_host_name');
    localStorage.removeItem('lunaris_auto_launch_codec_support');

    const qRes = localStorage.getItem('lunaris_stream_res');
    const qFps = localStorage.getItem('lunaris_stream_fps');
    const qBitrate = localStorage.getItem('lunaris_stream_bitrate');
    const qCodec = localStorage.getItem('lunaris_stream_codec');

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

      // The desktop (app 0, the only one an agent offers): no app picker.
      setSelectedAppId(0);
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
  // Two-factor sign-in: the password was right, the code from the app is next.
  const [mfaStep, setMfaStep] = useState<{ token: string; username: string } | null>(null);
  const [mfaCode, setMfaCode] = useState('');
  const [mfaUseRecovery, setMfaUseRecovery] = useState(false);
  const [mfaRecovery, setMfaRecovery] = useState('');
  const mfaInputRef = useRef<HTMLInputElement>(null);
  // A new server with no account yet: the setup page instead of sign-in.
  const [setupNeeded, setSetupNeeded] = useState(false);
  useEffect(() => {
    if (token) return;
    let live = true;
    fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/setup`)
      .then(async r => { const d = await r.json().catch(() => null); if (live) setSetupNeeded(parseSetupStatus(r.ok, d)); })
      .catch(() => { if (live) setSetupNeeded(false); });
    return () => { live = false; };
  }, [token]);
  useEffect(() => { if (mfaStep) mfaInputRef.current?.focus(); }, [mfaStep, mfaUseRecovery]);
  const [showAccountSecurity, setShowAccountSecurity] = useState(false);
  const [theme, setTheme] = useState<ThemeChoice>(readThemeChoice);
  useEffect(() => {
    applyTheme(theme);
    try { localStorage.setItem('lunaris_theme', theme); } catch { /* storage unavailable */ }
  }, [theme]);
  // The display's refresh rate, once: the default frame rate while none is
  // chosen (streamPrefs.ts).
  useEffect(() => { measureRefresh(localStorage).catch(() => {}); }, []);
  const [showAbout, setShowAbout] = useState(false);
  const nextTheme: Record<ThemeChoice, ThemeChoice> = { system: 'light', light: 'dark', dark: 'system' };
  const themeLabel: Record<ThemeChoice, string> = { system: 'Theme: system', light: 'Theme: light', dark: 'Theme: dark' };

  // Admin State
  const [userRole, setUserRole] = useState<string | null>(localStorage.getItem('lunaris_role'));
  const [activeTab, setActiveTab] = useState<'devices' | 'users' | 'groups' | 'turn_servers' | 'install_agents' | 'server'>('devices');

  // Dashboard State
  const [hosts, setHosts] = useState<Host[]>([]);
  const [hostsLoading, setHostsLoading] = useState<boolean>(false);
  // File manager open for this host (its own connection, FileManager.tsx).
  const [filesHost, setFilesHost] = useState<Host | null>(null);
  // ... or for this source: a computer of another type, through the host it is reached through.
  const [filesSource, setFilesSource] = useState<Source | null>(null);
  // Agents connect with ws:// to the server's HTTP port, also when this page
  // was opened over https (another port).
  const [agentServerUrl, setAgentServerUrl] = useState<string>(() => `ws://${getBackendHost()}`);
  useEffect(() => {
    const { http } = getBackendProtocol();
    if (http !== 'https:') return;
    fetch(`${http}//${getBackendHost()}/api/server/ports`)
      .then(r => (r.ok ? r.json() : null))
      .then(p => {
        if (p && typeof p.http === 'number') {
          const hostname = getBackendHost().replace(/:\d+$/, '');
          setAgentServerUrl(`ws://${hostname}:${p.http}`);
        }
      })
      .catch(() => {});
  }, []);
  const [hostsError, setHostsError] = useState<string | null>(null);
  const [selectedHost, setSelectedHost] = useState<Host | null>(null);
  // Computers of other types (sources), each reached through a Lunaris host.
  const [sources, setSources] = useState<Source[]>([]);
  const [selectedSource, setSelectedSource] = useState<Source | null>(null);
  // One card for each computer: the way to connect used last for each card
  // (kept in this browser), the one picked on a card since the page opened
  // (it stays picked while it is offline, to edit or remove it), and the
  // card whose "choose how to connect" list is open.
  const [cardMethods, setCardMethods] = useState<Record<string, string>>(() => loadMethods(localStorage));
  const [pickedMethods, setPickedMethods] = useState<Record<string, string>>({});
  const [chooserCard, setChooserCard] = useState<string | null>(null);
  const [connectorHosts, setConnectorHosts] = useState<ConnectorHost[]>([]);
  const [agentToken, setAgentToken] = useState<string | null>(null);
  // The server only gives the connection token to administrators.
  const [agentTokenDenied, setAgentTokenDenied] = useState<boolean>(false);


  // Application view navigation states
  const [viewingHost, setViewingHost] = useState<Host | null>(null);
  const [viewingApps, setViewingApps] = useState<{ id: number; title: string; icon_base64?: string | null }[] | null>(null);
  const [viewingAppsLoading, setViewingAppsLoading] = useState<boolean>(false);
  const [viewingAppsError, setViewingAppsError] = useState<string | null>(null);
  const [selectedAppId, setSelectedAppId] = useState<number | null>(null);
  // The stream settings dialog: of a Lunaris computer, of a computer of
  // another type, or this browser's defaults (streamPrefs.ts).
  const [settingsHost, setSettingsHost] = useState<Host | null>(null);
  const [settingsSource, setSettingsSource] = useState<Source | null>(null);
  const [settingsDefaults, setSettingsDefaults] = useState(false);
  const [draftHostSettings, setDraftHostSettings] = useState<HostStreamSettings>(() => loadHostStreamSettings());
  // The encoders the computer whose settings are open reported ([] until it answers).
  const [hostEncoders, setHostEncoders] = useState<string[]>([]);
  const [deleteHostLoading, setDeleteHostLoading] = useState<string | null>(null);
  const [pendingDeleteHost, setPendingDeleteHost] = useState<Host | null>(null);
  // The computer's own settings (its agent's), for administrators.
  const [computerSettingsHost, setComputerSettingsHost] = useState<Host | null>(null);
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
    fetchSources();
  };

  // The computers of other types. A server from before them answers 404:
  // then there are none, and nothing is said about it.
  const fetchSources = async () => {
    if (!token) return;
    try {
      const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/sources`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (!response.ok) {
        if (response.status === 404) setSources([]);
        return;
      }
      const data = await response.json();
      setSources(Array.isArray(data) ? data.filter((s: any) => s && typeof s.id === 'string' && typeof s.host_id === 'string') : []);
    } catch {
      // The host list reports a server that cannot be reached.
    }
  };

  // The types a computer can be added as, per Lunaris computer. 404 (an
  // older server) or nothing: only the "Lunaris" type is offered.
  const fetchConnectors = async () => {
    if (!token) return;
    try {
      const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/connectors`, {
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (!response.ok) { setConnectorHosts([]); return; }
      const data = await response.json();
      const list: ConnectorHost[] = (Array.isArray(data) ? data : [])
        .filter((h: any) => h && typeof h.host_id === 'string' && Array.isArray(h.connectors))
        .map((h: any) => ({
          host_id: h.host_id,
          host_name: String(h.host_name || h.host_id),
          connectors: h.connectors
            .filter((c: any) => c && typeof c.id === 'string')
            .map((c: any) => ({
              id: c.id,
              name: String(c.name || c.id),
              note: typeof c.note === 'string' ? c.note : '',
              fields: (Array.isArray(c.fields) ? c.fields : []).filter((f: any) => f && typeof f.key === 'string'),
            })),
        }))
        .filter((h: ConnectorHost) => h.connectors.length > 0);
      setConnectorHosts(list);
    } catch {
      setConnectorHosts([]);
    }
  };





  useEffect(() => {
    if (token) {
      fetchHosts();
      fetchConnectors();
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
        setAgentTokenDenied(false);
      } else if (response.status === 403 || response.status === 401) {
        setAgentToken(null);
        setAgentTokenDenied(true);
      }
    } catch (err) {
      console.error("Failed to fetch agent token:", err);
    }
  };

  // Only for administrators (the role is known once /api/auth/me answered).
  useEffect(() => {
    if (token && userRole === 'admin') fetchAgentToken();
  }, [token, userRole]);

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

      const data = await response.json().catch(() => ({}));
      const answer = parseLoginAnswer(response.ok, data);
      if (answer.kind === 'mfa') {
        // No session yet: ask for the code from the authenticator app.
        setMfaStep({ token: answer.mfaToken, username: answer.username || authUsername });
        setMfaCode('');
        setMfaRecovery('');
        setMfaUseRecovery(false);
        setAuthPassword('');
      } else {
        finishSignIn(answer);
      }
    } catch (err) {
      setAuthError("Connection to authentication server failed");
    } finally {
      setAuthLoading(false);
    }
  };

  /** A sign-in answer: keep the session, or show the error. */
  const finishSignIn = (answer: LoginAnswer) => {
    if (answer.kind === 'session') {
      localStorage.setItem('lunaris_token', answer.token);
      localStorage.setItem('lunaris_username', answer.username);
      localStorage.setItem('lunaris_server_host', authServerHost);
      if (answer.role) {
        localStorage.setItem('lunaris_role', answer.role);
        setUserRole(answer.role);
      }
      setToken(answer.token);
      setUsername(answer.username);
      // Clear fields
      setAuthUsername('');
      setAuthPassword('');
      setMfaStep(null);
      setMfaCode('');
      setMfaRecovery('');
    } else if (answer.kind === 'error') {
      setAuthError(t(answer.error));
    }
  };

  // The second step: the code from the app, or a recovery code.
  const handleMfa = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!mfaStep) return;
    setAuthError(null);
    if (mfaUseRecovery ? !isPlausibleRecoveryCode(mfaRecovery) : !isCompleteTotp(mfaCode)) {
      setAuthError(mfaUseRecovery ? t('Enter one of your recovery codes') : t('Enter the 6-digit code from your app'));
      return;
    }
    setAuthLoading(true);
    try {
      const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/auth/login/mfa`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(mfaUseRecovery
          ? { mfa_token: mfaStep.token, recovery_code: normalizeRecoveryInput(mfaRecovery) }
          : { mfa_token: mfaStep.token, code: mfaCode }),
      });
      const data = await response.json().catch(() => ({}));
      const answer = parseLoginAnswer(response.ok, data, 'Wrong code');
      if (answer.kind === 'error' && answer.error === 'This sign-in has expired. Sign in again.') {
        // Back to the password.
        setMfaStep(null);
      }
      if (answer.kind === 'error') setMfaCode('');
      finishSignIn(answer);
    } catch {
      setAuthError(t('Connection to authentication server failed'));
    } finally {
      setAuthLoading(false);
    }
  };

  const cancelMfa = () => {
    setMfaStep(null);
    setMfaCode('');
    setMfaRecovery('');
    setMfaUseRecovery(false);
    setAuthError(null);
  };

  const handleLogout = () => {
    localStorage.removeItem('lunaris_token');
    localStorage.removeItem('lunaris_username');
    localStorage.removeItem('lunaris_role');
    setToken(null);
    setUsername(null);
    setUserRole(null);
    setShowAccountSecurity(false);
    setActiveTab('devices');
    setHosts([]);
    setSelectedHost(null);
  };



  const openHostSettings = (host: Host) => {
    setHostEncoders([]);
    setSettingsSource(null);
    setSettingsDefaults(false);
    setSettingsHost(host);
    setDraftHostSettings(loadHostStreamSettings(hostPrefsKey(host.id)));
  };
  const openSourceSettings = (source: Source) => {
    setSettingsHost(null);
    setSettingsDefaults(false);
    setSettingsSource(source);
    setDraftHostSettings(loadHostStreamSettings(sourcePrefsKey(source.host_id, source.id)));
  };
  const openDefaultSettings = () => {
    setHostEncoders([]);
    setSettingsHost(null);
    setSettingsSource(null);
    setSettingsDefaults(true);
    setDraftHostSettings(loadHostStreamSettings(null));
  };
  const closeSettings = () => { setSettingsHost(null); setSettingsSource(null); setSettingsDefaults(false); };
  // Whose settings the dialog edits (null: the defaults).
  const settingsComputer = settingsHost ? hostPrefsKey(settingsHost.id)
    : settingsSource ? sourcePrefsKey(settingsSource.host_id, settingsSource.id) : null;
  const settingsOpen = !!settingsHost || !!settingsSource || settingsDefaults;
  // Whether the draft differs from the defaults: the computer has settings of its own.
  const settingsOwn = !!settingsComputer && (() => {
    const own = differences(loadDefaults(localStorage), picturePrefs(draftHostSettings));
    // A source's 'default' display is not one of its own.
    if (settingsSource && own.display === 'default') delete own.display;
    return Object.keys(own).length > 0;
  })();
  // "Use defaults": the draft takes the defaults (saved with Save).
  const useDefaultSettings = () => {
    const defaults = prefsFor(localStorage, null);
    setDraftHostSettings(current => ({ ...current, ...defaults, display: settingsSource ? 'default' : defaults.display }));
  };

  // What the settings dialog shows about its computer, as the list has it now.
  const settingsHostNow = settingsHost ? hosts.find(h => h.id === settingsHost.id) || settingsHost : null;
  const settingsHostInfo: { label: string; value: string }[] = settingsHostNow ? [
    { label: t('Name'), value: settingsHostNow.name },
    ...(settingsHostNow.agent_name ? [{ label: t('Name it reports'), value: settingsHostNow.agent_name }] : []),
    { label: t('Status'), value: t(settingsHostNow.status) },
    ...((settingsHostNow.addresses || []).length > 0 ? [{ label: t('IP addresses'), value: (settingsHostNow.addresses || []).join(', ') }] : []),
    ...(settingsHostNow.address ? [{ label: t('Added by address'), value: shortAddress(settingsHostNow.address) }] : []),
  ] : [];

  const applyHostSettings = () => {
    saveHostStreamSettings(draftHostSettings, settingsComputer);
    closeSettings();
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
        if (Array.isArray(payload?.encoders)) {
          setHostEncoders(payload.encoders.filter((e: unknown): e is string => typeof e === 'string'));
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

  // Add a computer by address (admins): the server connects to its agent.
  const [addHostOpen, setAddHostOpen] = useState(false);
  const [addHostAddress, setAddHostAddress] = useState('');
  const [addHostPassword, setAddHostPassword] = useState('');
  const [addHostName, setAddHostName] = useState('');
  const [addHostBusy, setAddHostBusy] = useState(false);
  const [addHostError, setAddHostError] = useState('');

  // The same dialog adds a computer of another type ('' = Lunaris, else a
  // connector id) and edits one (`editSource`).
  const [addType, setAddType] = useState('');
  const [sourceName, setSourceName] = useState('');
  const [sourceHostId, setSourceHostId] = useState('');
  const [sourceValues, setSourceValues] = useState<Record<string, string>>({});
  const [editSource, setEditSource] = useState<Source | null>(null);
  // The set-up of a computer being added (a pairing): what its Lunaris
  // computer wants shown (a PIN to type at the other computer), until it is
  // done; `failed` is why it did not work.
  const [sourceSetup, setSourceSetup] = useState<{ hostId: string; setup: string; message: string; code: string; failed: string } | null>(null);
  // "Add a connection method": the card the new source is to be one
  // computer with. `lone` is that card's only source while it has no group
  // yet (it gets one first).
  const [addTo, setAddTo] = useState<{ title: string; group: string; lone?: Source } | null>(null);
  // Edit: the card the source is one computer with ('' = a computer of its
  // own), as it was when the dialog opened and as chosen.
  const [editSame, setEditSame] = useState('');
  const [editSameWas, setEditSameWas] = useState('');
  const [pendingDeleteSource, setPendingDeleteSource] = useState<Source | null>(null);
  const [deleteSourceBusy, setDeleteSourceBusy] = useState(false);
  const [deleteSourceError, setDeleteSourceError] = useState('');

  // One entry per connector id, whichever computers have it.
  const connectorTypes: Connector[] = [];
  connectorHosts.forEach(h => h.connectors.forEach(c => {
    if (!connectorTypes.some(x => x.id === c.id)) connectorTypes.push(c);
  }));
  // One card for each computer (computerCards.ts). A source's method is
  // named by its type, in the person's language.
  const uiLang = getLang();
  // The server gateway is no computer to connect to: its computers have
  // cards of their own, and it is a choice when one is added.
  const computerHosts = useMemo(() => hosts.filter(h => !h.gateway), [hosts]);
  const cards: ComputerCard[] = useMemo(
    () => buildCards(computerHosts, sources, (source) => typeShortName(source.connector, t(source.connector_name || source.connector))),
    // (The names are in the person's language.)
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [computerHosts, sources, uiLang],
  );
  // Finding a computer (computerCards.ts, as the desktop app does): the
  // search, the types among the cards with the one filtered by, and grid or
  // list. The view and the type are remembered in this browser.
  const [computerSearch, setComputerSearch] = useState('');
  const [computerView, setComputerView] = useState<ComputerView>(() => loadView(browserStore()));
  const [typeChoice, setTypeChoice] = useState<string>(() => loadTypeFilter(browserStore()));
  const cardTypeList = useMemo(
    () => typesOf(cards, (id, source) => (id === 'lunaris' ? 'Lunaris' : typeShortName(id, t(source?.connector_name || id)))),
    [cards],
  );
  const typeFilter = effectiveType(cardTypeList, typeChoice);
  // Favourites and the computers used last, for this account on this server (favourites.ts).
  const account = accountKey(username || '', localStorage.getItem('lunaris_server_host') || getCurrentBrowserServerUrl());
  const loadMarks = (forAccount: string) => ({
    account: forAccount,
    favourites: loadFavourites(browserStore(), forAccount),
    recent: loadRecent(browserStore(), forAccount),
  });
  const [marksState, setMarks] = useState(() => loadMarks(account));
  // Another account or server: its own lists.
  const marks = marksState.account === account ? marksState : loadMarks(account);
  const { favourites, recent } = marks;
  const [cardShow, setCardShow] = useState<CardShow>(() => loadShow(browserStore()));
  const chooseShow = (show: CardShow) => { setCardShow(show); saveShow(browserStore(), show); };
  const toggleFav = (cardKey: string) => setMarks({ ...marks, favourites: toggleFavourite(browserStore(), account, cardKey) });
  const noteUsed = (cardKey: string) => setMarks({ ...marks, recent: recordRecent(browserStore(), account, cardKey) });
  const shownCards = useMemo(
    () => arrangeCards(filterCards(cards, computerSearch, typeFilter), cardShow, favourites, recent),
    [cards, computerSearch, typeFilter, cardShow, favourites, recent],
  );
  const chooseView = (view: ComputerView) => { setComputerView(view); saveView(browserStore(), view); };
  const chooseType = (type: string) => { setTypeChoice(type); saveTypeFilter(browserStore(), type); };
  const clearFinding = () => { setComputerSearch(''); chooseType(''); chooseShow('all'); };
  const methodOf = (card: ComputerCard): ComputerMethod | undefined =>
    card.methods.find(m => m.key === pickedMethods[card.key]) || currentMethod(card.methods, cardMethods[card.key]);
  const pickMethod = (card: ComputerCard, method: ComputerMethod) => {
    setPickedMethods(current => ({ ...current, [card.key]: method.key }));
    setCardMethods(current => rememberMethod(localStorage, current, card.key, method.key));
  };
  const connectWith = (card: ComputerCard, method: ComputerMethod) => {
    if (!method.online) return;
    pickMethod(card, method);
    setChooserCard(null);
    noteUsed(card.key);
    if (method.host) connectToHost(method.host);
    else if (method.source) setSelectedSource(method.source);
  };
  // The open list closes at Escape and at a click anywhere else.
  useEffect(() => {
    if (!chooserCard) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setChooserCard(null); };
    const onDown = (e: MouseEvent) => {
      if (!(e.target instanceof Element) || !e.target.closest('.method-chooser, .connect-more')) setChooserCard(null);
    };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onDown);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onDown);
    };
  }, [chooserCard]);
  // A card's menu (administrators): closes at Escape and at a click elsewhere.
  const [menuCard, setMenuCard] = useState<string | null>(null);
  useEffect(() => {
    if (!menuCard) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setMenuCard(null); };
    const onDown = (e: MouseEvent) => {
      if (!(e.target instanceof Element) || !e.target.closest('.card-menu, .card-menu-btn')) setMenuCard(null);
    };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onDown);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onDown);
    };
  }, [menuCard]);

  // The other computers a source being edited can be said to be.
  const editOwnCard = editSource ? cards.find(card => card.methods.some(m => m.source?.id === editSource.id && m.source.host_id === editSource.host_id)) : undefined;
  const sameChoices = editSource
    ? cards.filter(card => !(card.host && (isPlaceholderHost(card.host) || !card.group)) && !(card === editOwnCard && card.methods.length === 1))
    : [];
  const deleteSourceCard = pendingDeleteSource
    ? cards.find(card => card.methods.some(m => m.source?.id === pendingDeleteSource.id && m.source.host_id === pendingDeleteSource.host_id))
    : undefined;

  const dialogConnectorId = editSource ? editSource.connector : addType;
  // The Lunaris computers that can reach a computer of the chosen type.
  const dialogHosts = dialogConnectorId
    ? connectorHosts.filter(h => h.connectors.some(c => c.id === dialogConnectorId))
    : [];
  const dialogHostId = editSource ? editSource.host_id : sourceHostId;
  // The form of the chosen type, as the chosen Lunaris computer describes it
  // (an edit of a computer whose Lunaris computer is not listed uses another
  // computer's description of the same type, if there is one).
  const dialogConnector: Connector | null = dialogConnectorId
    ? (connectorHosts.find(h => h.host_id === dialogHostId)?.connectors.find(c => c.id === dialogConnectorId)
      || (editSource ? connectorTypes.find(c => c.id === dialogConnectorId) : null)
      || null)
    : null;

  // The types the dialog offers ('' is Lunaris; not when a connection method
  // is being added: a Lunaris computer is a card of its own), and the chosen one.
  const dialogTypes: { id: string; name: string; note: string }[] = (addTo ? [] : [{
    id: '',
    name: 'Lunaris',
    note: 'The server connects to lunaris-agent on that computer (direct access on). `lunaris-agent status --show-password` there shows its addresses and password.',
  }]).concat(connectorTypes.map(c => ({ id: c.id, name: c.name, note: c.note || '' })));
  const dialogType = dialogTypes.find(type => type.id === addType);
  // The name and the Lunaris computer to go through share a row when there
  // are one or two of those to choose from.
  const throughBesideName = dialogHosts.length <= 2;
  // The types as a list beside the form (a wide window) or above it (a narrow
  // one): when a computer is being added and there is more than one type.
  const hasTypeRail = !sourceSetup && !editSource && dialogTypes.length > 1;

  const defaultsOf = (connector: Connector | null | undefined): Record<string, string> => {
    const values: Record<string, string> = {};
    (connector?.fields || []).forEach(f => { values[f.key] = f.default || ''; });
    return values;
  };

  // A long form scrolls: what the server answered must not stay out of sight.
  const addHostErrorRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (addHostError) addHostErrorRef.current?.scrollIntoView({ block: 'nearest' });
  }, [addHostError]);

  const openAddHost = () => {
    setAddHostAddress('');
    setAddHostPassword('');
    setAddHostName('');
    setAddHostError('');
    setAddType('');
    setAddTo(null);
    setEditSource(null);
    setSourceName('');
    setSourceHostId('');
    setSourceValues({});
    setSourceSetup(null);
    setAddHostOpen(true);
    fetchConnectors();
  };

  const chooseAddType = (type: string) => {
    setAddType(type);
    setAddHostError('');
    const first = type ? connectorHosts.find(h => h.connectors.some(c => c.id === type)) : undefined;
    setSourceHostId(first?.host_id || '');
    setSourceValues(defaultsOf(first?.connectors.find(c => c.id === type)));
  };

  const chooseSourceHost = (hostId: string) => {
    setSourceHostId(hostId);
    // Another computer may describe the type with other fields: keep what
    // was typed for the fields both have.
    const next = defaultsOf(connectorHosts.find(h => h.host_id === hostId)?.connectors.find(c => c.id === addType));
    Object.keys(next).forEach(k => { if (sourceValues[k] !== undefined && sourceValues[k] !== '') next[k] = sourceValues[k]; });
    setSourceValues(next);
  };

  // "Add a connection method" on a card: the same dialog, for a computer of
  // another type that is this card's computer.
  const openAddMethod = (card: ComputerCard) => {
    const first = connectorTypes[0];
    const through = first ? connectorHosts.find(h => h.connectors.some(c => c.id === first.id)) : undefined;
    setAddHostAddress('');
    setAddHostPassword('');
    setAddHostError('');
    setEditSource(null);
    setSourceSetup(null);
    setAddTo({ title: card.title, group: card.group, lone: card.group ? undefined : card.methods[0]?.source });
    setAddType(first?.id || '');
    setSourceHostId(through?.host_id || '');
    setSourceValues(defaultsOf(through?.connectors.find(c => c.id === first?.id)));
    setSourceName(card.title);
    setAddHostOpen(true);
    fetchConnectors();
  };

  const openEditSource = (source: Source) => {
    setAddHostError('');
    setAddTo(null);
    const own = cards.find(card => card.methods.some(m => m.source?.id === source.id && m.source.host_id === source.host_id));
    const same = own && own.methods.length > 1 ? own.key : '';
    setEditSame(same);
    setEditSameWas(same);
    setEditSource(source);
    setSourceName(source.name);
    // The fields as stored, except the secrets (the server sends no
    // password back, and only to administrators the rest): a secret starts
    // empty, and an empty field keeps what is stored.
    const stored: Record<string, string> = {};
    Object.entries(source.values || {}).forEach(([key, value]) => { if (typeof value === 'string') stored[key] = value; });
    setSourceValues(stored);
    setAddHostOpen(true);
    fetchConnectors();
  };

  // What the server said about a refused write: JSON {"error"} or, for a
  // non-admin, a plain line of text.
  const writeErrorText = async (response: Response, fallback: string): Promise<string> => {
    const text = (await response.text().catch(() => '')).trim();
    try {
      const body = JSON.parse(text);
      if (body && typeof body.error === 'string' && body.error) return t(body.error);
    } catch { /* not JSON */ }
    if (text && text.length <= 200 && !text.startsWith('<')) return t(text);
    return fallback;
  };

  const submitSource = async () => {
    if (!token || addHostBusy) return;
    const name = sourceName.trim();
    if (!name) { setAddHostError(t('Enter a name for the computer')); return; }
    if (!editSource && !dialogHosts.some(h => h.host_id === sourceHostId)) { setAddHostError(t('Choose the Lunaris computer to go through')); return; }
    const values: Record<string, string> = {};
    for (const field of dialogConnector?.fields || []) {
      const raw = sourceValues[field.key] ?? '';
      const value = field.kind === 'secret' ? raw : raw.trim();
      if (!value) {
        if (field.required && !editSource) { setAddHostError(t('%1 is required', t(field.label))); return; }
        // An edit that showed the stored values: an optional field emptied
        // is sent empty, which clears it. Otherwise (a secret, a required
        // field, or nothing was shown) an empty field keeps what is stored.
        if (editSource?.values && field.kind !== 'secret' && !field.required && (editSource.values[field.key] ?? '') !== '') values[field.key] = '';
        continue;
      }
      if (field.kind === 'port') {
        const port = Number(value);
        if (!/^\d+$/.test(value) || port < 1 || port > 65535) {
          setAddHostError(t('%1 must be a number from 1 to 65535', t(field.label)));
          return;
        }
      }
      values[field.key] = value;
    }
    setAddHostBusy(true);
    setAddHostError('');
    try {
      // Which computer it is. Nothing said: adding lets its Lunaris computer
      // group it with one at the same address, and an edit keeps its group.
      let group: string | undefined;
      if (editSource) {
        if (editSame !== editSameWas) {
          const target = editSame ? cards.find(card => card.key === editSame) : undefined;
          group = target ? await groupOfCard(target.group, target.group ? undefined : target.methods[0]?.source) : '';
          if (group === undefined) return;
        }
      } else if (addTo) {
        group = await groupOfCard(addTo.group, addTo.lone);
        if (group === undefined) return;
        // Not made a second time when the form is sent again.
        if (group !== addTo.group) setAddTo({ title: addTo.title, group });
      }
      const said = group === undefined ? {} : { group };
      const base = `${getBackendProtocol().http}//${getBackendHost()}/api/hosts/${encodeURIComponent(editSource ? editSource.host_id : sourceHostId)}/sources`;
      const response = await fetch(editSource ? `${base}/${encodeURIComponent(editSource.id)}` : base, {
        method: editSource ? 'PUT' : 'POST',
        headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify(editSource ? { name, values, ...said } : { name, connector: addType, values, ...said }),
      });
      if (response.status === 202 && !editSource) {
        // Its type has a set-up: the Lunaris computer says what to show, and
        // is asked about it until it is done (below).
        const body = await response.json().catch(() => null);
        if (body && typeof body.setup === 'string' && body.setup) {
          setSourceSetup({ hostId: sourceHostId, setup: body.setup, message: textOf(body.message), code: textOf(body.code), failed: '' });
        } else {
          setAddHostError(t('Could not add the computer'));
        }
      } else if (response.ok) {
        setAddHostOpen(false);
        setEditSource(null);
        fetchSources();
      } else if (response.status === 401) {
        handleLogout();
      } else {
        setAddHostError(await writeErrorText(response, editSource ? t('Could not save the computer') : t('Could not add the computer')));
      }
    } catch {
      setAddHostError(t('Cannot reach the server'));
    } finally {
      setAddHostBusy(false);
    }
  };

  // The group of a card that another source is to join. A card that has
  // none yet is one source alone (`lone`): it is given a new group first, by
  // an edit that changes nothing else. `undefined` when that did not work
  // (the reason is shown).
  const groupOfCard = async (group: string, lone?: Source): Promise<string | undefined> => {
    if (group) return group;
    if (!lone) { setAddHostError(t('Could not save the computer')); return undefined; }
    const made = newGroup();
    const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/hosts/${encodeURIComponent(lone.host_id)}/sources/${encodeURIComponent(lone.id)}`, {
      method: 'PUT',
      headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: lone.name, values: {}, group: made }),
    });
    if (response.status === 401) { handleLogout(); return undefined; }
    if (!response.ok) { setAddHostError(await writeErrorText(response, t('Could not save the computer'))); return undefined; }
    fetchSources();
    return made;
  };

  const textOf = (value: unknown): string => (typeof value === 'string' ? value : '');
  const sourceSetupUrl = (hostId: string, setup: string) =>
    `${getBackendProtocol().http}//${getBackendHost()}/api/hosts/${encodeURIComponent(hostId)}/sources/setup/${encodeURIComponent(setup)}`;

  // While a set-up waits for the person it is asked about once a second:
  // done closes the dialog (the new card is listed), failed shows why.
  const setupHostId = sourceSetup?.hostId || '';
  const setupId = sourceSetup && !sourceSetup.failed ? sourceSetup.setup : '';
  useEffect(() => {
    if (!token || !setupId) return;
    let stopped = false;
    let asking = false;
    let misses = 0;
    const fail = (text: string) => setSourceSetup(current => (current && current.setup === setupId ? { ...current, failed: text } : current));
    const ask = async () => {
      if (stopped || asking) return;
      asking = true;
      try {
        const response = await fetch(sourceSetupUrl(setupHostId, setupId), { headers: { 'Authorization': `Bearer ${token}` } });
        if (stopped) return;
        if (response.status === 401) {
          handleLogout();
        } else if (response.ok) {
          misses = 0;
          const body = await response.json().catch(() => null);
          if (stopped || !body) return;
          if (body.state === 'done') {
            setSourceSetup(null);
            setAddHostOpen(false);
            fetchSources();
          } else if (body.state === 'failed') {
            fail(textOf(body.error) ? t(textOf(body.error)) : t('Could not add the computer'));
          } else if (body.state === 'pending') {
            const message = textOf(body.message);
            const code = textOf(body.code);
            setSourceSetup(current => (current && current.setup === setupId && (current.message !== message || current.code !== code)
              ? { ...current, message, code }
              : current));
          }
        } else if (response.status === 404) {
          fail(t('That set-up is not there any more. Add the computer again.'));
        } else if (++misses >= 5) {
          // Not at the first hiccup: the computer may be back in a moment.
          fail(await writeErrorText(response, t('Could not add the computer')));
        }
      } catch {
        if (!stopped && ++misses >= 5) fail(t('Cannot reach the server'));
      } finally {
        asking = false;
      }
    };
    const timer = window.setInterval(ask, 1000);
    // The page is left: the set-up is not left waiting on that computer.
    const leave = () => { fetch(sourceSetupUrl(setupHostId, setupId), { method: 'DELETE', headers: { 'Authorization': `Bearer ${token}` }, keepalive: true }).catch(() => {}); };
    window.addEventListener('pagehide', leave);
    return () => {
      stopped = true;
      window.clearInterval(timer);
      window.removeEventListener('pagehide', leave);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token, setupHostId, setupId]);

  // Closing the dialog while a set-up waits stops it on that computer.
  const cancelSourceSetup = () => {
    const current = sourceSetup;
    setSourceSetup(null);
    if (!current || current.failed || !token) return;
    fetch(sourceSetupUrl(current.hostId, current.setup), { method: 'DELETE', headers: { 'Authorization': `Bearer ${token}` }, keepalive: true }).catch(() => {});
  };

  const handleDeleteSource = async (source: Source) => {
    if (!token || deleteSourceBusy) return;
    setDeleteSourceBusy(true);
    setDeleteSourceError('');
    try {
      const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/hosts/${encodeURIComponent(source.host_id)}/sources/${encodeURIComponent(source.id)}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${token}` }
      });
      if (response.ok) {
        setPendingDeleteSource(null);
        setSources((current) => current.filter((x) => !(x.id === source.id && x.host_id === source.host_id)));
      } else if (response.status === 401) {
        handleLogout();
      } else {
        setDeleteSourceError(await writeErrorText(response, t('Could not delete the computer')));
      }
    } catch {
      setDeleteSourceError(t('Cannot reach the server'));
    } finally {
      setDeleteSourceBusy(false);
    }
  };

  const submitAddHost = async () => {
    if (!token || addHostBusy) return;
    if (!addHostAddress.trim()) { setAddHostError(t('Enter the address of the computer')); return; }
    setAddHostBusy(true);
    setAddHostError('');
    try {
      const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/hosts/address`, {
        method: 'POST',
        headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ address: addHostAddress.trim(), password: addHostPassword.trim() || null, name: addHostName.trim() || null }),
      });
      if (response.ok) {
        setAddHostOpen(false);
        fetchHosts();
        // The first connection takes a moment.
        window.setTimeout(fetchHosts, 2500);
      } else if (response.status === 401) {
        handleLogout();
      } else {
        const body = await response.json().catch(() => null);
        setAddHostError(body?.error ? t(body.error) : t('Could not add the computer'));
      }
    } catch {
      setAddHostError(t('Cannot reach the server'));
    } finally {
      setAddHostBusy(false);
    }
  };

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

  // Administrators: a computer's name, a computer added by address, its key.
  const [renameHost, setRenameHost] = useState<Host | null>(null);
  const [renameText, setRenameText] = useState('');
  const [editAddressHost, setEditAddressHost] = useState<Host | null>(null);
  const [editAddress, setEditAddress] = useState('');
  const [editAddressPassword, setEditAddressPassword] = useState('');
  const [editAddressName, setEditAddressName] = useState('');
  const [resetKeyHost, setResetKeyHost] = useState<Host | null>(null);
  const [hostDialogBusy, setHostDialogBusy] = useState(false);
  const [hostDialogError, setHostDialogError] = useState('');
  const openRename = (host: Host) => {
    setMenuCard(null);
    setHostDialogError('');
    setRenameText(host.agent_name ? host.name : '');
    setRenameHost(host);
  };
  const openEditAddress = (host: Host) => {
    setMenuCard(null);
    setHostDialogError('');
    setEditAddress(host.address ? shortAddress(host.address) : '');
    setEditAddressPassword('');
    setEditAddressName(host.agent_name || (isPlaceholderHost(host) && host.name !== host.address) ? host.name : '');
    setEditAddressHost(host);
  };
  const openResetKey = (host: Host) => { setMenuCard(null); setHostDialogError(''); setResetKeyHost(host); };
  /** An administrator's change to a computer; true when it was made. */
  const hostAdminRequest = async (method: string, path: string, body?: unknown): Promise<boolean> => {
    if (!token || hostDialogBusy) return false;
    setHostDialogBusy(true);
    setHostDialogError('');
    try {
      const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}${path}`, {
        method,
        headers: { 'Authorization': `Bearer ${token}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (response.status === 401) { handleLogout(); return false; }
      if (!response.ok) { setHostDialogError(await writeErrorText(response, t('Could not save the computer'))); return false; }
      fetchHosts();
      return true;
    } catch {
      setHostDialogError(t('Cannot reach the server'));
      return false;
    } finally {
      setHostDialogBusy(false);
    }
  };
  const submitRename = async () => {
    if (!renameHost) return;
    if (await hostAdminRequest('PUT', `/api/admin/hosts/${encodeURIComponent(renameHost.id)}/name`, { name: renameText.trim() })) setRenameHost(null);
  };
  const submitEditAddress = async () => {
    if (!editAddressHost) return;
    if (!editAddress.trim()) { setHostDialogError(t('Enter the address of the computer')); return; }
    const done = await hostAdminRequest('PUT', `/api/admin/hosts/${encodeURIComponent(editAddressHost.id)}/address`, {
      address: editAddress.trim(),
      password: editAddressPassword.trim() || null,
      name: editAddressName.trim(),
    });
    if (done) {
      setEditAddressHost(null);
      // The new connection takes a moment.
      window.setTimeout(fetchHosts, 2500);
    }
  };
  const submitResetKey = async () => {
    if (!resetKeyHost) return;
    if (await hostAdminRequest('POST', `/api/admin/hosts/${encodeURIComponent(resetKeyHost.id)}/reset-key`)) setResetKeyHost(null);
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

  // A one-time ticket for a lunaris:// link to `hostId`: the link goes on
  // the desktop app's command line, so it never carries the sign-in token.
  // The app trades the ticket (good for a minute, once) for a token that
  // opens only that computer. Null when the server gives none.
  const requestLinkTicket = async (hostId: string, sourceId?: string): Promise<string | null> => {
    try {
      const response = await fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/links/ticket`, {
        method: 'POST',
        headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify(sourceId ? { host_id: hostId, source_id: sourceId } : { host_id: hostId }),
      });
      if (!response.ok) return null;
      const data = await response.json();
      return typeof data?.ticket === 'string' ? data.ticket : null;
    } catch {
      return null;
    }
  };

  // Remote control in the Lunaris desktop app (lunaris:// link), with this
  // page's stream settings: a Lunaris computer (`host`), or a computer of
  // another type shown through one (`source`).
  const openInDesktopApp = async (target: { host: Host } | { source: Source }) => {
    const hostId = 'host' in target ? target.host.id : target.source.host_id;
    const sourceId = 'source' in target ? target.source.id : undefined;
    const name = 'host' in target ? target.host.name : target.source.name;
    const ticket = await requestLinkTicket(hostId, sourceId);
    if (!ticket) {
      alert(t("Could not prepare the link for the desktop app. Try again."));
      return;
    }
    const protocol = getBackendProtocol();
    const serverHost = getBackendHost();
    // This computer's own settings, else the defaults (streamPrefs.ts).
    const prefs = prefsFor(localStorage, sourceId ? sourcePrefsKey(hostId, sourceId) : hostPrefsKey(hostId));
    const params = new URLSearchParams({
      host_id: hostId,
      server: `${protocol.ws}//${serverHost}`,
      ticket,
      res: resolutionString(prefs.resolution),
      fps: String(prefs.fps),
      bitrate: String(prefs.bitrate),
      codec: linkCodec(prefs),
      mouse_queue_limit: localStorage.getItem('lunaris_mouse_queue_limit') || '256',
      host_name: name,
      app_id: '0',
      encoder: prefs.encoder,
      display: prefs.display,
      input_protocol: localStorage.getItem('lunaris_input_protocol') || 'webrtc',
    });
    if (sourceId) params.set('source_id', sourceId);
    window.location.href = `lunaris://connect?${params.toString()}`;
  };

  // Connect straight to a computer's desktop (app 0, the only one an agent
  // offers) in this browser.
  const connectToHost = (host: Host) => {
    setSelectedAppId(0);
    setSelectedHost(host);
  };

  const closeAddHost = () => { cancelSourceSetup(); setAddHostOpen(false); setEditSource(null); setAddTo(null); };
  const sourceDialog = !!editSource || !!addType;

  const currentViewingHost = viewingHost ? (hosts.find(h => h.id === viewingHost.id) || viewingHost) : null;

  // A computer of another type: a session to the Lunaris computer it is
  // reached through, asking for that source.
  if (token && selectedSource) {
    return (
      <StreamPlayer
        key={`source:${selectedSource.host_id}:${selectedSource.id}`}
        hostId={selectedSource.host_id}
        hostName={selectedSource.name}
        hostAddress={selectedSource.address || ''}
        sourceId={selectedSource.id}
        sourceFiles={sourceHasFiles(selectedSource)}
        token={token}
        serverCodecModeSupport={hosts.find(h => h.id === selectedSource.host_id)?.server_codec_mode_support}
        appId={0}
        onBack={() => {
          setSelectedSource(null);
          fetchHosts();
        }}
      />
    );
  }

  // If inside streaming session, render full screen stream viewer
  if (token && selectedHost) {
    return (
      <StreamPlayer
        hostId={selectedHost.id}
        hostName={selectedHost.name}
        hostAddress={hostAddressText(hosts.find(h => h.id === selectedHost.id) || selectedHost)}
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

      {/* Header bar */}
      <header className="navbar">
        <div className="nav-brand">
          <img className="brand-logo" src="/favicon.svg" alt="" width={28} height={28} />
          <span className="brand-name">Lunaris</span>
        </div>
        <div className="nav-actions">
          <select
            className="lang-select"
            value={getLang()}
            onChange={(e) => setLang(e.target.value as Lang)}
            aria-label={t('Language')}
            title={t('Language')}
          >
            {LANGUAGES.map(l => <option key={l.value} value={l.value}>{l.label}</option>)}
          </select>
          <button
            type="button"
            className="icon-btn"
            onClick={() => setShowAbout(true)}
            title={t('About Lunaris')}
            aria-label={t('About Lunaris')}
          >
            {ICON_INFO}
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={() => setTheme(nextTheme[theme])}
            title={themeLabel[theme]}
            aria-label={themeLabel[theme]}
          >
            {theme === 'light' ? (
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                <circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
              </svg>
            ) : theme === 'dark' ? (
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
              </svg>
            ) : (
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <rect x="2" y="3" width="20" height="14" rx="2" /><path d="M8 21h8M12 17v4" />
              </svg>
            )}
          </button>
          {token && username && (
            <div className="nav-user-panel">
              <div className="nav-user-text">
                <span className="nav-username">{username}</span>
                <span className="nav-server">
                  {localStorage.getItem('lunaris_server_host') || getCurrentBrowserServerUrl()}
                </span>
              </div>
              <button
                type="button"
                className="icon-btn"
                onClick={() => setShowAccountSecurity(true)}
                title={t('Account security')}
                aria-label={t('Account security')}
              >
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10Z" />
                  <rect x="9" y="11" width="6" height="5" rx="1" />
                  <path d="M10 11V9.5a2 2 0 0 1 4 0V11" />
                </svg>
              </button>
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
        {!token && setupNeeded ? (
          <SetupPage
            apiBase={`${getBackendProtocol().http}//${getBackendHost()}`}
            onDone={(answer) => { setSetupNeeded(false); finishSignIn(answer); }}
            onClosed={() => setSetupNeeded(false)}
          />
        ) : !token ? (
          /* Authentication Screen */
          <div className="auth-card-wrapper">
            <div className="auth-card">
              <div className="auth-header">
                <img className="auth-logo" src="/favicon.svg" alt="Lunaris" width={48} height={48} />
                {mfaStep ? (
                  <>
                    <h2>{t('Two-factor sign-in')}</h2>
                    <p>{mfaUseRecovery
                      ? t('Enter one of the recovery codes you saved when you turned on two-factor sign-in.')
                      : t('Enter the 6-digit code from your authenticator app for %1.', mfaStep.username)}</p>
                  </>
                ) : (
                  <>
                    <h2>Welcome Back</h2>
                    <p>Access your remote desktop network</p>
                  </>
                )}
              </div>

              {authError && (
                <div className="auth-error-banner">
                  <span className="error-icon">⚠️</span>
                  <span>{authError}</span>
                </div>
              )}

              {mfaStep ? (
              <form onSubmit={handleMfa} className="auth-form mfa-login-form">
                {mfaUseRecovery ? (
                  <div className="form-group">
                    <label htmlFor="mfaRecovery">{t('Recovery code')}</label>
                    <input
                      ref={mfaInputRef}
                      type="text"
                      id="mfaRecovery"
                      className="mfa-recovery-input"
                      value={mfaRecovery}
                      onChange={(e) => setMfaRecovery(e.target.value)}
                      placeholder="xxxx-xxxx-xxxx"
                      autoComplete="off"
                      autoCapitalize="none"
                      spellCheck={false}
                      disabled={authLoading}
                    />
                  </div>
                ) : (
                  <div className="form-group">
                    <label htmlFor="mfaCode">{t('Code from the app')}</label>
                    <input
                      ref={mfaInputRef}
                      type="text"
                      id="mfaCode"
                      className="mfa-code-input"
                      inputMode="numeric"
                      autoComplete="one-time-code"
                      pattern="[0-9]*"
                      maxLength={7}
                      value={mfaCode}
                      onChange={(e) => setMfaCode(normalizeTotpInput(e.target.value))}
                      placeholder="123456"
                      disabled={authLoading}
                    />
                  </div>
                )}

                <button type="submit" disabled={authLoading} className="btn-primary auth-submit-btn">
                  {authLoading ? <div className="inline-loader"></div> : t('Verify')}
                </button>
                <div className="mfa-login-links">
                  <button type="button" className="btn-link" onClick={() => { setMfaUseRecovery(u => !u); setAuthError(null); }} disabled={authLoading}>
                    {mfaUseRecovery ? t('Use a code from the app') : t('Use a recovery code')}
                  </button>
                  <button type="button" className="btn-link mfa-back-link" onClick={cancelMfa} disabled={authLoading}>
                    {t('Back')}
                  </button>
                </div>
              </form>
              ) : (
              <form onSubmit={handleAuth} className="auth-form">
                <div className="form-group">
                  <label htmlFor="serverHost">Server URL</label>
                  <input
                    type="text"
                    id="serverHost"
                    value={authServerHost}
                    onChange={(e) => setAuthServerHost(e.target.value)}
                    placeholder="e.g. http://localhost:8090"
                    required
                  />
                </div>

                <div className="form-group">
                  <label htmlFor="username">{t('Username')}</label>
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
                  <label htmlFor="password">{t('Password')}</label>
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
              )}
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
                    {t('Devices')}
                  </button>
                  <button
                    className={`nav-tab ${activeTab === 'users' ? 'active' : ''}`}
                    onClick={() => setActiveTab('users')}
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
                      <circle cx="12" cy="7" r="4" />
                    </svg>
                    {t('Users')}
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
                    {t('Groups')}
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
                    {t('TURN Servers')}
                  </button>
                  <button
                    className={`nav-tab ${activeTab === 'install_agents' ? 'active' : ''}`}
                    onClick={() => setActiveTab('install_agents')}
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                      <polyline points="7 10 12 15 17 10" />
                      <line x1="12" y1="15" x2="12" y2="3" />
                    </svg>
                    {t('Install agents')}
                  </button>
                  <button
                    className={`nav-tab ${activeTab === 'server' ? 'active' : ''}`}
                    onClick={() => setActiveTab('server')}
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <rect x="3" y="3" width="18" height="7" rx="1.5" />
                      <rect x="3" y="14" width="18" height="7" rx="1.5" />
                      <path d="M7 6.5h.01M7 17.5h.01" />
                    </svg>
                    {t('Server')}
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
                          <span className="breadcrumb-item">{t('Device Directory')}</span>
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
                                      onClick={async () => {
                                        const protocol = getBackendProtocol().http;
                                        const serverHost = getBackendHost();
                                        const serverUrl = `${protocol}//${serverHost}`;
                                        const hostToUse = currentViewingHost || viewingHost;

                                        if (!hostToUse) return;
                                        const prefs = prefsFor(localStorage, hostPrefsKey(hostToUse.id));
                                        const resStr = resolutionString(prefs.resolution);
                                        const { fps, bitrate, codec, encoder, display } = prefs;
                                        const mouseQueueLimit = localStorage.getItem('lunaris_mouse_queue_limit') || '256';
                                        const inputProtocol = localStorage.getItem('lunaris_input_protocol') || 'webrtc';

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
                                            inputProtocol
                                          }).catch((err: any) => {
                                            console.error("Failed to launch native client:", err);
                                            alert("Failed to launch native client: " + err);
                                          });
                                        } else {
                                          const wsProtocol = getBackendProtocol().ws;
                                          const wsServerUrl = `${wsProtocol}//${serverHost}`;
                                          const ticket = await requestLinkTicket(hostToUse.id);
                                          if (!ticket) {
                                            alert(t("Could not prepare the link for the desktop app. Try again."));
                                            return;
                                          }
                                          window.location.href = `lunaris://connect?${new URLSearchParams({
                                            host_id: hostToUse.id,
                                            server: wsServerUrl,
                                            ticket,
                                            res: resStr,
                                            fps: String(fps),
                                            bitrate: String(bitrate),
                                            codec: linkCodec(prefs),
                                            mouse_queue_limit: String(mouseQueueLimit),
                                            host_name: hostToUse.name,
                                            app_id: String(app.id),
                                            encoder: String(encoder),
                                            display: String(display),
                                            input_protocol: String(inputProtocol),
                                          }).toString()}`;
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
                          <h1 className="section-title">{t('Device Directory')}</h1>
                          <p className="section-subtitle">{t('Manage and connect to active remote agent streams')}</p>
                        </div>
                        <div style={{ display: 'flex', gap: '0.75rem' }}>
                          {userRole === 'admin' && (
                            <button onClick={openAddHost} className="btn-primary refresh-btn" title={t('Add a computer by its address')}>
                              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                                <path d="M12 5v14M5 12h14" />
                              </svg>
                              {t('Add computer')}
                            </button>
                          )}
                          <button
                            type="button"
                            onClick={openDefaultSettings}
                            className="btn-secondary refresh-btn"
                            title={t('Stream settings for every computer that has none of its own')}
                          >
                            {ICON_GEAR}
                            {t('Default stream settings')}
                          </button>
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
                            {t('Sync Devices')}
                          </button>
                        </div>
                      </div>

                      {/* Agent setup credentials — only needed when adding a host, so it is
                          collapsed unless there are no hosts yet. */}
                      <details className="setup-panel" open={!hostsLoading && computerHosts.length === 0}>
                        <summary>
                          <span className="setup-panel-title">
                            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                              <path d="M12 5v14M5 12h14" />
                            </svg>
                            {t('Add a host agent')}
                          </span>
                          <span className="setup-panel-hint">{t('Server URL and connection token')}</span>
                        </summary>
                        <div className="setup-panel-grid">
                          <div className="setup-field">
                            <span className="setup-field-label">Signaling server URL</span>
                            <div className="setup-field-row">
                              <code>{agentServerUrl}</code>
                              <button
                                type="button"
                                className="icon-btn"
                                onClick={() => {
                                  navigator.clipboard.writeText(agentServerUrl);
                                  alert("Server URL copied!");
                                }}
                                title="Copy server URL"
                                aria-label="Copy server URL"
                              >
                                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                                  <rect x="9" y="9" width="13" height="13" rx="2" /><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                                </svg>
                              </button>
                            </div>
                          </div>
                          <div className="setup-field">
                            <span className="setup-field-label">Agent connection token</span>
                            <div className="setup-field-row">
                              <code>{agentToken || ((userRole && userRole !== 'admin') || agentTokenDenied ? t("Ask an administrator for the connection token.") : "Loading...")}</code>
                              <button
                                type="button"
                                className="icon-btn"
                                onClick={() => {
                                  if (agentToken) {
                                    navigator.clipboard.writeText(agentToken);
                                    alert("Connection Token copied!");
                                  }
                                }}
                                disabled={!agentToken}
                                title="Copy connection token"
                                aria-label="Copy connection token"
                              >
                                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                                  <rect x="9" y="9" width="13" height="13" rx="2" /><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                                </svg>
                              </button>
                            </div>
                          </div>
                        </div>
                        {userRole === 'admin' && (
                          <div className="setup-panel-install">
                            <span>{t('Installing many computers? Create an install token and paste one command on each.')}</span>
                            <button type="button" className="btn-secondary" onClick={() => setActiveTab('install_agents')}>{t('Install agents')}</button>
                          </div>
                        )}
                      </details>

                      {hostsError && (
                        <div className="error-card">
                          <div className="error-title">⚠️ Sync Error</div>
                          <div className="error-desc">{hostsError}</div>
                          <button onClick={fetchHosts} className="btn-secondary">Retry Sync</button>
                        </div>
                      )}

                      {hostsLoading && computerHosts.length === 0 && sources.length === 0 ? (
                        <div className="loading-card">
                          <div className="tech-loader"></div>
                          <div>Syncing active host agent list...</div>
                        </div>
                      ) : computerHosts.length === 0 && sources.length === 0 ? (
                        <div className="empty-card">
                          <div className="empty-icon">🖥️</div>
                          <h3>{t('No agents connected')}</h3>
                          <p>{t('There are no host agents registered with the server.')}</p>
                          <p className="empty-hint">{t('Agents will appear here automatically once they connect.')}</p>
                        </div>
                      ) : (
                        <>
                        {/* Search, the type of computer, how many, grid or list. */}
                        <div className="computer-finder">
                          <div className="finder-search">
                            {cardIcon(<><circle cx="11" cy="11" r="7" /><path d="M20 20l-4-4" /></>, 15)}
                            <input
                              type="search"
                              value={computerSearch}
                              placeholder={t('Search by name or address')}
                              aria-label={t('Search computers')}
                              onChange={(e) => setComputerSearch(e.target.value)}
                              onKeyDown={(e) => {
                                if (e.key === 'Escape' && computerSearch) { e.preventDefault(); e.stopPropagation(); setComputerSearch(''); }
                              }}
                            />
                            {computerSearch && (
                              <button type="button" className="finder-clear" title={t('Clear search')} aria-label={t('Clear search')} onClick={() => setComputerSearch('')}>
                                {cardIcon(<path d="M6 6l12 12M18 6L6 18" />, 14)}
                              </button>
                            )}
                          </div>
                          {cardTypeList.length > 1 && (
                            <div className="type-chips" role="radiogroup" aria-label={t('Type of computer')}>
                              {[{ id: '', name: t('All') }, ...cardTypeList].map((type) => (
                                <button
                                  key={type.id || '*'}
                                  type="button"
                                  role="radio"
                                  aria-checked={type.id === typeFilter}
                                  className={`type-chip${type.id === typeFilter ? ' selected' : ''}`}
                                  onClick={() => chooseType(type.id)}
                                >
                                  {type.id && <TypeMark id={type.id} name={type.name} small />}
                                  <span>{type.name}</span>
                                </button>
                              ))}
                            </div>
                          )}
                          <div className="show-chips" role="radiogroup" aria-label={t('Show')}>
                            {([['all', t('All computers')], ['favourites', t('Favourites')], ['recent', t('Recent')]] as const).map(([value, label]) => (
                              <button
                                key={value}
                                type="button"
                                role="radio"
                                aria-checked={cardShow === value}
                                className={`type-chip${cardShow === value ? ' selected' : ''}`}
                                onClick={() => chooseShow(value)}
                              >
                                {value === 'favourites' && <span className="show-chip-star" aria-hidden="true">{cardIcon(<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />, 13)}</span>}
                                <span>{label}</span>
                              </button>
                            ))}
                          </div>
                          <div className="finder-end">
                            <span className="computer-count" aria-live="polite">
                              {shownCards.length === cards.length
                                ? (cards.length === 1 ? t('1 computer') : t('%1 computers', cards.length))
                                : t('%1 of %2 computers', shownCards.length, cards.length)}
                            </span>
                            <div className="view-toggle" role="radiogroup" aria-label={t('View')}>
                              {(['grid', 'list'] as const).map((view) => (
                                <button
                                  key={view}
                                  type="button"
                                  role="radio"
                                  aria-checked={computerView === view}
                                  className={computerView === view ? 'selected' : ''}
                                  title={t(view === 'grid' ? 'Grid' : 'List')}
                                  aria-label={t(view === 'grid' ? 'Grid' : 'List')}
                                  onClick={() => chooseView(view)}
                                >
                                  {view === 'grid'
                                    ? cardIcon(<><rect x="4" y="4" width="7" height="7" rx="1" /><rect x="13" y="4" width="7" height="7" rx="1" /><rect x="4" y="13" width="7" height="7" rx="1" /><rect x="13" y="13" width="7" height="7" rx="1" /></>)
                                    : cardIcon(<><path d="M9 6h11M9 12h11M9 18h11" /><path d="M4.5 6h.01M4.5 12h.01M4.5 18h.01" /></>)}
                                </button>
                              ))}
                            </div>
                          </div>
                        </div>

                        {shownCards.length === 0 ? (
                          <div className="empty-card no-match">
                            <div className="empty-icon" aria-hidden="true">{cardIcon(<><circle cx="11" cy="11" r="7" /><path d="M20 20l-4-4" /></>, 30)}</div>
                            <h3>{cardShow === 'favourites' && !computerSearch && !typeFilter ? t('No favourites yet')
                              : cardShow === 'recent' && !computerSearch && !typeFilter ? t('No computer used yet')
                              : t('No computer matches')}</h3>
                            <p>{cardShow === 'favourites' && !computerSearch && !typeFilter ? t('Mark a computer with its star to keep it here.')
                              : cardShow === 'recent' && !computerSearch && !typeFilter ? t('The computers you connect to are listed here, the last one first.')
                              : t('Try other words, or show every type.')}</p>
                            <button type="button" className="btn-secondary" onClick={clearFinding}>{t('Clear search and filter')}</button>
                          </div>
                        ) : (
                        <div className={computerView === 'list' ? 'hosts-card-grid hosts-card-list' : 'hosts-card-grid'}>
                          {shownCards.map((card) => {
                            const method = methodOf(card);
                            if (!method) return null;
                            const host = method.host;
                            const source = method.source;
                            const many = card.methods.length > 1;
                            const isOnline = method.online;
                            const status = host ? host.status : (source?.online ? 'Online' : 'Offline');
                            // A computer of another type alone on its card: as it always looked.
                            const lone = !card.host && !many ? source : undefined;
                            const canAddMethod = userRole === 'admin' && connectorTypes.length > 0
                              && (card.host ? !!card.group && !isPlaceholderHost(card.host) : true);
                            const chooserOpen = chooserCard === card.key;
                            return (
                              <div
                                key={card.key}
                                data-card={card.key}
                                className={`host-card ${card.host ? '' : 'source-card '}${status.toLowerCase()} ${isOnline ? 'clickable-host-card' : ''}`}
                                onClick={() => connectWith(card, method)}
                              >
                                {/* (The list's row starts with the type of the way chosen.) */}
                                <span className="host-card-lead"><TypeMark id={methodType(method)} name={method.label} /></span>
                                <div className="host-card-header">
                                  <div className="source-card-title">
                                    <h3 className="host-name" title={card.host?.agent_name ? `${card.title} (${card.host.agent_name})` : undefined}>{card.title}</h3>
                                    {card.host && !isPlaceholderHost(card.host) && (
                                      <span className="host-id">ID: {card.host.id.slice(0, 8)}...</span>
                                    )}
                                    {lone && (
                                      <div className="source-card-sub">
                                        <span className="source-type-badge">{t(lone.connector_name || lone.connector)}</span>
                                        <span className="source-via">{t('via %1', lone.host_name)}</span>
                                      </div>
                                    )}
                                  </div>
                                  <button
                                    type="button"
                                    className={`card-star${favourites.includes(card.key) ? ' on' : ''}`}
                                    aria-pressed={favourites.includes(card.key)}
                                    title={favourites.includes(card.key) ? t('Remove from favourites') : t('Add to favourites')}
                                    aria-label={favourites.includes(card.key) ? t('Remove from favourites') : t('Add to favourites')}
                                    onClick={(e) => { e.stopPropagation(); toggleFav(card.key); }}
                                  >
                                    {ICON_STAR}
                                  </button>
                                </div>

                                {/* The ways to connect to this computer: one is picked. */}
                                {many && (
                                  <div className="method-chips" role="radiogroup" aria-label={t('Ways to connect')} onClick={(e) => e.stopPropagation()}>
                                    {card.methods.map((m) => (
                                      <button
                                        key={m.key}
                                        type="button"
                                        role="radio"
                                        aria-checked={m.key === method.key}
                                        className={`method-chip ${m.online ? 'online' : 'offline'}${m.key === method.key ? ' selected' : ''}`}
                                        title={`${m.source ? t(m.source.connector_name || m.source.connector) : m.label}: ${t(m.online ? 'Online' : 'Offline')}`}
                                        onClick={() => pickMethod(card, m)}
                                      >
                                        <span className="method-chip-dot" aria-hidden="true" />
                                        <TypeMark id={m.source ? m.source.connector : 'lunaris'} name={m.label} small />
                                        <span className="method-chip-label">{m.label}</span>
                                      </button>
                                    ))}
                                  </div>
                                )}

                                <div className="host-card-body">
                                  {host && hostAddressText(host) && (
                                    <div className="host-meta-item">
                                      <span className="meta-label">{t('IP Address')}</span>
                                      <span className="meta-value meta-address" title={hostAddressText(host)}>{hostAddressText(host)}</span>
                                    </div>
                                  )}
                                  {source?.address && (
                                    <div className="host-meta-item">
                                      <span className="meta-label">{t('Address')}</span>
                                      <span className="meta-value meta-address" title={source.address}>{source.address}</span>
                                    </div>
                                  )}
                                  {host?.needs_approval && (
                                    <div className="host-meta-item">
                                      <span className="meta-label">{t('Access')}</span>
                                      <span className="meta-value">{t('Accepted at the computer')}</span>
                                    </div>
                                  )}
                                  {host?.link_error && (
                                    <div className="host-link-error" title={host.link_error}>{t(host.link_error)}</div>
                                  )}
                                  {source && many && (
                                    <div className="host-meta-item">
                                      <span className="meta-label">{t('Through')}</span>
                                      <span className="meta-value method-through">{source.host_name}</span>
                                    </div>
                                  )}
                                  <div className="host-meta-item">
                                    <span className="meta-label">{t('Status')}</span>
                                    <div className="host-status-badge">
                                      <span className={`status-indicator ${status.toLowerCase()}`}></span>
                                      <span className="status-label">{t(status)}</span>
                                    </div>
                                  </div>
                                </div>

                                <div className="host-card-actions" onClick={(e) => e.stopPropagation()}>
                                  {(isOnline || many) && (
                                    <div className="connect-split">
                                      {isOnline && (
                                        <button
                                          type="button"
                                          className="btn-primary host-card-connect"
                                          onClick={() => connectWith(card, method)}
                                        >
                                          {t('Connect')}
                                        </button>
                                      )}
                                      {many && (
                                        <button
                                          type="button"
                                          className={`${isOnline ? 'btn-primary' : 'btn-secondary'} connect-more`}
                                          title={t('Choose how to connect')}
                                          aria-label={t('Choose how to connect')}
                                          aria-haspopup="menu"
                                          aria-expanded={chooserOpen}
                                          onClick={() => setChooserCard(chooserOpen ? null : card.key)}
                                        >
                                          {ICON_CHEVRON}
                                        </button>
                                      )}
                                    </div>
                                  )}
                                  {isOnline && (host || source) && (
                                    <button
                                      type="button"
                                      className="host-action-btn"
                                      title={t('Open in the Lunaris app')}
                                      aria-label={t('Open in the Lunaris app')}
                                      onClick={() => { noteUsed(card.key); openInDesktopApp(host ? { host } : { source: source! }); }}
                                    >
                                      {ICON_APP}
                                    </button>
                                  )}
                                  {isOnline && (host || (source && sourceHasFiles(source))) && (
                                    <button
                                      type="button"
                                      className={`host-action-btn${source ? ' source-files-btn' : ''}`}
                                      title={t('Files')}
                                      aria-label={t('Files')}
                                      onClick={() => { if (host) setFilesHost(host); else if (source) setFilesSource(source); }}
                                    >
                                      {ICON_FILES}
                                    </button>
                                  )}
                                  {(host || source) && (
                                    <button
                                      type="button"
                                      className="host-action-btn"
                                      title={t('Stream settings')}
                                      aria-label={t('Stream settings')}
                                      onClick={() => { if (host) openHostSettings(host); else if (source) openSourceSettings(source); }}
                                    >
                                      {ICON_GEAR}
                                    </button>
                                  )}
                                  {host && userRole === 'admin' && (
                                    <button
                                      type="button"
                                      className="host-action-btn"
                                      title={t('Computer settings')}
                                      aria-label={t('Computer settings')}
                                      onClick={() => setComputerSettingsHost(host)}
                                    >
                                      {ICON_SLIDERS}
                                    </button>
                                  )}
                                  {canAddMethod && (
                                    <button
                                      type="button"
                                      className="host-action-btn add-method-btn"
                                      title={t('Add a connection method')}
                                      aria-label={t('Add a connection method')}
                                      onClick={() => openAddMethod(card)}
                                    >
                                      {ICON_PLUS}
                                    </button>
                                  )}
                                  {source && userRole === 'admin' && (
                                    <button
                                      type="button"
                                      className="host-action-btn"
                                      title={t('Edit')}
                                      aria-label={t('Edit')}
                                      onClick={() => openEditSource(source)}
                                    >
                                      {ICON_EDIT}
                                    </button>
                                  )}
                                  {host && userRole === 'admin' && (
                                    <button
                                      type="button"
                                      className="host-action-btn card-menu-btn"
                                      title={t('More')}
                                      aria-label={t('More')}
                                      aria-haspopup="menu"
                                      aria-expanded={menuCard === card.key}
                                      onClick={() => setMenuCard(menuCard === card.key ? null : card.key)}
                                    >
                                      {ICON_MORE}
                                    </button>
                                  )}
                                  {host && (
                                    <button
                                      type="button"
                                      className="host-action-btn danger"
                                      title={t('Delete')}
                                      aria-label={t('Delete')}
                                      disabled={deleteHostLoading === host.id}
                                      onClick={() => setPendingDeleteHost(host)}
                                    >
                                      {ICON_TRASH}
                                    </button>
                                  )}
                                  {source && userRole === 'admin' && (
                                    <button
                                      type="button"
                                      className="host-action-btn danger"
                                      title={t('Delete')}
                                      aria-label={t('Delete')}
                                      onClick={() => { setDeleteSourceError(''); setPendingDeleteSource(source); }}
                                    >
                                      {ICON_TRASH}
                                    </button>
                                  )}
                                </div>

                                {/* An administrator's changes to the Lunaris computer. */}
                                {host && userRole === 'admin' && menuCard === card.key && (
                                  <div className="method-chooser card-menu" role="menu" aria-label={t('More')} onClick={(e) => e.stopPropagation()}>
                                    <button type="button" role="menuitem" className="method-option" onClick={() => openRename(host)}>
                                      <span className="card-menu-icon">{ICON_EDIT}</span>
                                      <span className="method-option-text">
                                        <span className="method-option-name">{t('Rename')}</span>
                                        <span className="method-option-sub">{t('The name everyone sees in the list')}</span>
                                      </span>
                                    </button>
                                    {host.address && (
                                      <button type="button" role="menuitem" className="method-option" onClick={() => openEditAddress(host)}>
                                        <span className="card-menu-icon">{ICON_APP}</span>
                                        <span className="method-option-text">
                                          <span className="method-option-name">{t('Edit address and password')}</span>
                                          <span className="method-option-sub">{shortAddress(host.address)}</span>
                                        </span>
                                      </button>
                                    )}
                                    {!isPlaceholderHost(host) && !host.address && (
                                      <button type="button" role="menuitem" className="method-option" onClick={() => openResetKey(host)}>
                                        <span className="card-menu-icon">{ICON_KEY}</span>
                                        <span className="method-option-text">
                                          <span className="method-option-name">{t('Reset its key')}</span>
                                          <span className="method-option-sub">{t('After a reinstall, when it can no longer sign in')}</span>
                                        </span>
                                      </button>
                                    )}
                                  </div>
                                )}

                                {/* Connecting another way, right here. */}
                                {many && chooserOpen && (
                                  <div className="method-chooser" role="menu" aria-label={t('Choose how to connect')} onClick={(e) => e.stopPropagation()}>
                                    {card.methods.map((m) => (
                                      <button
                                        key={m.key}
                                        type="button"
                                        role="menuitem"
                                        className={`method-option ${m.online ? 'online' : 'offline'}`}
                                        disabled={!m.online}
                                        onClick={() => connectWith(card, m)}
                                      >
                                        <span className="method-chip-dot" aria-hidden="true" />
                                        <TypeMark id={m.source ? m.source.connector : 'lunaris'} name={m.label} />
                                        <span className="method-option-text">
                                          <span className="method-option-name">{t('Connect with %1', m.label)}</span>
                                          <span className="method-option-sub">
                                            {m.source ? `${t('via %1', m.source.host_name)} · ` : ''}{t(m.host ? m.host.status : (m.online ? 'Online' : 'Offline'))}
                                          </span>
                                        </span>
                                      </button>
                                    ))}
                                  </div>
                                )}
                              </div>
                            );
                          })}
                        </div>
                        )}
                        </>
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
              {activeTab === 'install_agents' && userRole === 'admin' && (
                <AdminInstallAgents token={token!} getBackendHost={getBackendHost} getBackendProtocol={getBackendProtocol} />
              )}
              {activeTab === 'server' && userRole === 'admin' && (
                <AdminServerInfo token={token!} apiBase={() => `${getBackendProtocol().http}//${getBackendHost()}`} />
              )}
            </div>
          </div>
        )}
      {filesHost && token && (
        <FileManager
          hostId={filesHost.id}
          hostName={filesHost.name}
          wsUrl={`${getBackendProtocol().ws}//${getBackendHost()}/ws/client?token=${encodeURIComponent(token)}`}
          onClose={() => setFilesHost(null)}
        />
      )}
      {filesSource && token && (
        <FileManager
          key={`files:${filesSource.host_id}:${filesSource.id}`}
          hostId={filesSource.host_id}
          sourceId={filesSource.id}
          hostName={filesSource.name}
          wsUrl={`${getBackendProtocol().ws}//${getBackendHost()}/ws/client?token=${encodeURIComponent(token)}`}
          onClose={() => setFilesSource(null)}
        />
      )}
      {showAccountSecurity && token && (
        <AccountSecurity
          token={token}
          username={username || ''}
          server={localStorage.getItem('lunaris_server_host') || getCurrentBrowserServerUrl()}
          apiBase={() => `${getBackendProtocol().http}//${getBackendHost()}`}
          onClose={() => setShowAccountSecurity(false)}
          onToken={(next) => { localStorage.setItem('lunaris_token', next); setToken(next); }}
          onSessionEnded={() => { setShowAccountSecurity(false); handleLogout(); }}
        />
      )}
      {linkRequest && (
        <div className="host-confirm-overlay">
          <div className="host-confirm-card" role="alertdialog" aria-modal="true" aria-labelledby="linkTitle">
            {/* The card is two columns (icon, text): without the icon the
                text would be squeezed into the icon's column. */}
            <div className="host-confirm-icon danger" aria-hidden="true">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4" />
                <polyline points="10 17 15 12 10 7" />
                <line x1="15" y1="12" x2="3" y2="12" />
              </svg>
            </div>
            <div className="host-confirm-content">
              <h2 id="linkTitle">{linkRequest.connect ? t('Sign in and connect?') : t('Sign in?')}</h2>
              <p>
                {linkRequest.connect
                  ? t('Sign in as %1 and connect to %2?', linkRequest.user || t('another account'), linkRequest.computer || linkRequest.connect)
                  : t('Sign in as %1?', linkRequest.user || t('another account'))}
              </p>
              <p>{t('This replaces the account this browser is signed in to (%1). Only continue if you opened this link yourself.', window.location.host)}</p>
            </div>
            <div className="host-confirm-actions">
              <button
                className="btn-secondary"
                autoFocus
                onClick={() => { pendingLink = null; setLinkRequest(null); }}
              >{t('Cancel')}</button>
              <button
                className="btn-danger"
                onClick={() => { applyLink(linkRequest); pendingLink = null; window.location.reload(); }}
              >{linkRequest.connect ? t('Sign in and connect') : t('Sign in')}</button>
            </div>
          </div>
        </div>
      )}

      {computerSettingsHost && token && userRole === 'admin' && (
        <ComputerSettingsDialog
          hostId={computerSettingsHost.id}
          hostName={computerSettingsHost.name}
          token={token}
          apiBase={() => `${getBackendProtocol().http}//${getBackendHost()}`}
          onClose={() => setComputerSettingsHost(null)}
        />
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

      {pendingDeleteSource && (
        <div className="host-confirm-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !deleteSourceBusy) setPendingDeleteSource(null); }}>
          <div className="host-confirm-card" role="dialog" aria-modal="true" aria-labelledby="deleteSourceTitle">
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
              <h2 id="deleteSourceTitle">{t('Delete computer')}</h2>
              <p>
                {deleteSourceCard && deleteSourceCard.methods.length > 1
                  ? t('Remove %1 from %2? The other ways to connect stay.',
                    deleteSourceCard.methods.find(m => m.source?.id === pendingDeleteSource.id && m.source.host_id === pendingDeleteSource.host_id)?.label || pendingDeleteSource.name,
                    deleteSourceCard.title)
                  : t('Delete %1 from the list? It is reached through %2, which stays.', pendingDeleteSource.name, pendingDeleteSource.host_name)}
              </p>
              {deleteSourceError && <p className="add-host-error" role="alert">{deleteSourceError}</p>}
            </div>
            <div className="host-confirm-actions">
              <button className="btn-secondary" disabled={deleteSourceBusy} onClick={() => setPendingDeleteSource(null)}>{t('Cancel')}</button>
              <button className="btn-danger" disabled={deleteSourceBusy} onClick={() => handleDeleteSource(pendingDeleteSource)}>
                {deleteSourceBusy ? t('Deleting…') : t('Delete')}
              </button>
            </div>
          </div>
        </div>
      )}

      {addHostOpen && (
        <div className="host-confirm-overlay add-host-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !addHostBusy && !sourceSetup) closeAddHost(); }}>
          <form
            className={`host-confirm-card add-host-card${hasTypeRail ? ' has-rail' : ''}`}
            role="dialog"
            aria-modal="true"
            aria-labelledby="addHostTitle"
            noValidate
            onSubmit={(e) => { e.preventDefault(); if (sourceSetup) return; if (sourceDialog) submitSource(); else submitAddHost(); }}
          >
            <div className="host-confirm-content">
              <h2 id="addHostTitle">{editSource ? t('Edit computer') : addTo ? t('Add a connection method to %1', addTo.title) : t('Add computer')}</h2>
              {sourceSetup ? (
                <p>{t('Setting up %1', sourceName.trim())}</p>
              ) : addTo ? (
                <p>{t('It is shown as one more way to connect to %1.', addTo.title)}</p>
              ) : editSource ? (
                <p>
                  {editSource.address
                    ? t('%1 at %2, reached through %3.', t(editSource.connector_name || editSource.connector), editSource.address, editSource.host_name)
                    : t('%1, reached through %2.', t(editSource.connector_name || editSource.connector), editSource.host_name)}
                </p>
              ) : null}
            </div>
            {sourceSetup && (
              <div className="source-setup" id="sourceSetup">
                {sourceSetup.failed ? (
                  <div className="add-host-error" role="alert">{sourceSetup.failed}</div>
                ) : (
                  <>
                    {/* In the connector's own words. */}
                    <p className="source-setup-message" aria-live="polite">{t(sourceSetup.message)}</p>
                    {sourceSetup.code && (
                      <div className="source-setup-code" aria-label={t('Code')}>{sourceSetup.code}</div>
                    )}
                    <div className="source-setup-waiting" role="status">
                      <span className="source-setup-spinner" aria-hidden="true" />
                      {t('Waiting for the other computer…')}
                    </div>
                  </>
                )}
              </div>
            )}
            {!sourceSetup && (
              <div className="add-host-main">
                {hasTypeRail && (
                  <div className="form-group type-rail">
                    <span className="option-group-label" id="addHostTypeLabel">{t('Type')}</span>
                    {/* Every type in sight: its logo and its short name, a list beside the form on a wide window. */}
                    <div className="type-tiles" id="addHostType" role="radiogroup" aria-labelledby="addHostTypeLabel">
                      {dialogTypes.map(type => (
                        <button
                          key={type.id || 'lunaris'}
                          type="button"
                          role="radio"
                          aria-checked={addType === type.id}
                          aria-label={t(type.name)}
                          title={t(type.name)}
                          data-type={type.id || 'lunaris'}
                          className={`type-tile${addType === type.id ? ' selected' : ''}`}
                          onClick={() => chooseAddType(type.id)}
                        >
                          <TypeMark id={type.id || 'lunaris'} name={t(type.name)} />
                          <span className="type-tile-name">{typeShortName(type.id || 'lunaris', t(type.name))}</span>
                        </button>
                      ))}
                    </div>
                  </div>
                )}
                <div className="add-host-body">
                  {/* What the chosen type is and says about itself (in its connector's words), once. */}
                  {!editSource && dialogType && (
                    <p className="type-note" id="addHostTypeNote">
                      <strong>{t(dialogType.name)}</strong>
                      {dialogType.note ? <span>{t(dialogType.note)}</span> : null}
                    </p>
                  )}
                  {/* RustDesk, Sunshine, RDP, VNC… are reached through a Lunaris
                      agent that is online and offers them: none now. */}
                  {!editSource && !addTo && connectorTypes.length === 0 && (
                    <p className="type-note add-types-missing" role="note">
                      <span>{t('To add a RustDesk, Sunshine, RDP or VNC computer, a Lunaris computer must be online to reach it through: install lunaris-agent on a computer in that network (a VPS works too).')}</span>
                      {userRole === 'admin' && (
                        <span>{t('Or let this server reach them itself: its gateway runs when lunaris-agent is installed on the server too (see the Server page).')}</span>
                      )}
                      {userRole === 'admin' && (
                        <button type="button" className="btn-secondary" onClick={() => { closeAddHost(); setActiveTab('install_agents'); }}>{t('Install agents')}</button>
                      )}
                      {userRole === 'admin' && (
                        <button type="button" className="btn-secondary" onClick={() => { closeAddHost(); setActiveTab('server'); }}>{t('Server gateway')}</button>
                      )}
                    </p>
                  )}
                  {!sourceDialog && (
                    <div className="form-grid">
                      <div className="form-group span-6">
                        <label htmlFor="addHostName">{t('Name (optional)')}</label>
                        <input
                          id="addHostName"
                          autoFocus
                          value={addHostName}
                          onChange={(e) => setAddHostName(e.target.value)}
                          placeholder={t('The name the computer reports')}
                          autoComplete="off"
                          maxLength={100}
                        />
                      </div>
                      <div className="form-group span-3">
                        <label htmlFor="addHostAddress">{t('Address')}</label>
                        <input
                          id="addHostAddress"
                          placeholder={t('e.g. 192.168.1.20')}
                          value={addHostAddress}
                          onChange={(e) => setAddHostAddress(e.target.value)}
                          autoComplete="off"
                        />
                      </div>
                      <div className="form-group span-3">
                        <label htmlFor="addHostPassword">{t('Password (optional)')}</label>
                        <input
                          id="addHostPassword"
                          type="password"
                          value={addHostPassword}
                          onChange={(e) => setAddHostPassword(e.target.value)}
                          autoComplete="new-password"
                        />
                      </div>
                      <span className="form-hint span-6">{t('Without the password, the person at that computer is asked to accept each connection.')}</span>
                    </div>
                  )}
                  {sourceDialog && (
                    <div className="form-grid">
                      <div className={`form-group ${!editSource && throughBesideName ? 'span-3' : 'span-6'}`}>
                        <label htmlFor="sourceName">{t('Name')}</label>
                        <input
                          id="sourceName"
                          value={sourceName}
                          onChange={(e) => setSourceName(e.target.value)}
                          autoComplete="off"
                          maxLength={100}
                        />
                      </div>
                      {!editSource && (
                        <div className={`form-group ${throughBesideName ? 'span-3' : 'span-6'}`}>
                          {/* A few to choose from: all in sight. Many: a list to open. */}
                          {dialogHosts.length <= MAX_OPTION_CARDS ? (
                            <>
                              <span className="option-group-label" id="sourceHostLabel">{t('Through which Lunaris computer')}</span>
                              <div className="option-cards compact" id="sourceHost" role="radiogroup" aria-labelledby="sourceHostLabel">
                                {dialogHosts.map(h => (
                                  <button
                                    key={h.host_id}
                                    type="button"
                                    role="radio"
                                    aria-checked={sourceHostId === h.host_id}
                                    className={`option-card${sourceHostId === h.host_id ? ' selected' : ''}`}
                                    onClick={() => chooseSourceHost(h.host_id)}
                                  >
                                    <span className="option-card-name">{h.host_name}</span>
                                  </button>
                                ))}
                              </div>
                            </>
                          ) : (
                            <>
                              <label htmlFor="sourceHost">{t('Through which Lunaris computer')}</label>
                              <select id="sourceHost" value={sourceHostId} onChange={(e) => chooseSourceHost(e.target.value)}>
                                {dialogHosts.map(h => <option key={h.host_id} value={h.host_id}>{h.host_name}</option>)}
                              </select>
                            </>
                          )}
                        </div>
                      )}
                      {!editSource && (
                        <span className="form-hint span-6">{t('That computer must be on the same network as the computer being added.')}</span>
                      )}
                      {editSource && (
                        <span className="form-hint span-6">
                          {dialogConnector
                            ? (editSource.values ? t('Passwords are not shown: leave one empty to keep it.') : t('Leave a field empty to keep it.'))
                            : t('Only the name can be changed while %1 is offline.', editSource.host_name)}
                        </span>
                      )}
                      {layoutFields(dialogConnector?.fields || []).map(({ field, span }) => {
                        const choices = choicesOf(field);
                        const label = (
                          <>
                            {t(field.label || field.key)}
                            {!editSource && !field.required && !field.default ? ` ${t('(optional)')}` : ''}
                          </>
                        );
                        // Many, or long ones: a list to open, like the boxes around it.
                        if (choices.length > 0 && choiceAsList(field)) {
                          return (
                            <div className={`form-group span-${span}`} key={field.key}>
                              <label htmlFor={`sourceField-${field.key}`}>{label}</label>
                              <select
                                id={`sourceField-${field.key}`}
                                value={sourceValues[field.key] ?? ''}
                                onChange={(e) => setSourceValues((current) => ({ ...current, [field.key]: e.target.value }))}
                              >
                                {!choices.some(choice => choice.value === (sourceValues[field.key] ?? '')) && <option value={sourceValues[field.key] ?? ''} disabled hidden />}
                                {choices.map(choice => <option key={choice.value} value={choice.value}>{t(choice.label)}</option>)}
                              </select>
                            </div>
                          );
                        }
                        // One of a few: options to press, not a box to type in.
                        if (choices.length > 0) {
                          return (
                            <div className={`form-group span-${span}`} key={field.key}>
                              <span className="option-group-label" id={`sourceFieldLabel-${field.key}`}>{label}</span>
                              <div className="option-cards compact" id={`sourceField-${field.key}`} role="radiogroup" aria-labelledby={`sourceFieldLabel-${field.key}`}>
                                {choices.map(choice => (
                                  <button
                                    key={choice.value}
                                    type="button"
                                    role="radio"
                                    aria-checked={(sourceValues[field.key] ?? '') === choice.value}
                                    data-value={choice.value}
                                    className={`option-card${(sourceValues[field.key] ?? '') === choice.value ? ' selected' : ''}`}
                                    onClick={() => setSourceValues((current) => ({ ...current, [field.key]: choice.value }))}
                                  >
                                    <span className="option-card-name">{t(choice.label)}</span>
                                  </button>
                                ))}
                              </div>
                            </div>
                          );
                        }
                        return (
                          <div className={`form-group span-${span}`} key={field.key}>
                            <label htmlFor={`sourceField-${field.key}`}>{label}</label>
                            <input
                              id={`sourceField-${field.key}`}
                              type={field.kind === 'secret' ? 'password' : field.kind === 'port' ? 'number' : 'text'}
                              inputMode={field.kind === 'port' ? 'numeric' : undefined}
                              min={field.kind === 'port' ? 1 : undefined}
                              max={field.kind === 'port' ? 65535 : undefined}
                              value={sourceValues[field.key] ?? ''}
                              onChange={(e) => setSourceValues((current) => ({ ...current, [field.key]: e.target.value }))}
                              autoComplete={field.kind === 'secret' ? 'new-password' : 'off'}
                              placeholder={editSource && field.kind === 'secret' ? t('Empty keeps the saved password') : undefined}
                            />
                          </div>
                        );
                      })}
                    </div>
                  )}
                  {editSource && (sameChoices.length > 0 || editSameWas) && (
                    <div className="form-group">
                      <span className="option-group-label" id="sourceSameLabel">{t('Same computer as')}</span>
                      <div className="option-cards compact scroll" id="sourceSame" role="radiogroup" aria-labelledby="sourceSameLabel">
                        <button
                          type="button"
                          role="radio"
                          aria-checked={editSame === ''}
                          className={`option-card${editSame === '' ? ' selected' : ''}`}
                          onClick={() => setEditSame('')}
                        >
                          <span className="option-card-name">{t('A computer of its own')}</span>
                        </button>
                        {sameChoices.map(card => (
                          <button
                            key={card.key}
                            type="button"
                            role="radio"
                            aria-checked={editSame === card.key}
                            className={`option-card${editSame === card.key ? ' selected' : ''}`}
                            onClick={() => setEditSame(card.key)}
                          >
                            <span className="option-card-name">{card.title}</span>
                          </button>
                        ))}
                      </div>
                      <span className="form-hint">{t('Computers that are the same machine are shown as one, with a way to connect for each.')}</span>
                    </div>
                  )}
                  {addHostError && <div className="add-host-error" role="alert" ref={addHostErrorRef}>{addHostError}</div>}
                </div>
              </div>
            )}
            {sourceSetup ? (
              <div className="host-confirm-actions">
                {sourceSetup.failed && (
                  <button type="button" className="btn-secondary" onClick={() => setSourceSetup(null)}>{t('Back')}</button>
                )}
                <button type="button" className={sourceSetup.failed ? 'btn-primary' : 'btn-secondary'} onClick={closeAddHost}>
                  {sourceSetup.failed ? t('Close') : t('Cancel')}
                </button>
              </div>
            ) : (
              <div className="host-confirm-actions">
                <button type="button" className="btn-secondary" disabled={addHostBusy} onClick={closeAddHost}>{t('Cancel')}</button>
                <button type="submit" className="btn-primary" disabled={addHostBusy}>
                  {editSource ? (addHostBusy ? t('Saving…') : t('Save')) : (addHostBusy ? t('Adding…') : t('Add'))}
                </button>
              </div>
            )}
          </form>
        </div>
      )}

      {showAbout && (
        <AboutDialog
          token={token}
          apiBase={() => `${getBackendProtocol().http}//${getBackendHost()}`}
          onClose={() => setShowAbout(false)}
        />
      )}

      {renameHost && (
        <div className="host-confirm-overlay add-host-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !hostDialogBusy) setRenameHost(null); }}>
          <form
            className="host-confirm-card add-host-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="renameHostTitle"
            noValidate
            onSubmit={(e) => { e.preventDefault(); submitRename(); }}
          >
            <div className="host-confirm-content">
              <h2 id="renameHostTitle">{t('Rename %1', renameHost.name)}</h2>
              <p>{t('Everyone who can use this computer sees it with this name.')}</p>
            </div>
            <div className="add-host-main">
              <div className="add-host-body">
                <div className="form-grid">
                  <div className="form-group span-6">
                    <label htmlFor="renameHostName">{t('Name')}</label>
                    <input
                      id="renameHostName"
                      autoFocus
                      value={renameText}
                      maxLength={100}
                      autoComplete="off"
                      placeholder={renameHost.agent_name || renameHost.name}
                      onChange={(e) => setRenameText(e.target.value)}
                    />
                  </div>
                  <span className="form-hint span-6">{t('Leave it empty to use the name the computer reports (%1).', renameHost.agent_name || renameHost.name)}</span>
                </div>
                {hostDialogError && <div className="add-host-error" role="alert">{hostDialogError}</div>}
              </div>
            </div>
            <div className="host-confirm-actions">
              <button type="button" className="btn-secondary" disabled={hostDialogBusy} onClick={() => setRenameHost(null)}>{t('Cancel')}</button>
              <button type="submit" className="btn-primary" disabled={hostDialogBusy}>{hostDialogBusy ? t('Saving…') : t('Save')}</button>
            </div>
          </form>
        </div>
      )}

      {editAddressHost && (
        <div className="host-confirm-overlay add-host-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !hostDialogBusy) setEditAddressHost(null); }}>
          <form
            className="host-confirm-card add-host-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="editAddressTitle"
            noValidate
            onSubmit={(e) => { e.preventDefault(); submitEditAddress(); }}
          >
            <div className="host-confirm-content">
              <h2 id="editAddressTitle">{t('Edit computer')}</h2>
              <p>{t('The server connects to this computer at its address. Saving connects to it again.')}</p>
            </div>
            <div className="add-host-main">
              <div className="add-host-body">
                <div className="form-grid">
                  <div className="form-group span-6">
                    <label htmlFor="editAddressName">{t('Name (optional)')}</label>
                    <input
                      id="editAddressName"
                      value={editAddressName}
                      maxLength={100}
                      autoComplete="off"
                      placeholder={editAddressHost.agent_name || (isPlaceholderHost(editAddressHost) ? '' : editAddressHost.name) || t('The name the computer reports')}
                      onChange={(e) => setEditAddressName(e.target.value)}
                    />
                  </div>
                  <div className="form-group span-3">
                    <label htmlFor="editAddressAddress">{t('Address')}</label>
                    <input
                      id="editAddressAddress"
                      autoFocus
                      value={editAddress}
                      autoComplete="off"
                      placeholder={t('e.g. 192.168.1.20')}
                      onChange={(e) => setEditAddress(e.target.value)}
                    />
                  </div>
                  <div className="form-group span-3">
                    <label htmlFor="editAddressPassword">{t('Password')}</label>
                    <input
                      id="editAddressPassword"
                      type="password"
                      value={editAddressPassword}
                      autoComplete="new-password"
                      placeholder={t('Empty keeps the saved password')}
                      onChange={(e) => setEditAddressPassword(e.target.value)}
                    />
                  </div>
                  <span className="form-hint span-6">
                    {editAddressHost.needs_approval
                      ? t('No password is saved: the person at that computer accepts each connection. Enter its password to connect without asking.')
                      : t('Leave the password empty to keep the saved one.')}
                  </span>
                </div>
                {hostDialogError && <div className="add-host-error" role="alert">{hostDialogError}</div>}
              </div>
            </div>
            <div className="host-confirm-actions">
              <button type="button" className="btn-secondary" disabled={hostDialogBusy} onClick={() => setEditAddressHost(null)}>{t('Cancel')}</button>
              <button type="submit" className="btn-primary" disabled={hostDialogBusy}>{hostDialogBusy ? t('Saving…') : t('Save')}</button>
            </div>
          </form>
        </div>
      )}

      {resetKeyHost && (
        <div className="host-confirm-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget && !hostDialogBusy) setResetKeyHost(null); }}>
          <div className="host-confirm-card" role="dialog" aria-modal="true" aria-labelledby="resetKeyTitle">
            <div className="host-confirm-icon danger" aria-hidden="true">{ICON_KEY}</div>
            <div className="host-confirm-content">
              <h2 id="resetKeyTitle">{t('Reset the key of %1?', resetKeyHost.name)}</h2>
              <p>{t('Each computer proves it is itself with a key it made when it first connected. Resetting forgets that key: the computer is disconnected now, and the next Lunaris agent that connects with this computer\'s ID is accepted and its key is kept.')}</p>
              <p>{t('Use it when the computer was reinstalled or lost its settings and is refused. A computer that joined with an install token needs a valid install token again. Its groups stay.')}</p>
              {hostDialogError && <p className="add-host-error" role="alert">{hostDialogError}</p>}
            </div>
            <div className="host-confirm-actions">
              <button className="btn-secondary" autoFocus disabled={hostDialogBusy} onClick={() => setResetKeyHost(null)}>{t('Cancel')}</button>
              <button className="btn-danger" disabled={hostDialogBusy} onClick={submitResetKey}>{hostDialogBusy ? t('Resetting…') : t('Reset key')}</button>
            </div>
          </div>
        </div>
      )}

      {settingsOpen && (
        <StreamSettingsDialog
          hostName={settingsHostNow ? settingsHostNow.name : settingsSource ? settingsSource.name : undefined}
          hostAddress={settingsHostNow ? hostAddressText(settingsHostNow) : settingsSource?.address || ''}
          sourceOnly={!!settingsSource}
          hostInfo={settingsHostNow ? settingsHostInfo : settingsSource ? [
            { label: t('Name'), value: settingsSource.name },
            { label: t('Type'), value: t(settingsSource.connector_name || settingsSource.connector) },
            { label: t('Through'), value: settingsSource.host_name },
            ...(settingsSource.address ? [{ label: t('Address'), value: settingsSource.address }] : []),
          ] : undefined}
          values={draftHostSettings}
          onChange={(key, value) => updateDraftHostSettings(key, value)}
          onCancel={closeSettings}
          onSave={applyHostSettings}
          onUseDefaults={settingsComputer ? useDefaultSettings : undefined}
          ownSettings={settingsOwn}
          footerNote={settingsDefaults
            ? t('Used for every computer that has no settings of its own.')
            : settingsOwn
              ? t('%1 has its own settings; the ones not changed here follow the defaults.', settingsHostNow?.name || settingsSource?.name || '')
              : t('%1 uses the default settings. A change made here is kept for it only.', settingsHostNow?.name || settingsSource?.name || '')}
          displays={settingsHost ? hostAvailableDisplays.map(d => ({
            id: d.id,
            label: `${d.name} (${d.width}x${d.height} @ ${d.refresh_rate.toFixed(0)}Hz)${d.is_primary ? ' ★' : ''}`,
          })) : []}
          encoders={hostEncoders}
          showNativeClient={!!(window as any).__TAURI__}
        />
      )}

      </main>


    </div>
  );
}

export default App;
