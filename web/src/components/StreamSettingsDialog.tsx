import { useEffect, useState, type ReactNode } from 'react';
import {
  BITRATES, CHROMAS, CODECS, DISPLAY_MODES, FPS_OPTIONS, MOUSE_QUEUES, RESOLUTIONS,
  bitrateLabel, mouseQueueLabel, readDisplayMode, readVolume, type DisplayMode,
} from '../streamOptions';
import { t } from '../i18n';
import { readClipboardSync, readFileTransfer } from '../share';
import { readGamepadEnabled } from '../gamepad';
import { readSystemKeys } from '../keyboard';
import GamepadTester from './GamepadTester';

/** The settings the dialog edits (same names as the stored settings). */
export type StreamSettingsValues = {
  resolution: string;
  fps: number;
  codec: string;
  chroma: string;
  bitrate: number;
  mouseQueueLimit: number;
  inputProtocol: string;
  encoder: string;
  display: string;
  useCanvasRenderer: boolean;
};

/** Settings of the viewing device (not of the remote computer), applied right away. */
export type ViewerSettings = {
  displayMode: DisplayMode;
  setDisplayMode: (m: DisplayMode) => void;
  muted: boolean;
  setMuted: (m: boolean) => void;
  volume: number;
  setVolume: (v: number) => void;
  /** Statistics overlay (in a session only). */
  showStats?: boolean;
  setShowStats?: (v: boolean) => void;
  /** Clipboard both ways / sending files with the remote computer. */
  clipboardSync: boolean;
  setClipboardSync: (v: boolean) => void;
  fileTransfer: boolean;
  setFileTransfer: (v: boolean) => void;
};

type Props = {
  hostName?: string;
  /** Where the computer is (its IP addresses, or a source's host:port),
   *  shown beside its name in the header like the app's; '' when not known. */
  hostAddress?: string;
  /** The computer is reached through another one (a source): its picture
   *  arrives as that computer encoded it, so there is no encoder to choose,
   *  and its displays are its own (listed once a session has started). */
  sourceOnly?: boolean;
  /** What is known about the computer (name, status, addresses), shown
   *  read-only first under "Remote computer". */
  /** Rows of "This computer"; a row may add a line under it and a link. */
  hostInfo?: { label: string; value: string; note?: string; link?: { href: string; label: string } }[];
  values: StreamSettingsValues;
  onChange: <K extends keyof StreamSettingsValues>(key: K, value: StreamSettingsValues[K]) => void;
  onCancel: () => void;
  onSave: () => void;
  /** "Save" for stored defaults, "Apply" in a session. */
  saveLabel?: string;
  footerNote?: string;
  displays?: { id: string; label: string }[];
  /** Codecs this browser/host pair can't use (disabled in the list). */
  unsupportedCodecs?: string[];
  /** Per-codec labels (e.g. "Auto (H264)", "AV1 (Unsupported by browser)"). */
  codecLabels?: Record<string, string>;
  /** Encoders the remote computer reported: the list to choose from (the
   *  standard list while none are known). */
  encoders?: string[];
  /** Settings of one computer: "Use defaults" forgets what it has of its
   *  own (`ownSettings`: it has some now). */
  onUseDefaults?: () => void;
  ownSettings?: boolean;
  /** This device's display and sound in a session. Without it the dialog
   *  keeps them itself, as this browser's defaults. */
  viewer?: ViewerSettings;
};

/** The encoders offered when the remote computer has not said which it has. */
const STANDARD_ENCODERS = ['native', 'ffmpeg', 'nvenc', 'amf', 'qsv', 'vaapi', 'software'];
const ENCODER_LABELS: Record<string, string> = {
  native: 'Native GPU', ffmpeg: 'FFmpeg GPU', nvenc: 'NVENC', amf: 'AMF', qsv: 'QSV', vaapi: 'VAAPI', software: 'Software',
};
const encoderLabel = (e: string) => ENCODER_LABELS[e] || e;

/** The encoders to choose from (besides Auto): those the remote computer
 *  reported, else the standard ones; the chosen one stays in the list. */
const encoderChoices = (reported: string[], chosen: string): string[] => {
  const known = reported.map(e => e.toLowerCase()).filter(e => e && e !== 'auto');
  const list = [...new Set(known.length > 0 ? known : STANDARD_ENCODERS)];
  const current = chosen.toLowerCase();
  if (current && current !== 'auto' && !list.includes(current)) list.push(current);
  return list;
};

type Category = 'video' | 'display' | 'audio' | 'sharing' | 'host' | 'input' | 'controllers';

const CATEGORIES: { key: Category; label: string; icon: ReactNode }[] = [
  { key: 'video', label: 'Video', icon: <><rect x="3" y="4" width="18" height="12" rx="1.5" /><path d="M8 20h8M12 16v4" /></> },
  { key: 'display', label: 'Display', icon: <><rect x="3" y="5" width="18" height="12" rx="1.5" /><path d="M7 21h10M9 9l-2 2 2 2M15 9l2 2-2 2" /></> },
  { key: 'audio', label: 'Audio', icon: <><path d="M4 9v6h4l5 4V5L8 9H4z" /><path d="M16 9a3.5 3.5 0 0 1 0 6M18.5 6.5a7 7 0 0 1 0 11" /></> },
  { key: 'sharing', label: 'Clipboard & files', icon: <><rect x="8" y="3" width="8" height="4" rx="1" /><path d="M8 5H6a1 1 0 0 0-1 1v14a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1h-2" /></> },
  { key: 'host', label: 'Remote computer', icon: <><rect x="4" y="4" width="16" height="6" rx="1" /><rect x="4" y="14" width="16" height="6" rx="1" /><path d="M8 7h.01M8 17h.01" /></> },
  { key: 'input', label: 'Input', icon: <><rect x="3" y="6" width="18" height="12" rx="2" /><path d="M7 10h.01M11 10h.01M15 10h.01M7 14h10" /></> },
  { key: 'controllers', label: 'Controllers', icon: <><path d="M7 7h10a5 5 0 0 1 4.9 6l-.8 4a2.5 2.5 0 0 1-4.4 1.1L15 16H9l-1.7 2.1a2.5 2.5 0 0 1-4.4-1.1l-.8-4A5 5 0 0 1 7 7z" /><path d="M7.5 10v3M6 11.5h3M15.5 11h.01M17.5 13h.01" /></> },
];

const Icon = ({ children }: { children: ReactNode }) => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{children}</svg>
);

/** "Name + explanation" on the left, the control on the right. */
const Row = ({ title, hint, children }: { title: string; hint?: string; children: ReactNode }) => (
  <div className="ssd-row">
    <div className="ssd-row-text">
      <div className="ssd-row-title">{title}</div>
      {hint && <div className="ssd-row-hint">{hint}</div>}
    </div>
    <div className="ssd-row-control">{children}</div>
  </div>
);

const Switch = ({ checked, disabled, onChange, label }: { checked: boolean; disabled?: boolean; onChange: (v: boolean) => void; label: string }) => (
  <label className="ssd-switch">
    <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} aria-label={label} />
    <span aria-hidden="true" />
  </label>
);

const readShowStats = (): boolean => {
  try {
    const stored = localStorage.getItem('lunaris_show_stats');
    if (stored !== null) return stored !== 'false';
  } catch { /* storage unavailable */ }
  return !(window.matchMedia?.('(pointer: coarse)').matches && Math.min(window.innerWidth, window.innerHeight) < 600);
};

/** Display and sound kept in this browser when no session applies them. */
const useStoredViewer = (): ViewerSettings => {
  const [displayMode, setMode] = useState<DisplayMode>(readDisplayMode);
  const [muted, setMutedState] = useState<boolean>(() => localStorage.getItem('lunaris_stream_muted') === 'true');
  const [volume, setVol] = useState<number>(readVolume);
  const [clipboardSync, setClip] = useState<boolean>(readClipboardSync);
  const [fileTransfer, setFiles] = useState<boolean>(readFileTransfer);
  // The statistics overlay as the next session starts it (StreamPlayer reads
  // the same key; on by default except on phones).
  const [showStats, setStats] = useState<boolean>(readShowStats);
  return {
    showStats,
    setShowStats: (v) => { setStats(v); localStorage.setItem('lunaris_show_stats', String(v)); },
    clipboardSync,
    setClipboardSync: (v) => { setClip(v); localStorage.setItem('lunaris_clipboard_sync', String(v)); },
    fileTransfer,
    setFileTransfer: (v) => { setFiles(v); localStorage.setItem('lunaris_file_transfer', String(v)); },
    displayMode,
    setDisplayMode: (m) => { setMode(m); localStorage.setItem('lunaris_display_mode', m); },
    muted,
    setMuted: (m) => { setMutedState(m); localStorage.setItem('lunaris_stream_muted', String(m)); },
    volume,
    setVolume: (v) => { setVol(v); localStorage.setItem('lunaris_stream_volume', String(v)); },
  };
};

/**
 * Stream settings in the Settings style: categories on the left, sections of
 * explained rows on the right. The desktop app's dialog (Settings.qml) has the
 * same layout and choices.
 */
export default function StreamSettingsDialog({
  hostName, hostAddress, sourceOnly, hostInfo, values, onChange, onCancel, onSave, saveLabel, footerNote, displays, unsupportedCodecs = [],
  codecLabels = {}, encoders = [], viewer: viewerProp, onUseDefaults, ownSettings,
}: Props) {
  const [category, setCategory] = useState<Category>('video');
  // Ask the remote computer to record its microphone too (sent with the
  // next session request; the app keeps the same choice in view_prefs).
  const [recordMic, setRecordMic] = useState<boolean>(() => {
    try { return localStorage.getItem('lunaris_stream_microphone') === 'true'; } catch { return false; }
  });
  const storedViewer = useStoredViewer();
  const viewer = viewerProp ?? storedViewer;
  const canvasSupported = typeof (window as any).MediaStreamTrackProcessor !== 'undefined';
  const webTransportSupported = typeof (window as any).WebTransport !== 'undefined';
  const [gamepadOn, setGamepadOn] = useState<boolean>(readGamepadEnabled);
  // The app's "System shortcuts" has a third choice, "While the pointer is
  // over the picture": a browser gives a page those keys (Keyboard Lock) in
  // full screen only, so here it is "In full screen only" or "Never".
  const [systemKeysOn, setSystemKeysOn] = useState<boolean>(readSystemKeys);
  const keyboardLockSupported = typeof navigator !== 'undefined' && !!(navigator as any).keyboard?.lock;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onCancel(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onCancel]);

  const current = CATEGORIES.find(c => c.key === category)!;

  return (
    <div className="ssd-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) onCancel(); }}>
      <div className="ssd" role="dialog" aria-modal="true" aria-label={t('Stream settings')}>
        <header className="ssd-header">
          <div className="ssd-breadcrumb">
            <span className="ssd-crumb">{t('Stream settings')}</span>
            <span className="ssd-crumb-sep" aria-hidden="true">›</span>
            <span className="ssd-crumb-current">{t(current.label)}</span>
            {hostName && (
              <span className="ssd-host">
                <span className="ssd-host-name" title={hostName}>{hostName}</span>
                {hostAddress && (
                  <span className="ssd-host-address" title={`${t('IP Address')}: ${hostAddress}`}>{hostAddress}</span>
                )}
              </span>
            )}
          </div>
          <button type="button" className="ssd-close" onClick={onCancel} aria-label={t('Close')} title={t('Close')}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M6 6l12 12M18 6L6 18" /></svg>
          </button>
        </header>

        <div className="ssd-body">
          <nav className="ssd-nav">
            {CATEGORIES.map(c => (
              <button key={c.key} type="button" className={`ssd-nav-item${c.key === category ? ' active' : ''}`} onClick={() => setCategory(c.key)}>
                <Icon>{c.icon}</Icon>
                <span>{t(c.label)}</span>
              </button>
            ))}
          </nav>

          <div className="ssd-content">
            {category === 'video' && (
              <>
                <h3 className="ssd-section">{t('Picture')}</h3>
                <div className="ssd-card">
                  <Row title={t('Resolution')} hint={t('Size of the picture the remote computer sends. Default 1080p (FHD).')}>
                    <select value={values.resolution} onChange={(e) => onChange('resolution', e.target.value)}>
                      {RESOLUTIONS.map(r => <option key={r.value} value={r.value}>{r.label} ({r.width} × {r.height})</option>)}
                    </select>
                  </Row>
                  <Row title={t('Frame rate')} hint={t("Frames per second sent by the remote computer. It cannot send more than its screen's refresh rate.")}>
                    <select value={values.fps} onChange={(e) => onChange('fps', Number(e.target.value))}>
                      {FPS_OPTIONS.map(f => <option key={f} value={f}>{f} FPS</option>)}
                    </select>
                  </Row>
                  <Row title={t('Bitrate')} hint={t('Higher looks sharper but needs a faster network.')}>
                    <select value={values.bitrate} onChange={(e) => onChange('bitrate', Number(e.target.value))}>
                      {BITRATES.map(b => <option key={b} value={b}>{bitrateLabel(b)}</option>)}
                      {!BITRATES.includes(values.bitrate) && <option value={values.bitrate}>{(values.bitrate / 1000).toFixed(1)} Mbps</option>}
                    </select>
                  </Row>
                </div>

                <h3 className="ssd-section">{t('Compression')}</h3>
                <div className="ssd-card">
                  <Row title={t('Codec')} hint={t('Auto: the best codec this browser and the remote computer both handle (AV1, then H.265, then H.264).')}>
                    <select value={values.codec} onChange={(e) => onChange('codec', e.target.value)}>
                      {CODECS.map(c => <option key={c.value} value={c.value} disabled={unsupportedCodecs.includes(c.value)}>{codecLabels[c.value] ?? t(c.label)}</option>)}
                    </select>
                  </Row>
                  <Row title={t('Chroma')} hint={t('4:4:4 keeps coloured text sharp. H.264 only, encoded on NVIDIA or the CPU.')}>
                    <select value={values.chroma} onChange={(e) => onChange('chroma', e.target.value === '444' ? '444' : '420')}>
                      {CHROMAS.map(c => <option key={c.value} value={c.value}>{c.label}</option>)}
                    </select>
                  </Row>
                </div>

                <h3 className="ssd-section">{t('Decoding')}</h3>
                <div className="ssd-card">
                  <Row title={t('Canvas renderer')} hint={t('Draws frames on a canvas as soon as they are decoded. Can lower latency, but uses about twice the CPU. Not available in every browser.') + (canvasSupported ? '' : ' ' + t('(not supported by this browser)'))}>
                    <Switch label={t('Canvas renderer')} checked={values.useCanvasRenderer && canvasSupported} disabled={!canvasSupported} onChange={(v) => onChange('useCanvasRenderer', v)} />
                  </Row>
                </div>
              </>
            )}

            {category === 'display' && (
              <>
                <h3 className="ssd-section">{t('Picture on screen')}</h3>
                <div className="ssd-card">
                  <Row title={t('Display mode')} hint={t('How the remote screen fits this window. Applies right away to every computer.')}>
                    <select value={viewer.displayMode} onChange={(e) => viewer.setDisplayMode(e.target.value as DisplayMode)}>
                      {DISPLAY_MODES.map(m => <option key={m.value} value={m.value}>{t(m.label)}</option>)}
                    </select>
                  </Row>
                  {viewer.setShowStats && (
                    <Row title={t('Statistics overlay')} hint={t('Frame rate, bitrate, latency and the codec in a corner of the stream.')}>
                      <Switch label={t('Statistics overlay')} checked={!!viewer.showStats} onChange={(v) => viewer.setShowStats!(v)} />
                    </Row>
                  )}
                </div>
              </>
            )}

            {category === 'audio' && (
              <>
                <h3 className="ssd-section">{t('Sound')}</h3>
                <div className="ssd-card">
                  <Row title={t('Play sound')} hint={t("The remote computer's sound plays on this device. Applies right away.")}>
                    <Switch label={t('Play sound')} checked={!viewer.muted} onChange={(v) => viewer.setMuted(!v)} />
                  </Row>
                  <Row title={t('Volume')} hint={`${viewer.volume}%`}>
                    <input
                      type="range" min={0} max={100} step={1}
                      className="ssd-range"
                      value={viewer.volume}
                      disabled={viewer.muted}
                      onChange={(e) => viewer.setVolume(Number(e.target.value))}
                      aria-label={t('Volume')}
                    />
                  </Row>
                </div>
                <h3 className="ssd-section">{t('Record')}</h3>
                <div className="ssd-card">
                  <Row title={t("Remote computer's microphone")} hint={t('Also send what its microphone hears, mixed with its sound. Applies when you next connect.')}>
                    <Switch
                      label={t("Remote computer's microphone")}
                      checked={recordMic}
                      onChange={(v) => {
                        setRecordMic(v);
                        try { localStorage.setItem('lunaris_stream_microphone', String(v)); } catch { /* private mode */ }
                      }}
                    />
                  </Row>
                </div>
              </>
            )}

            {category === 'sharing' && (
              <>
                <h3 className="ssd-section">{t('Clipboard')}</h3>
                <div className="ssd-card">
                  <Row title={t('Sync clipboard')} hint={t('Text you copy here can be pasted on the remote computer, and the other way round. The remote computer can turn this off for itself.') + ' ' + t('In the browser, paste with Ctrl+V in the stream; on http pages copied remote text arrives with your next key press or click.')}>
                    <Switch label={t('Sync clipboard')} checked={viewer.clipboardSync} onChange={viewer.setClipboardSync} />
                  </Row>
                </div>
                <h3 className="ssd-section">{t('Files')}</h3>
                <div className="ssd-card">
                  <Row title={t('File transfer')} hint={t('Browse the remote computer and copy files both ways with the Files button (toolbar or computer card); files dropped on the stream go to its Downloads/Lunaris. Transfers use a connection of their own, so the stream stays smooth.')}>
                    <Switch label={t('File transfer')} checked={viewer.fileTransfer} onChange={viewer.setFileTransfer} />
                  </Row>
                </div>
              </>
            )}

            {category === 'host' && (
              <>
                {hostInfo && hostInfo.length > 0 && (
                  <>
                    <h3 className="ssd-section">{t('This computer')}</h3>
                    <dl className="ssd-card ssd-info">
                      {hostInfo.map(item => (
                        <div className="ssd-info-row" key={item.label}>
                          <dt>{item.label}</dt>
                          <dd>
                            {item.value}
                            {item.note && <span className="ssd-info-note">{item.note}</span>}
                            {item.link && (
                              <a className="ssd-info-link" href={item.link.href} target="_blank" rel="noopener noreferrer">{item.link.label}</a>
                            )}
                          </dd>
                        </div>
                      ))}
                    </dl>
                  </>
                )}
                {!sourceOnly && <h3 className="ssd-section">{t('Encoding')}</h3>}
                {!sourceOnly && <div className="ssd-card">
                  <Row title={t('Encoder')} hint={t("Auto picks the remote computer's hardware encoder (NVENC, VAAPI, QSV, AMF) and falls back to the CPU.")}>
                    <select value={values.encoder} onChange={(e) => onChange('encoder', e.target.value)}>
                      <option value="auto">{t('Auto (recommended)')}</option>
                      {encoderChoices(encoders, values.encoder).map(e => <option key={e} value={e}>{encoderLabel(e)}</option>)}
                    </select>
                  </Row>
                </div>}

                <h3 className="ssd-section">{t('Screen')}</h3>
                <div className="ssd-card">
                  <Row title={t('Display')} hint={t('Which screen of the remote computer to show.')}>
                    <select value={values.display} onChange={(e) => onChange('display', e.target.value)}>
                      <option value="default">{t('Default')}</option>
                      {(displays || []).map(d => <option key={d.id} value={d.id}>{d.label}</option>)}
                      {/* Chosen before, not listed now (the computer has not answered yet). */}
                      {values.display !== 'default' && !(displays || []).some(d => d.id === values.display) && (
                        <option value={values.display}>{values.display}</option>
                      )}
                    </select>
                  </Row>
                </div>
              </>
            )}

            {category === 'input' && (
              <>
                <h3 className="ssd-section">{t('Mouse and keyboard')}</h3>
                <div className="ssd-card">
                  <Row title={t('Input protocol')} hint={t("WebTransport sends mouse and keyboard over QUIC datagrams, which lowers latency on lossy networks; WebRTC is used when it can't connect.") + (webTransportSupported ? '' : ' ' + t('(not supported by this browser)'))}>
                    <select value={values.inputProtocol} onChange={(e) => onChange('inputProtocol', e.target.value)}>
                      <option value="webrtc">{t('WebRTC data channels')}</option>
                      <option value="webtransport" disabled={!webTransportSupported}>WebTransport</option>
                    </select>
                  </Row>
                  <Row title={t('Mouse queue limit')} hint={t('How much mouse movement may wait to be sent. Smaller drops old moves for responsiveness; 256 B suits most networks.')}>
                    <select value={values.mouseQueueLimit} onChange={(e) => onChange('mouseQueueLimit', Number(e.target.value))}>
                      {MOUSE_QUEUES.map(q => <option key={q} value={q}>{mouseQueueLabel(q)}{q === 256 ? ' ' + t('(recommended)') : ''}</option>)}
                    </select>
                  </Row>
                  <Row title={t('System shortcuts')} hint={t('In full screen, send the key combinations the browser and this computer keep for themselves (Alt+Tab, the Windows key, Ctrl+W and others) to the remote computer. Hold Esc to leave full screen. Works in Chrome and Edge; other browsers do not allow it. Applies right away.') + (keyboardLockSupported ? '' : ' ' + t('(not supported by this browser)'))}>
                    <select
                      value={systemKeysOn ? 'fullscreen' : 'off'}
                      aria-label={t('System shortcuts')}
                      onChange={(e) => { const on = e.target.value === 'fullscreen'; setSystemKeysOn(on); localStorage.setItem('lunaris_system_keys', String(on)); }}
                    >
                      <option value="fullscreen">{t('In full screen only')}</option>
                      <option value="off">{t('Never')}</option>
                    </select>
                  </Row>
                </div>
              </>
            )}

            {category === 'controllers' && (
              <>
                <h3 className="ssd-section">{t('Game controllers')}</h3>
                <div className="ssd-card">
                  <Row title={t('Use controllers in sessions')} hint={t('Controllers connected to this device (PlayStation, Xbox, Nintendo and others) act as Xbox controllers on the remote computer. Windows computers need the ViGEmBus driver. Applies right away.')}>
                    <Switch label={t('Use controllers in sessions')} checked={gamepadOn} onChange={(v) => { setGamepadOn(v); localStorage.setItem('lunaris_gamepad', String(v)); }} />
                  </Row>
                </div>
                <h3 className="ssd-section">{t('Test')}</h3>
                <GamepadTester />
              </>
            )}
          </div>
        </div>

        <footer className="ssd-footer">
          <span className="ssd-footer-note">{footerNote ?? (hostName ? t('Used when you connect to %1.', hostName) : '')}</span>
          <div className="ssd-actions">
            {onUseDefaults && (
              <button
                type="button"
                className="btn-secondary"
                disabled={!ownSettings}
                title={t('Forget the settings this computer has of its own: it uses the default settings again.')}
                onClick={onUseDefaults}
              >{t('Use defaults')}</button>
            )}
            <button type="button" className="btn-secondary" onClick={onCancel}>{t('Cancel')}</button>
            <button type="button" className="btn-primary" onClick={onSave}>{saveLabel ?? t('Save')}</button>
          </div>
        </footer>
      </div>
    </div>
  );
}
