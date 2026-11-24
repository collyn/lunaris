import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { BITRATES, CHROMAS, RESOLUTIONS, bitrateLabel, readDisplayMode, readVolume, objectFitFor, pictureClip, pointerReference, resolutionSize, shownPictureSize, type DisplayMode } from '../streamOptions';
import StreamSettingsDialog, { type StreamSettingsValues, type ViewerSettings } from './StreamSettingsDialog';
import { t } from '../i18n';
import { WebShare, readClipboardSync, readFileTransfer } from '../share';
import { GamepadSender } from '../gamepad';
import FileManager from './FileManager';
import { TouchGestures, type GestureActions, type TouchPoint } from '../touchGestures';
import { accessRefusal } from '../accessRequest';
import { PictureReader, PointerInPictureDrag, looksForPointerInPicture, SentPositions, buildTemplates, matchRegion, searchGroups, type Template } from '../pointerInPicture';
import { relativeMovePacket, takeRelativeStep } from '../relativePointer';
import { gameMousePacket, readGameMouseOn, saveGameMouseOn } from '../gameMouse';
import { canFullscreen, exitFullscreen, exitPointerLock, requestFullscreen } from '../pageLocks';
import { hostPointerInPicture, isStaleCursorSeq, saysPointerAlwaysInPicture, showsHostCursor, showsOwnCursor, streamCursorCss, type PointerView } from '../pointerShown';
import { agentRoundTripMs, decodeStatText, decoderName, encodeStatText, pingStatText, shownPingMs, sourceLatencyMs } from '../streamStats';
import { KEY_COMBOS, PressedKeys, applyKeyboardLock, combosFor, comboStrokes, escapeCanBeKept, keyboardLockFor, readSystemKeys, secureAddress, tapsUnderMeta, vkOfCode } from '../keyboard';
import { hostPrefsKey, overrideOf, prefsFor, sourcePrefsKey, updateComputerPrefs, type ComputerPrefs } from '../streamPrefs';

/** Windows virtual-key codes for the phone keys bar and keyboard keys. */
/** The keys bar above the phone keyboard, in order. The first eleven are the
 *  ones seen without sliding the row. */
const MOBILE_KEYS: { label: string; key?: string; mod?: 'ctrl' | 'alt' | 'shift' | 'meta' }[] = [
  { label: 'Win', mod: 'meta' }, { label: 'Esc', key: 'Escape' }, { label: 'Tab', key: 'Tab' },
  { label: 'Shift', mod: 'shift' }, { label: 'Ctrl', mod: 'ctrl' }, { label: 'Alt', mod: 'alt' },
  { label: 'Del', key: 'Delete' },
  { label: '←', key: 'ArrowLeft' }, { label: '↑', key: 'ArrowUp' }, { label: '↓', key: 'ArrowDown' }, { label: '→', key: 'ArrowRight' },
  { label: 'Home', key: 'Home' }, { label: 'End', key: 'End' }, { label: 'PgUp', key: 'PageUp' }, { label: 'PgDn', key: 'PageDown' },
  { label: 'Ins', key: 'Insert' },
  ...Array.from({ length: 12 }, (_, i) => ({ label: `F${i + 1}`, key: `F${i + 1}` })),
];

const PHONE_KEY_CODES: Record<string, number> = {
  Backspace: 8, Tab: 9, Enter: 13, Escape: 27, PageUp: 33, PageDown: 34, End: 35, Home: 36,
  ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Insert: 45, Delete: 46,
  F1: 112, F2: 113, F3: 114, F4: 115, F5: 116, F6: 117, F7: 118, F8: 119, F9: 120, F10: 121, F11: 122, F12: 123,
};

const getBackendHost = () => {
  const savedHost = localStorage.getItem('lunaris_server_host');
  if (savedHost) {
    return savedHost.replace(/^(https?:\/\/)?/, '').replace(/\/$/, '');
  }
  if (window.location.port === '5173' || window.location.port === '3000') {
    return `${window.location.hostname}:8090`;
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

type HostCursorKind =
  | 'arrow'
  | 'ibeam'
  | 'hand'
  | 'cross'
  | 'move'
  | 'resize_ns'
  | 'resize_ew'
  | 'resize_nesw'
  | 'resize_nwse'
  | 'unavailable'
  | 'unknown';

type HostCursorImagePayload = {
  width: number;
  height: number;
  hotspotX: number;
  hotspotY: number;
  rgba: string;
};

type HostCursorImageMetrics = {
  hotspotX: number;
  hotspotY: number;
  native: boolean;
  kind: HostCursorKind;
};

// Bibata Modern Classic (GPL-3.0), see assets/cursors/README.md.
const HOST_CURSOR_ASSETS: Record<HostCursorKind, { src: string; hotspotX: number; hotspotY: number }> = {
  arrow: { src: '/cursors/arrow.svg', hotspotX: 7, hotspotY: 2 },
  ibeam: { src: '/cursors/ibeam.svg', hotspotX: 16, hotspotY: 16 },
  hand: { src: '/cursors/hand.svg', hotspotX: 14, hotspotY: 2 },
  cross: { src: '/cursors/cross.svg', hotspotX: 16, hotspotY: 16 },
  move: { src: '/cursors/move.svg', hotspotX: 16, hotspotY: 16 },
  resize_ns: { src: '/cursors/resize-ns.svg', hotspotX: 16, hotspotY: 16 },
  resize_ew: { src: '/cursors/resize-ew.svg', hotspotX: 16, hotspotY: 16 },
  resize_nesw: { src: '/cursors/resize-nesw.svg', hotspotX: 16, hotspotY: 16 },
  resize_nwse: { src: '/cursors/resize-nwse.svg', hotspotX: 16, hotspotY: 16 },
  unavailable: { src: '/cursors/unavailable.svg', hotspotX: 16, hotspotY: 16 },
  unknown: { src: '/cursors/arrow.svg', hotspotX: 7, hotspotY: 2 },
};

// Whether a <video> can show part of its frame (object-view-box; not in
// Safari, where the lines a decoder added are clipped off instead).
const VIDEO_CAN_CROP = typeof CSS !== 'undefined' && typeof CSS.supports === 'function' && CSS.supports('object-view-box', 'inset(0px)');

const normalizeHostCursorKind = (kind: unknown): HostCursorKind => {
  return typeof kind === 'string' && kind in HOST_CURSOR_ASSETS
    ? (kind as HostCursorKind)
    : 'arrow';
};

const parseHostCursorImage = (image: any): HostCursorImagePayload | null => {
  if (!image || typeof image !== 'object') return null;
  const width = Number(image.width);
  const height = Number(image.height);
  const hotspotX = Number(image.hotspot_x);
  const hotspotY = Number(image.hotspot_y);
  const rgba = typeof image.rgba === 'string' ? image.rgba : '';
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0 || !rgba) {
    return null;
  }
  if (!Number.isFinite(hotspotX) || !Number.isFinite(hotspotY)) return null;
  return { width, height, hotspotX, hotspotY, rgba };
};


type CodecName = 'h264' | 'h265' | 'av1';

/** The video codecs this browser decodes over WebRTC, as the host is told them. */
const decodableCodecs = (): string[] | undefined => {
  try {
    const caps = typeof RTCRtpReceiver !== 'undefined' && RTCRtpReceiver.getCapabilities
      ? RTCRtpReceiver.getCapabilities('video') : null;
    if (!caps || !caps.codecs) return undefined;
    const names: Record<string, string> = {
      'video/h264': 'h264', 'video/h265': 'h265', 'video/hevc': 'h265', 'video/av1': 'av1', 'video/vp8': 'vp8', 'video/vp9': 'vp9',
    };
    const found: string[] = [];
    for (const codec of caps.codecs) {
      const name = names[codec.mimeType.toLowerCase()];
      if (name && !found.includes(name)) found.push(name);
    }
    return found.length > 0 ? found : undefined;
  } catch {
    return undefined;
  }
};
type CodecChoice = CodecName | 'auto';

interface StreamPlayerProps {
  hostId: string;
  hostName: string;
  /** Where the computer is (its IP addresses, or a source's host:port),
   *  shown beside its name in the settings header. */
  hostAddress?: string;
  token: string;
  serverCodecModeSupport?: number;
  onBack: () => void;
  appId?: number | null;
  /** Another computer this host shows through a connector (a source card):
   *  the session asks for it instead of the host's own screen. */
  sourceId?: string;
  /** false: that source's connector has no files, so none are offered. */
  sourceFiles?: boolean;
  /** A Windows computer's game mouse, as its agent said ("usbip", "none");
   *  undefined: not a Windows computer (src/gameMouse.ts). */
  gameMouse?: string;
  /** The computer is a Windows one (the game mouse setting is shown). */
  windowsHost?: boolean;
}

/**
 * Build a CSS `cursor` value from the host cursor bitmap so the client OS
 * draws the host-shaped pointer as a hardware cursor (native smoothness),
 * instead of a DOM element moved from JavaScript on every mousemove.
 * Returns null when the image cannot be used (browsers cap CSS cursors at
 * 128x128).
 */
const hostCursorImageToCss = (image: HostCursorImagePayload): string | null => {
  if (image.width <= 0 || image.height <= 0 || image.width > 128 || image.height > 128) return null;
  try {
    const binary = atob(image.rgba);
    if (binary.length !== image.width * image.height * 4) return null;
    const bytes = new Uint8ClampedArray(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const ctx = canvas.getContext('2d');
    if (!ctx) return null;
    ctx.putImageData(new ImageData(bytes, image.width, image.height), 0, 0);
    const hx = Math.max(0, Math.min(image.width - 1, Math.round(image.hotspotX)));
    const hy = Math.max(0, Math.min(image.height - 1, Math.round(image.hotspotY)));
    return `url("${canvas.toDataURL('image/png')}") ${hx} ${hy}, default`;
  } catch {
    return null;
  }
};

/** True for focused elements that take typed text (inputs, textareas, contenteditable). */
const isTextEntryElement = (el: Element | null): boolean => {
  if (!el) return false;
  if (el.tagName === 'TEXTAREA' || (el as HTMLElement).isContentEditable) return true;
  if (el.tagName !== 'INPUT') return false;
  const type = ((el as HTMLInputElement).type || 'text').toLowerCase();
  return ['text', 'search', 'email', 'password', 'number', 'url', 'tel'].includes(type);
};

export const StreamPlayer: React.FC<StreamPlayerProps> = ({
  hostId,
  hostName,
  hostAddress,
  token,
  serverCodecModeSupport,
  onBack,
  appId,
  sourceId,
  sourceFiles,
  gameMouse,
  windowsHost,
}) => {
  // The files of what is shown can be offered (a computer of another type
  // only when its connector has files).
  const filesOffered = sourceFiles !== false;
  const videoRef = useRef<HTMLVideoElement | HTMLCanvasElement>(null);
  const hiddenVideoRef = useRef<HTMLVideoElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const pcRef = useRef<RTCPeerConnection | null>(null);
  const channelsRef = useRef<Record<string, RTCDataChannel>>({});
  const scrollXAccumulatorRef = useRef<number>(0);
  const scrollYAccumulatorRef = useRef<number>(0);
  const lastJitterResetTimeRef = useRef<number>(0);
  const wtRef = useRef<any>(null);
  const wtDatagramWriterRef = useRef<any>(null);
  const agentIpsRef = useRef<string[]>([]);
  const wtPortRef = useRef<number | undefined>(undefined);
  const wtCertHashRef = useRef<string | undefined>(undefined);
  const wtTokenRef = useRef<string | undefined>(undefined);
  const wtConnectingRef = useRef<boolean>(false);

  // Mobile gesture and keyboard refs
  const keyboardInputRef = useRef<HTMLInputElement>(null);
  /** When a finger last touched the picture (performance.now()). */
  const lastTouchAtRef = useRef(0);
  const viewportWrapperRef = useRef<HTMLDivElement>(null);
  const localCursorRef = useRef<HTMLDivElement>(null);
  const localCursorImageRef = useRef<HTMLImageElement>(null);
  const localCursorImageMetricsRef = useRef<HostCursorImageMetrics | null>(null);
  const hostCursorRef = useRef<HTMLDivElement>(null);
  const hostCursorImageRef = useRef<HTMLImageElement>(null);
  const hostCursorImageMetricsRef = useRef<HostCursorImageMetrics | null>(null);
  const hasNativeCursorImageRef = useRef<boolean>(false);
  const hostCursorMouseDownRef = useRef<boolean>(false);
  const lastHostCursorLocalPredictionAtRef = useRef<number>(0);
  const cursorHideLogRef = useRef({ since: 0, count: 0 });
  const hostCursorSeqRef = useRef<number | null>(null);
  const hostCursorPosRef = useRef<{ x: number, y: number, visible: boolean, kind: HostCursorKind, inWindowMoveSize: boolean }>({
    x: 0,
    y: 0,
    visible: false,
    kind: 'arrow',
    inWindowMoveSize: false,
  });
  // Finding the host's pointer in the picture (pointerInPicture.ts): a host
  // that cannot know says "the pointer may be in the picture"; while the left
  // button is held the picture is looked at, and the three places that hide
  // the pointers for `inWindowMoveSize` hide for `pointerDragRef.current.inPicture` too.
  const leftButtonHeldRef = useRef<boolean>(false);
  const pointerMaybeInPictureRef = useRef<boolean>(false);
  // The host's pointer is part of its picture for the whole session
  // (`pointer_in_picture: "always"`, pointerShown.ts): no pointer is shown
  // over the picture. The ref for the handlers, the state for the CSS cursor.
  const pointerAlwaysInPictureRef = useRef<boolean>(false);
  const [pointerAlwaysInPicture, setPointerAlwaysInPicture] = useState<boolean>(false);
  const notePointerAlwaysInPicture = (always: boolean): boolean => {
    if (always === pointerAlwaysInPictureRef.current) return false;
    pointerAlwaysInPictureRef.current = always;
    setPointerAlwaysInPicture(always);
    if (always) addLog("The computer's pointer is part of its picture: no pointer is shown over the picture.");
    return true;
  };
  const [pointerDragRef] = useState(() => ({ current: new PointerInPictureDrag() }));
  const [sentPositionsRef] = useState(() => ({ current: new SentPositions() }));
  /** The host cursor image last delivered, and its templates once a drag needed them. */
  const hostCursorImagePayloadRef = useRef<HostCursorImagePayload | null>(null);
  const pointerTemplatesRef = useRef<{ image: HostCursorImagePayload; templates: Template[] } | null>(null);
  const pictureReaderRef = useRef<PictureReader | null>(null);
  const pointerPictureUnreadableRef = useRef<boolean>(false);
  const pointerCheckBusyRef = useRef<boolean>(false);
  const stopPointerCheckLoopRef = useRef<(() => void) | null>(null);
  const updateHostCursorDOMRef = useRef<() => void>(() => {});
  const isHardwareMouseActiveRef = useRef<boolean>(false);
  const localCursorPosRef = useRef<{ x: number, y: number }>({ x: 960, y: 540 });
  const mouseAccumulatorXRef = useRef<number>(0);
  const mouseAccumulatorYRef = useRef<number>(0);
  const latestAbsoluteMousePosRef = useRef<{ clientX: number, clientY: number } | null>(null);
  const mouseSeqRef = useRef<number>(0);
  const mouseFlushTimeoutRef = useRef<any | null>(null);
  // Cache video bounding rect — getBoundingClientRect() forces layout reflow.
  // Updated on resize/fullscreen only, not every mousemove.
  const cachedVideoRectRef = useRef<DOMRect>(new DOMRect());
  const workerRef = useRef<Worker | null>(null);
  const lastCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const canvasTransferredRef = useRef<boolean>(false);
  const canvasReaderRef = useRef<ReadableStreamDefaultReader<VideoFrame> | null>(null);
  const canvasRenderLoopActiveRef = useRef<boolean>(false);

  // Prediction cursor refs — zero-latency local cursor rendering
  const rawPredictionXRef = useRef<number>(-1);
  const rawPredictionYRef = useRef<number>(-1);
  const lastPointerRawUpdateMsRef = useRef<number>(0);
  const accDxRef = useRef<number>(0);
  const accDyRef = useRef<number>(0);
  const touchModeRef = useRef<'direct' | 'trackpad'>('trackpad');
  const useCanvasRendererRef = useRef<boolean>(true);
  const mouseQueueLimitRef = useRef<number>(256);
  const updateVirtualCursorDOMRef = useRef<() => void>(() => {});
  const isPointerLockedRef = useRef<boolean>(false);
  const escapeHoldTimerRef = useRef<number | null>(null);
  const escapeHoldStartRef = useRef<number>(0);
  const escapeHoldLastTickRef = useRef<number>(0);
  const [escapeHoldProgress, setEscapeHoldProgress] = useState<number>(0); // 0 – 1, 0 = not holding

  const [status, setStatus] = useState<string>('Initializing...');
  const [canvasKey, setCanvasKey] = useState<number>(0);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [isPointerLocked, setIsPointerLocked] = useState<boolean>(false);
  // Host-shaped CSS cursor (see hostCursorImageToCss). When set, the OS draws
  // the pointer instead of the JavaScript-driven DOM cursor.
  const [hostCssCursor, setHostCssCursor] = useState<string | null>(null);
  const hostCssCursorRef = useRef<string | null>(null);
  const hostCssCursorNativeRef = useRef<boolean>(false);
  const [appList, setAppList] = useState<{ id: number; title: string; icon_base64?: string | null }[] | null>(null);
  const [currentGameId, setCurrentGameId] = useState<number>(0);
  const [selectedAppId, setSelectedAppId] = useState<number | null>(appId ?? null);
  const [isStoppingStream, setIsStoppingStream] = useState<boolean>(false);
  
  // Settings States: this computer's own (streamPrefs.ts), else this
  // browser's defaults. Changes made in the session are kept for this computer.
  const prefsKey = sourceId ? sourcePrefsKey(hostId, sourceId) : hostPrefsKey(hostId);
  const [initialPrefs] = useState(() => prefsFor(localStorage, prefsKey));
  const rememberPrefs = (change: Partial<ComputerPrefs>) => updateComputerPrefs(localStorage, prefsKey, change);
  const [activeResolution, setActiveResolution] = useState<string>(initialPrefs.resolution);
  const [activeFps, setActiveFps] = useState<number>(initialPrefs.fps);
  const [activeBitrate, setActiveBitrate] = useState<number>(initialPrefs.bitrate);
  const [activeCodec, setActiveCodec] = useState<string>(() => {
    const savedCodec = localStorage.getItem('lunaris_stream_codec') || 'auto';

    // One-time migrations: old builds defaulted or persisted H264 before AV1/HEVC
    // negotiation was wired. Move only H264/default users back to Auto once.
    if (localStorage.getItem('lunaris_codec_auto_reset') !== 'true') {
      localStorage.setItem('lunaris_codec_auto_reset', 'true');
      localStorage.setItem('lunaris_stream_codec', 'auto');
      return 'auto';
    }
    if (localStorage.getItem('lunaris_codec_auto_reset_v2') !== 'true') {
      localStorage.setItem('lunaris_codec_auto_reset_v2', 'true');
      if (savedCodec === 'h264') {
        localStorage.setItem('lunaris_stream_codec', 'auto');
        return overrideOf(localStorage, prefsKey).codec || 'auto';
      }
    }

    return overrideOf(localStorage, prefsKey).codec || savedCodec;
  });
  const [mouseQueueLimit, setMouseQueueLimit] = useState<number>(() => {
    const val = localStorage.getItem('lunaris_mouse_queue_limit');
    if (val === null || val === '0') {
      return 256;
    }
    return Number(val);
  });
  const [activeInputProtocol, setActiveInputProtocol] = useState<string>(() => localStorage.getItem('lunaris_input_protocol') || 'webrtc');

  const [draftResolution, setDraftResolution] = useState<string>(activeResolution);
  const [draftFps, setDraftFps] = useState<number>(activeFps);
  const [draftBitrate, setDraftBitrate] = useState<number>(activeBitrate);
  const [draftCodec, setDraftCodec] = useState<string>(activeCodec);
  const [draftMouseQueueLimit, setDraftMouseQueueLimit] = useState<number>(mouseQueueLimit);
  const [draftInputProtocol, setDraftInputProtocol] = useState<string>(activeInputProtocol);
  const [activeEncoder, setActiveEncoder] = useState<string>(initialPrefs.encoder);
  // A source's display is its own (prefsFor: not the host's default).
  const [activeDisplay, setActiveDisplay] = useState<string>(initialPrefs.display);
  const [draftEncoder, setDraftEncoder] = useState<string>(activeEncoder);
  const [draftDisplay, setDraftDisplay] = useState<string>(activeDisplay);
  // Chroma subsampling: '420' (default, every encoder) or '444' (sharper
  // coloured text; H.264 High 4:4:4 via NVENC or x264 on the host).
  const [activeChroma, setActiveChroma] = useState<'420' | '444'>(initialPrefs.chroma === '444' ? '444' : '420');
  const [draftChroma, setDraftChroma] = useState<'420' | '444'>(activeChroma);
  // 4:4:4 is only negotiated as H.264 High 4:4:4 Predictive (profile-level-id f4…).
  const sessionCodec = (codec: string) => (activeChroma === '444' ? 'h264' : codec);
  const [availableEncoders, setAvailableEncoders] = useState<string[]>([]);
  const [agentGpuInfo, setAgentGpuInfo] = useState<string>('');
  const [agentHostOs, setAgentHostOs] = useState<string>('unknown');
  // For handlers made before the host said its system.
  const agentHostOsRef = useRef<string>('unknown');
  agentHostOsRef.current = agentHostOs;
  // The picture's size (EncoderStatus, else the size asked for) and what of
  // the decoded frame is cropped to show only it (shownPictureSize).
  const pictureSizeRef = useRef<{ width: number; height: number } | null>(null);
  const [pictureCrop, setPictureCrop] = useState<{ right: number; bottom: number }>({ right: 0, bottom: 0 });
  // Where the <video> cannot crop: what of it is clipped (pictureClip).
  const [videoClip, setVideoClip] = useState<{ right: number; bottom: number }>({ right: 0, bottom: 0 });
  const [activeEncoderStatus, setActiveEncoderStatus] = useState<{ encoder: string; hwType: string; gpuInfo: string; requestedEncoder: string; displayId: string; displayName: string }>({
    encoder: 'Pending',
    hwType: 'Unknown',
    gpuInfo: '',
    requestedEncoder: activeEncoder,
    displayId: '',
    displayName: ''
  });
  // The computer shown: the host's own screen ('') or, from a source card,
  // another computer the host shows through a connector. It is given by the
  // card only: a choice remembered by 0.5.0 (lunaris_stream_source_<host>) is
  // not read any more, and is cleared.
  const activeSource = sourceId || '';
  useEffect(() => {
    try { localStorage.removeItem(`lunaris_stream_source_${hostId}`); } catch { /* private mode */ }
  }, [hostId]);
  // A source that changed its video codec asks for the session to be started again.
  const [sourceRestart, setSourceRestart] = useState(0);
  const sourceRestartsRef = useRef(0);
  const lastSourceRestartRef = useRef(0);
  const [availableDisplays, setAvailableDisplays] = useState<{id: string; name: string; width: number; height: number; refresh_rate: number; is_primary: boolean}[]>([]);
  const [isMuted, setIsMuted] = useState<boolean>(() => {
    const val = localStorage.getItem('lunaris_stream_muted');
    return val !== null ? val === 'true' : false;
  });

  const isIOSOrSafari = typeof navigator !== 'undefined' && (
    /iPad|iPhone|iPod/.test(navigator.userAgent) || 
    (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1) ||
    /^((?!chrome|android).)*safari/i.test(navigator.userAgent)
  );

  const [useCanvasRenderer, setUseCanvasRenderer] = useState<boolean>(() => {
    if (isIOSOrSafari) {
      return false;
    }
    const hasProcessor = typeof (window as any).MediaStreamTrackProcessor !== 'undefined';
    // Off unless chosen (see loadHostStreamSettings in App.tsx).
    return hasProcessor && localStorage.getItem('lunaris_canvas_renderer_v2') === 'true';
  });
  const [draftUseCanvasRenderer, setDraftUseCanvasRenderer] = useState<boolean>(useCanvasRenderer);

  // Mobile-specific States
  const [touchMode, setTouchMode] = useState<'direct' | 'trackpad'>(() => {
    return (localStorage.getItem('lunaris_mobile_touch_mode') as 'direct' | 'trackpad') || 'trackpad';
  });
  const [useTouchOffset, setUseTouchOffset] = useState<boolean>(() => {
    return localStorage.getItem('lunaris_mobile_touch_offset') !== 'false';
  });
  const [zoomScale, setZoomScale] = useState<number>(1);
  const [zoomPan, setZoomPan] = useState<{ x: number, y: number }>({ x: 0, y: 0 });
  const [showMobileMenu, setShowMobileMenu] = useState<boolean>(false);
  const [isKeyboardActive, setIsKeyboardActive] = useState<boolean>(false);
  const [isMobileFooterVisible, setIsMobileFooterVisible] = useState<boolean>(true);
  const [modifierKeys, setModifierKeys] = useState<{ ctrl: boolean; alt: boolean; shift: boolean; meta: boolean }>({
    ctrl: false,
    alt: false,
    shift: false,
    meta: false
  });

  const [browserCodecs] = useState<{ h264: boolean; h265: boolean; av1: boolean }>(() => {
    let h264 = true;
    let h265 = false;
    let av1 = false;
    let hasGetCapabilities = false;

    if (typeof RTCRtpReceiver !== 'undefined' && RTCRtpReceiver.getCapabilities) {
      const capabilities = RTCRtpReceiver.getCapabilities('video');
      if (capabilities && capabilities.codecs) {
        hasGetCapabilities = true;
        console.log("WebRTC Video Codec Capabilities:", capabilities.codecs);
        h264 = capabilities.codecs.some(codec => 
          codec.mimeType.toLowerCase() === 'video/h264'
        );
        h265 = capabilities.codecs.some(codec => {
          const mimeType = codec.mimeType.toLowerCase();
          return mimeType === 'video/h265' || mimeType === 'video/hevc';
        });
        av1 = capabilities.codecs.some(codec => 
          codec.mimeType.toLowerCase() === 'video/av1'
        );
      }
    }

    // Only run fallback checks using standard HTML5 video tag support if we couldn't 
    // query the WebRTC receiver capabilities directly. WebRTC capabilities are the 
    // source of truth for real-time video stream decoding support.
    if (isIOSOrSafari && (!hasGetCapabilities || !h265 || !av1) && typeof document !== 'undefined') {
      try {
        const tempVideo = document.createElement('video');
        if (tempVideo && tempVideo.canPlayType) {
          const types = [
            'video/mp4; codecs="hvc1.1.6.L93.B0"',
            'video/mp4; codecs="hev1.1.6.L93.B0"',
            'video/mp4; codecs="hvc1"',
            'video/mp4; codecs="hev1"',
            'video/webm; codecs="hevc"',
            'video/webm; codecs="h265"'
          ];
          h265 = types.some(t => {
            const canPlay = tempVideo.canPlayType(t);
            return canPlay === 'probably' || canPlay === 'maybe';
          });
        }
      } catch (e) {
        console.error("HEVC fallback check error:", e);
      }

      try {
        const tempVideo = document.createElement('video');
        if (tempVideo && tempVideo.canPlayType) {
          const types = [
            'video/mp4; codecs="av01.0.08M.08"',
            'video/webm; codecs="av1"'
          ];
          av1 = types.some(t => {
            const canPlay = tempVideo.canPlayType(t);
            return canPlay === 'probably' || canPlay === 'maybe';
          });
        }
      } catch (e) {
        console.error("AV1 fallback check error:", e);
      }
    }

    return { h264, h265, av1 };
  });

  const hostH264Supported = serverCodecModeSupport === undefined || serverCodecModeSupport === 0 || (serverCodecModeSupport & 262145) !== 0;
  const hostH265Supported = serverCodecModeSupport === undefined || serverCodecModeSupport === 0 || (serverCodecModeSupport & 1573632) !== 0;
  const hostAv1Supported = serverCodecModeSupport !== undefined && serverCodecModeSupport !== 0 && (serverCodecModeSupport & 6488064) !== 0;

  const supportedCodecs = {
    h264: browserCodecs.h264 && hostH264Supported,
    h265: browserCodecs.h265 && hostH265Supported,
    av1: browserCodecs.av1 && hostAv1Supported,
  };

  // Resolve 'auto' to the best available codec: AV1 > H265 > H264
  // Called every time we need the actual codec string to send to the agent.
  const resolveAutoCodec = (): CodecName => {
    if (supportedCodecs.av1) return 'av1';
    if (supportedCodecs.h265) return 'h265';
    return 'h264';
  };

  const codecSupportSummary = (): string => (
    `browser H264=${browserCodecs.h264}, H265=${browserCodecs.h265}, AV1=${browserCodecs.av1}; ` +
    `hostBits=${serverCodecModeSupport ?? 'unknown'}, host H264=${hostH264Supported}, H265=${hostH265Supported}, AV1=${hostAv1Supported}`
  );

  const normalizeCodecChoice = (codec: string): CodecChoice => {
    if (codec === 'h264' || codec === 'h265' || codec === 'av1') return codec;
    return 'auto';
  };

  const getCodecDecision = (requestedCodec: string = activeCodec): { requested: CodecChoice; resolved: CodecName; fellBack: boolean; reason?: string } => {
    const requested = normalizeCodecChoice(requestedCodec);
    // 4:4:4 chroma exists for H.264 only (the desktop app does the same).
    if (activeChroma === '444' && supportedCodecs.h264) {
      return { requested, resolved: 'h264', fellBack: requested !== 'h264' && requested !== 'auto', reason: requested !== 'h264' && requested !== 'auto' ? '4:4:4 chroma is H.264 only' : undefined };
    }
    const resolved = requested === 'auto' ? resolveAutoCodec() : requested;

    if (requested !== 'auto' && !supportedCodecs[resolved]) {
      return {
        requested,
        resolved: resolveAutoCodec(),
        fellBack: true,
        reason: `${requested.toUpperCase()} is not available for this browser/host pair`,
      };
    }

    return { requested, resolved, fellBack: false };
  };

  // For display: when auto, show the resolved codec in parentheses
  const resolvedActiveCodec = getCodecDecision(activeCodec).resolved;

  // Sync activeCodec if a manually-selected codec is no longer supported
  useEffect(() => {
    if (activeCodec === 'auto') return; // auto always resolves dynamically
    const currentCodec = activeCodec as 'h264' | 'h265' | 'av1';
    if (!supportedCodecs[currentCodec]) {
      addLog(`Active codec ${activeCodec} is not supported. Switching to Auto.`);
      setActiveCodec('auto');
      setDraftCodec('auto');
      localStorage.setItem('lunaris_stream_codec', 'auto');
    }
  }, [browserCodecs, serverCodecModeSupport, activeCodec]);

  // Update mouse control refs to avoid stale closures in the animation loop
  useEffect(() => {
    touchModeRef.current = touchMode;
  }, [touchMode]);

  useEffect(() => {
    useCanvasRendererRef.current = useCanvasRenderer;
  }, [useCanvasRenderer]);

  useEffect(() => {
    mouseQueueLimitRef.current = mouseQueueLimit;
  }, [mouseQueueLimit]);

  useEffect(() => {
    updateVirtualCursorDOMRef.current = updateVirtualCursorDOM;
    updateHostCursorDOMRef.current = () => {
      const hostCursor = hostCursorPosRef.current;
      updateHostCursorDOM(hostCursor.x, hostCursor.y, hostCursor.visible, hostCursor.kind);
    };
  });


  const getCodecLabel = (
    codecName: string,
    isBrowserSupported: boolean,
    isHostSupported: boolean
  ) => {
    if (isBrowserSupported && isHostSupported) {
      return codecName;
    }
    if (!isBrowserSupported && !isHostSupported) {
      return `${codecName} (Unsupported by browser & host)`;
    }
    if (!isBrowserSupported) {
      return `${codecName} (Unsupported by browser)`;
    }
    return `${codecName} (Unsupported by host)`;
  };
  
  const [showSettingsModal, setShowSettingsModal] = useState<boolean>(false);
  
  // Synchronize draft states when settings modal is opened
  useEffect(() => {
    if (showSettingsModal) {
      setDraftResolution(activeResolution);
      setDraftFps(activeFps);
      setDraftBitrate(activeBitrate);
      setDraftCodec(activeCodec);
      setDraftMouseQueueLimit(mouseQueueLimit);
      setDraftInputProtocol(activeInputProtocol);
      setDraftUseCanvasRenderer(useCanvasRenderer);
      setDraftEncoder(activeEncoder);
      setDraftDisplay(activeDisplay);
      setDraftChroma(activeChroma);
    }
  }, [showSettingsModal, activeResolution, activeFps, activeBitrate, activeCodec, mouseQueueLimit, activeInputProtocol, useCanvasRenderer, activeEncoder, activeDisplay, activeChroma]);
  const [hideLocalCursor, setHideLocalCursor] = useState<boolean>(() => localStorage.getItem('lunaris_stream_hide_cursor') !== 'false');
  const [isFullscreen, setIsFullscreen] = useState<boolean>(false);
  const [isHeaderVisible, setIsHeaderVisible] = useState<boolean>(true);
  const [isHeaderPinned, setIsHeaderPinned] = useState<boolean>(() => localStorage.getItem('lunaris_header_pinned') === 'true');
  // Stats are on by default, except on phones where they would cover the picture.
  const [showStats, setShowStats] = useState<boolean>(() => {
    const stored = localStorage.getItem('lunaris_show_stats');
    if (stored !== null) return stored !== 'false';
    return !(window.matchMedia?.('(pointer: coarse)').matches && Math.min(window.innerWidth, window.innerHeight) < 600);
  });

  // How the picture fits the window (Display settings): 'fit' keeps the whole
  // picture, 'fill' covers the window (edges cropped), 'stretch' ignores the
  // aspect ratio, 'native' shows one stream pixel per CSS pixel.
  const [displayMode, setDisplayMode] = useState<DisplayMode>(() => readDisplayMode());
  const displayModeRef = useRef<DisplayMode>(displayMode);
  displayModeRef.current = displayMode;
  // Playback volume 0-100 (Audio settings; mute is isMuted).
  const [volume, setVolume] = useState<number>(() => readVolume());

  // Clipboard and file sharing (share.ts); its news shows as a toast.
  const [shareToast, setShareToast] = useState<string>('');
  const shareToastTimerRef = useRef<number | null>(null);
  const shareRef = useRef<WebShare | null>(null);
  const showShareToast = (message: string, ms = 4000) => {
    setShareToast(message);
    if (shareToastTimerRef.current !== null) window.clearTimeout(shareToastTimerRef.current);
    shareToastTimerRef.current = window.setTimeout(() => setShareToast(''), ms);
  };
  if (shareRef.current === null) {
    shareRef.current = new WebShare((message) => showShareToast(message));
  }
  // Game controllers (gamepad.ts), sent on the agent's `gamepad` channel.
  const gamepadRef = useRef<GamepadSender | null>(null);
  if (gamepadRef.current === null) {
    gamepadRef.current = new GamepadSender((message) => showShareToast(t(message), 9000));
  }
  const [clipboardSync, setClipboardSync] = useState<boolean>(readClipboardSync);
  const [fileTransfer, setFileTransfer] = useState<boolean>(readFileTransfer);
  const [isDraggingFiles, setIsDraggingFiles] = useState(false);
  // File manager on its own connection (FileManager.tsx); files dropped on
  // the stream go to the remote computer's Downloads/Lunaris through it.
  const [fileManager, setFileManager] = useState<{ uploads?: File[] } | null>(null);

  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (isTextEntryElement(document.activeElement)) return;
      shareRef.current?.onPaste(e);
    };
    const onFocus = () => { void shareRef.current?.syncOnFocus(); };
    const onPointer = () => shareRef.current?.flushPendingCopy();
    document.addEventListener('paste', onPaste, true);
    window.addEventListener('focus', onFocus);
    window.addEventListener('mousedown', onPointer, true);
    return () => {
      document.removeEventListener('paste', onPaste, true);
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('mousedown', onPointer, true);
      shareRef.current?.detach();
      gamepadRef.current?.detach();
    };
  }, []);

  /** Where the picture lies inside a box of boxW x boxH for the display mode. */
  const fitVideoBox = (boxW: number, boxH: number, vidW: number, vidH: number) => {
    const mode = displayModeRef.current;
    let w = boxW, h = boxH;
    if (mode === 'stretch') {
      w = boxW; h = boxH;
    } else if (mode === 'native') {
      w = vidW; h = vidH;
    } else {
      const scale = mode === 'fill' ? Math.max(boxW / vidW, boxH / vidH) : Math.min(boxW / vidW, boxH / vidH);
      w = vidW * scale; h = vidH * scale;
    }
    return { w, h, x: (boxW - w) / 2, y: (boxH - h) / 2 };
  };

  // Playback volume on whichever element carries the audio.
  useEffect(() => {
    for (const el of [videoRef.current as HTMLVideoElement | null, hiddenVideoRef.current]) {
      if (el) el.volume = volume / 100;
    }
  });
  const [, setIsWebTransportConnected] = useState<boolean>(false);
  const headerTimeoutRef = useRef<any | null>(null);
  const headerBarRef = useRef<HTMLDivElement | null>(null);
  // A <select> is as wide as its longest option, and Firefox's options are wide
  // (long display names, "WebTransport"...). Size each toolbar select to the
  // text it currently shows instead, so they look the same in every browser.
  useLayoutEffect(() => {
    const bar = headerBarRef.current;
    if (!bar) return;
    const ctx = document.createElement('canvas').getContext('2d');
    if (!ctx) return;
    bar.querySelectorAll<HTMLSelectElement>('select.stream-select').forEach((sel) => {
      const cs = getComputedStyle(sel);
      ctx.font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
      const text = sel.selectedOptions[0]?.text ?? '';
      const extra = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight)
        + parseFloat(cs.borderLeftWidth) + parseFloat(cs.borderRightWidth);
      const width = `${Math.ceil(ctx.measureText(text).width + extra) + 2}px`;
      if (sel.style.width !== width) sel.style.width = width;
    });
  });

  // Stats State
  const [stats, setStats] = useState<{
    iceState: string;
    connState: string;
    fps: number;
    decodedFps: number;
    renderFps: number;
    bitrate: number;
    ping: number;
    decodeLatency: number;
    decoder: string;
    jitter: number;
    connectionType: string;
  }>({
    iceState: 'new',
    connState: 'new',
    fps: 0,
    decodedFps: 0,
    renderFps: 0,
    bitrate: 0,
    ping: 0,
    decodeLatency: 0,
    decoder: '',
    jitter: 0,
    connectionType: 'P2P (Direct)'
  });
  // The delay between the agent and the computer it shows through a
  // connector, there and back (ms), as the agent last told it; 0 when it
  // told none. Added to the ping shown; put back to 0 with each session.
  const sourceLatencyRef = useRef<number>(0);
  const renderFpsRef = useRef<number>(0);
  const renderFrameCounterRef = useRef<{ frames: number; lastMs: number }>({
    frames: 0,
    lastMs: performance.now()
  });

  const recordRenderedFrame = () => {
    const now = performance.now();
    const counter = renderFrameCounterRef.current;
    counter.frames += 1;
    const elapsed = now - counter.lastMs;
    if (elapsed >= 1000) {
      renderFpsRef.current = Math.round((counter.frames * 1000) / elapsed);
      counter.frames = 0;
      counter.lastMs = now;
    }
  };

  const addLog = (msg: string) => {
    console.log(`[Lunaris] ${msg}`);
  };

  /** A pointer position sent to the host (stream pixels), kept for the
   *  matcher while the left button is held. */
  const recordSentPointer = (x: number, y: number) => {
    if (leftButtonHeldRef.current) sentPositionsRef.current.record(x, y, performance.now());
  };

  const stopPointerCheckLoop = () => {
    const stop = stopPointerCheckLoopRef.current;
    stopPointerCheckLoopRef.current = null;
    if (stop) stop();
  };

  /** One look at the decoded picture for the host's pointer. */
  const runPointerCheck = async (video: HTMLVideoElement) => {
    const drag = pointerDragRef.current;
    const now = performance.now();
    if (pointerCheckBusyRef.current || !drag.wantsCheck(now)) return;
    if (video.readyState < 2 || video.videoWidth <= 0 || video.videoHeight <= 0) return; // no picture yet

    const image = hostCursorImagePayloadRef.current;
    if (image && pointerTemplatesRef.current?.image !== image) {
      let templates: Template[] = [];
      try {
        const binary = atob(image.rgba);
        if (binary.length === image.width * image.height * 4) {
          const rgba = new Uint8Array(binary.length);
          for (let i = 0; i < binary.length; i += 1) rgba[i] = binary.charCodeAt(i);
          templates = buildTemplates({ width: image.width, height: image.height, hotspotX: image.hotspotX, hotspotY: image.hotspotY, rgba });
        }
      } catch {
        // Not a cursor image that can be looked for.
      }
      pointerTemplatesRef.current = { image, templates };
    }
    const templates = drag.templatesToTry(pointerTemplatesRef.current?.templates ?? []);
    const positions = sentPositionsRef.current.recent(now);
    if (positions.length === 0) {
      positions.push({ x: Math.round(localCursorPosRef.current.x), y: Math.round(localCursorPosRef.current.y) });
    }
    let matched = 0;
    const groups = searchGroups(positions, templates, video.videoWidth, video.videoHeight);
    if (groups.length > 0) {
      if (!pictureReaderRef.current) pictureReaderRef.current = new PictureReader();
      const reader = pictureReaderRef.current;
      const generation = drag.generation;
      pointerCheckBusyRef.current = true;
      let unreadable = false;
      try {
        for (const group of groups) {
          const region = await reader.readVideo(video, group.box);
          if (!region) {
            unreadable = true;
            break;
          }
          matched = matchRegion(region, templates, group.positions);
          if (matched > 0) break;
        }
      } finally {
        pointerCheckBusyRef.current = false;
      }
      if (!drag.active || drag.generation !== generation) return; // that drag is over
      if (unreadable) {
        // The picture cannot be read here: the viewer's pointer stays.
        pointerPictureUnreadableRef.current = true;
        addLog(`pointer in the picture: the picture cannot be read (${reader.error}); the viewer's pointer stays`);
        stopPointerCheckLoop();
        return;
      }
    }
    if (drag.noteCheck(now, matched)) {
      stopPointerCheckLoop();
      updateHostCursorDOMRef.current();
      updateVirtualCursorDOMRef.current();
    }
  };

  /** Looks at each presented frame (at most one per 30 ms) while a drag is on. */
  const startPointerCheckLoop = () => {
    stopPointerCheckLoop();
    if (pointerPictureUnreadableRef.current) return;
    // The decoded picture is always a <video>: the canvas renderer keeps the
    // track on the hidden one as well.
    const video = useCanvasRendererRef.current ? hiddenVideoRef.current : (videoRef.current as HTMLVideoElement | null);
    if (!video || !(video instanceof HTMLVideoElement)) return;
    const hasFrameCallback = 'requestVideoFrameCallback' in video;
    let stopped = false;
    let callbackId = 0;
    let lastFrameAt = performance.now();
    const onFrame = () => {
      if (stopped) return;
      lastFrameAt = performance.now();
      void runPointerCheck(video);
      if (!stopped) callbackId = video.requestVideoFrameCallback(onFrame);
    };
    if (hasFrameCallback) callbackId = video.requestVideoFrameCallback(onFrame);
    // No frame callbacks in this browser, or none coming (a still picture, a
    // video that is not laid out): look at the picture as it is.
    const timerId = window.setInterval(() => {
      if (stopped) return;
      if (!hasFrameCallback || performance.now() - lastFrameAt > 100) void runPointerCheck(video);
    }, 33);
    stopPointerCheckLoopRef.current = () => {
      stopped = true;
      window.clearInterval(timerId);
      if (callbackId && hasFrameCallback) video.cancelVideoFrameCallback(callbackId);
    };
  };

  /** The left button or the host's "maybe" changed: start or end a drag. */
  const syncPointerInPictureDrag = () => {
    const drag = pointerDragRef.current;
    const wasActive = drag.active;
    const wasInPicture = drag.inPicture;
    const line = drag.update(leftButtonHeldRef.current, looksForPointerInPicture(pointerMaybeInPictureRef.current, agentHostOsRef.current));
    if (drag.active && !wasActive) {
      startPointerCheckLoop();
    } else if (!drag.active && wasActive) {
      stopPointerCheckLoop();
      if (line) addLog(line);
      if (wasInPicture) {
        updateHostCursorDOMRef.current();
        updateVirtualCursorDOMRef.current();
      }
    }
  };

  /** Writes the reference of a pointer position (bytes 5..8 of a
   *  mouse_absolute message). Positions are fractions of the shown picture
   *  on a 4096 scale; when the lines a decoder added are cropped
   *  (shownPictureSize, with the edge next to them) the shown part is
   *  smaller than the host's picture, and the scale says so: otherwise the
   *  pointer lands up to two pixels right of and below where it is shown. */
  const writePointerReference = (view: DataView) => {
    const activeVideo = getActiveVideoElement();
    const ref = activeVideo && activeVideo.videoWidth > 0 && activeVideo.videoHeight > 0
      ? pointerReference(activeVideo.videoWidth, activeVideo.videoHeight, pictureSizeRef.current, framesCropped())
      : { width: 4096, height: 4096 };
    view.setInt16(5, ref.width, false);
    view.setInt16(7, ref.height, false);
  };

  const getHostPointFromClient = (clientX: number, clientY: number) => {
    const video = videoRef.current;
    if (!video) return null;

    const activeVideo = getActiveVideoElement();
    const rect = cachedVideoRectRef.current.width > 0 && cachedVideoRectRef.current.height > 0
      ? cachedVideoRectRef.current
      : video.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return null;

    const [vidWidth, vidHeight] = shownSize(activeVideo, video);
    let actualVidWidth = rect.width;
    let actualVidHeight = rect.height;
    let offsetX = 0;
    let offsetY = 0;

    {
      // Picture area for the display mode (fit, fill, stretch, native size).
      const box = fitVideoBox(rect.width, rect.height, vidWidth, vidHeight);
      actualVidWidth = box.w;
      actualVidHeight = box.h;
      offsetX = box.x;
      offsetY = box.y;
    }

    const xLocal = (clientX - rect.left) - offsetX;
    const yLocal = (clientY - rect.top) - offsetY;
    if (xLocal < 0 || yLocal < 0 || xLocal > actualVidWidth || yLocal > actualVidHeight) return null;

    const xNorm = xLocal / actualVidWidth;
    const yNorm = yLocal / actualVidHeight;
    return {
      x: Math.round(xNorm * vidWidth),
      y: Math.round(yNorm * vidHeight),
      x16: Math.round(xNorm * 4096.0),
      y16: Math.round(yNorm * 4096.0),
    };
  };

  const syncPointerLockCursor = () => {
    const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
    if (!mouseAbsoluteChannel || mouseAbsoluteChannel.readyState !== "open") return;

    const latest = latestAbsoluteMousePosRef.current;
    const point = latest ? getHostPointFromClient(latest.clientX, latest.clientY) : null;
    const video = videoRef.current;
    const activeVideo = getActiveVideoElement();
    const [vidWidth, vidHeight] = shownSize(activeVideo, video);
    // Prefer the last confirmed host cursor position as fallback so the
    // virtual cursor starts where the remote cursor actually is, avoiding
    // a visual disconnect when the local mouse was off-video or stale.
    const hostCursor = hostCursorPosRef.current;
    const fallbackX = Math.max(0, Math.min(vidWidth, hostCursor.x));
    const fallbackY = Math.max(0, Math.min(vidHeight, hostCursor.y));
    const targetX = point?.x ?? fallbackX;
    const targetY = point?.y ?? fallbackY;
    const x16 = point?.x16 ?? Math.round((targetX / Math.max(1, vidWidth)) * 4096.0);
    const y16 = point?.y16 ?? Math.round((targetY / Math.max(1, vidHeight)) * 4096.0);

    accDxRef.current = 0;
    accDyRef.current = 0;
    mouseAccumulatorXRef.current = 0;
    mouseAccumulatorYRef.current = 0;
    rawPredictionXRef.current = -1;
    rawPredictionYRef.current = -1;
    localCursorPosRef.current = { x: targetX, y: targetY };
    recordSentPointer(targetX, targetY);
    lastHostCursorLocalPredictionAtRef.current = performance.now();
    updateVirtualCursorDOMRef.current();

    const buffer = new ArrayBuffer(13);
    const view = new DataView(buffer);
    view.setUint8(0, 1);
    view.setInt16(1, x16, false);
    view.setInt16(3, y16, false);
    writePointerReference(view);
    const seq = (mouseSeqRef.current++) >>> 0;
    view.setUint32(9, seq, false);
    mouseAbsoluteChannel.send(buffer);
  };

  useEffect(() => {
    addLog(`[System Diagnostics] RTCPeerConnection supported: ${typeof window.RTCPeerConnection !== 'undefined'}`);
  }, []);

  // The browser's Keyboard Lock (Chromium), decided in one place: every key
  // in full screen when "System shortcuts" is "In full screen only", Esc alone
  // otherwise while in full screen or pointer lock (keyboard.ts).
  const syncKeyboardLock = () => {
    const pointerLocked = !!videoRef.current && document.pointerLockElement === videoRef.current;
    applyKeyboardLock((navigator as any).keyboard, keyboardLockFor(!!document.fullscreenElement, pointerLocked, readSystemKeys()));
  };

  // Where the browser lets go of a locked pointer at the first Esc (an http
  // page; Safari, Firefox), Esc cannot reach the remote computer: said for a
  // while after locking, with this page's https address when the server has
  // one (keyboard.ts escapeCanBeKept, secureAddress).
  const [escapeNotice, setEscapeNotice] = useState<string | null>(null);
  const httpsAddressRef = useRef<string | null>(null);
  useEffect(() => {
    if (window.isSecureContext) return;
    let live = true;
    fetch(`${getBackendProtocol().http}//${getBackendHost()}/api/server/ports`)
      .then((r) => (r.ok ? r.json() : null))
      .then((p) => {
        if (live && p) httpsAddressRef.current = secureAddress(window.location, typeof p.https === 'number' ? p.https : null);
      })
      .catch(() => {});
    return () => { live = false; };
  }, []);
  useEffect(() => {
    if (!isPointerLocked || escapeCanBeKept((navigator as any).keyboard, window.isSecureContext)) {
      setEscapeNotice(null);
      return;
    }
    const address = httpsAddressRef.current;
    const text = window.isSecureContext
      ? t('This browser lets go of the pointer at the first Esc, so Esc does not reach the remote computer. Chrome or Edge keep it for the remote computer.')
      : t('This page is not https, so the browser lets go of the pointer at the first Esc and Esc does not reach the remote computer.')
        + (address ? ' ' + t('Open %1 to keep it.', address) : '');
    setEscapeNotice(text);
    addLog(text);
    const timer = window.setTimeout(() => setEscapeNotice(null), 10000);
    return () => window.clearTimeout(timer);
  }, [isPointerLocked]);

  // Pointer lock change listener
  useEffect(() => {
    const handlePointerLockChange = () => {
      const locked = document.pointerLockElement === videoRef.current;
      setIsPointerLocked(locked);
      isPointerLockedRef.current = locked;
      addLog(locked ? "Pointer locked. Prediction cursor mode." : "Pointer unlocked. Absolute mouse mode.");
      // When pointer is locked, show host cursor (user needs to see it in the stream).
      // When unlocked on mobile trackpad: hide host cursor (SVG virtual cursor shown instead).
      // When unlocked on desktop: keep host cursor visible (browser cursor overlaps it).

      // Lock the Escape key so the browser does not exit pointer lock
      // immediately (our key handlers require a 3‑second hold instead),
      // without taking back the keys locked for full screen.
      syncKeyboardLock();
      // Prediction cursor: show on lock, hide on unlock
      if (locked) {
        syncPointerLockCursor();
      } else {
        // Pointer unlocked — clear any in-progress ESC hold
        if (escapeHoldTimerRef.current !== null) {
          window.clearTimeout(escapeHoldTimerRef.current);
          escapeHoldTimerRef.current = null;
        }
        setEscapeHoldProgress(0);

        // Release any pressed mouse buttons to prevent stuck states on host
        const mouseReliableChannel = channelsRef.current["mouse_reliable"];
        if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
          [1, 2, 3].forEach(button => {
            const buffer = new ArrayBuffer(3);
            const view = new DataView(buffer);
            view.setUint8(0, 2); // Type 2: MouseButton
            view.setUint8(1, 0); // 0 = Release
            view.setUint8(2, button);
            mouseReliableChannel.send(buffer);
          });
        }
      }
    };

    document.addEventListener('pointerlockchange', handlePointerLockChange);
    return () => {
      document.removeEventListener('pointerlockchange', handlePointerLockChange);
    };
  }, [hideLocalCursor, touchMode]);



  // Listen to fullscreen changes (e.g. user presses ESC)
  useEffect(() => {
    const handleFullscreenChange = () => {
      const isFull = !!document.fullscreenElement;
      setIsFullscreen(isFull);
      
      // Browser Keyboard Lock when entering fullscreen (Chromium-based
      // browsers): the key combinations the browser and the system keep
      // (Alt+Tab, the Windows key, Ctrl+W...) come to the page and go to the
      // remote computer; holding Escape leaves full screen, as in client-qml.
      syncKeyboardLock();
      // Keys held across the change are let go on the remote computer
      // (moonlight-qt keyboard.cpp 104-111: "just in case key events get lost").
      releaseAllKeys();
    };
    document.addEventListener('fullscreenchange', handleFullscreenChange);
    return () => {
      document.removeEventListener('fullscreenchange', handleFullscreenChange);
    };
  }, []);

  // Cache video bounding rect — avoids getBoundingClientRect() layout reflow on every mousemove.
  useEffect(() => {
    const invalidateRect = () => {
      if (videoRef.current) {
        cachedVideoRectRef.current = videoRef.current.getBoundingClientRect();
      }
    };
    // Initial compute
    invalidateRect();
    window.addEventListener('resize', invalidateRect);
    document.addEventListener('fullscreenchange', invalidateRect);
    // Also refresh periodically in case of layout changes we didn't catch
    const interval = setInterval(invalidateRect, 1000);
    return () => {
      window.removeEventListener('resize', invalidateRect);
      document.removeEventListener('fullscreenchange', invalidateRect);
      clearInterval(interval);
    };
  }, []);

  // Prediction cursor: pointerrawupdate + pointermove listeners
  // Zero-latency cursor technique:
  // - Prediction cursor renders at full hardware rate (visual smoothness)
  // - Network sends batched to requestAnimationFrame (~60Hz) to avoid SCTP flooding
  //   that competes with video RTP on the shared DTLS/UDP connection.
  //   250 packets/s (old) → ~60 packets/s = 4x reduction in SCTP overhead.
  useEffect(() => {
    if (status !== "Streaming") return;

    const handlePointerLockMove = (event: PointerEvent) => {
      if (!document.pointerLockElement) return;
      accDxRef.current += event.movementX;
      accDyRef.current += event.movementY;
    };

    const onPointerRawUpdate = (event: PointerEvent) => {
      if (!document.pointerLockElement) return;
      if (event.pointerType !== 'mouse') return;
      lastPointerRawUpdateMsRef.current = performance.now();
      event.preventDefault();
      handlePointerLockMove(event);
    };

    const onPointerMove = (event: PointerEvent) => {
      if (!document.pointerLockElement) return;
      if (event.pointerType !== 'mouse') return;
      const rawUpdatedRecently = performance.now() - lastPointerRawUpdateMsRef.current < 100;
      if (rawUpdatedRecently) return;
      event.preventDefault();
      handlePointerLockMove(event);
    };

    (document as any).addEventListener('pointerrawupdate', onPointerRawUpdate, { passive: false });
    document.addEventListener('pointermove', onPointerMove, { passive: false });

    return () => {
      (document as any).removeEventListener('pointerrawupdate', onPointerRawUpdate);
      document.removeEventListener('pointermove', onPointerMove);
    };
  }, [status]);

  // Flush mouse movements at a low-latency cadence.
  // A 4ms interval keeps packets around 250Hz while retaining queue backpressure.
  useEffect(() => {
    if (status !== "Streaming") return;

    let flushTimerId: number;

    const sendTick = () => {
      const qLimit = mouseQueueLimitRef.current;

      // 1. A locked pointer sends how far the mouse moved (packet type 0,
      //    src/relativePointer.ts): games read the mouse themselves in a
      //    match and take only relative moves (an absolute position sent
      //    League of Legends' pointer to the top of the screen). On the
      //    reliable channel: a lost move would leave the pointer behind for
      //    good. Fractions of a pixel are kept for the next send.
      if (accDxRef.current !== 0 || accDyRef.current !== 0) {
        const mouseReliableChannel = channelsRef.current["mouse_reliable"];
        if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
          const buffered = mouseReliableChannel.bufferedAmount;
          if (buffered === undefined || buffered <= qLimit) {
            const step = takeRelativeStep(accDxRef.current, accDyRef.current);
            accDxRef.current = step.restX;
            accDyRef.current = step.restY;
            if (step.dx !== 0 || step.dy !== 0) {
              const seq = (mouseSeqRef.current++) >>> 0;
              mouseReliableChannel.send(relativeMovePacket(step.dx, step.dy, seq));
              // The cursor drawn here is not moved by a guess: the host's
              // pointer acceleration (and a game ignoring a move) make where
              // it lands differ. It follows where the host says its pointer
              // is (host_cursor, below).
            }
          }
        }
      }

      // 2. Process absolute mouse moves (unlocked / trackpad mode)
      const pos = latestAbsoluteMousePosRef.current;
      if (pos) {
        const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
        if (mouseAbsoluteChannel && mouseAbsoluteChannel.readyState === "open" && videoRef.current) {
          const buffered = mouseAbsoluteChannel.bufferedAmount;
          if (buffered === undefined || buffered <= qLimit) {
            latestAbsoluteMousePosRef.current = null;
            const video = videoRef.current;
            const activeVideo = useCanvasRendererRef.current ? hiddenVideoRef.current : (videoRef.current as HTMLVideoElement | null);
            const rect = cachedVideoRectRef.current;
            if (rect.width > 0 && rect.height > 0) {
              const elWidth = rect.width;
              const elHeight = rect.height;
              const [vidWidth, vidHeight] = shownSize(activeVideo, video);


              let actualVidWidth = elWidth;
              let actualVidHeight = elHeight;
              let offsetX = 0;
              let offsetY = 0;

              {
                // Picture area for the display mode (fit, fill, stretch, native size).
                const box = fitVideoBox(elWidth, elHeight, vidWidth, vidHeight);
                actualVidWidth = box.w;
                actualVidHeight = box.h;
                offsetX = box.x;
                offsetY = box.y;
              }

              const xLocal = pos.clientX - rect.left;
              const yLocal = pos.clientY - rect.top;

              let xNorm = (xLocal - offsetX) / actualVidWidth;
              let yNorm = (yLocal - offsetY) / actualVidHeight;

              xNorm = Math.max(0, Math.min(1, xNorm));
              yNorm = Math.max(0, Math.min(1, yNorm));

              // Send as 4096-normalized absolute position (13 bytes)
              const x16 = Math.round(xNorm * 4096.0);
              const y16 = Math.round(yNorm * 4096.0);

              const predictedX = Math.round(xNorm * vidWidth);
              const predictedY = Math.round(yNorm * vidHeight);
              localCursorPosRef.current = { x: predictedX, y: predictedY };
              const shouldUseWindowsPrediction = agentHostOs === "windows" && hasNativeCursorImageRef.current;
              if (touchModeRef.current === 'trackpad' || hideLocalCursor || shouldUseWindowsPrediction) {
                updateVirtualCursorDOMRef.current();
              }
              recordSentPointer(predictedX, predictedY);
              if (!hostPointerInPicture(pointerView())) {
                const hostCursor = hostCursorPosRef.current;
                lastHostCursorLocalPredictionAtRef.current = performance.now();
                updateHostCursorDOM(predictedX, predictedY, true, hostCursor.kind, null);
              }

              const buffer = new ArrayBuffer(13);
              const view = new DataView(buffer);
              view.setUint8(0, 1); // Type 1: MousePosition
              view.setInt16(1, x16, false);
              view.setInt16(3, y16, false);
              writePointerReference(view);
              const seq = (mouseSeqRef.current++) >>> 0;
              view.setUint32(9, seq, false);
              mouseAbsoluteChannel.send(buffer);
            }
          }
        }
      }

    };

    flushTimerId = window.setInterval(sendTick, 4);
    sendTick();
    return () => {
      window.clearInterval(flushTimerId);
    };
  }, [status, hideLocalCursor, agentHostOs]);



  // Auto-hide header menu logic
  useEffect(() => {
    if (status !== "Streaming") {
      setIsHeaderVisible(true);
      return;
    }

    if (showSettingsModal || isHeaderPinned) {
      if (headerTimeoutRef.current) {
        clearTimeout(headerTimeoutRef.current);
        headerTimeoutRef.current = null;
      }
      return;
    }

    // When header becomes visible, start the auto-hide timer
    if (isHeaderVisible) {
      if (headerTimeoutRef.current) {
        clearTimeout(headerTimeoutRef.current);
      }
      headerTimeoutRef.current = setTimeout(() => {
        setIsHeaderVisible(false);
      }, 3000);
    }

    return () => {
      if (headerTimeoutRef.current) {
        clearTimeout(headerTimeoutRef.current);
      }
    };
  }, [isHeaderVisible, isHeaderPinned, showSettingsModal, status]);

  // Sync to live edge on focus, tab visibility change, or mouse enter
  useEffect(() => {
    if (status !== "Streaming") return;

    const handleFocusSync = () => {
      addLog("Stream area active or window focused, synchronizing video stream...");
      const video = getActiveVideoElement();
      if (video) {
        // Ensure play state is active
        video.play().catch(e => console.error("Play error on focus sync:", e));

        // Seek video to latest buffered frame to flush HTMLMediaElement queue
        if (video.buffered.length > 0) {
          try {
            const end = video.buffered.end(video.buffered.length - 1);
            video.currentTime = end;
          } catch (e) {
            // Ignore if seeking on MediaStream is not supported by browser
          }
        }
      }

      // Reset playoutDelayHint and jitterBufferTarget to force WebRTC jitter buffer re-alignment
      if (pcRef.current) {
        pcRef.current.getReceivers().forEach(receiver => {
          try {
            if ('jitterBufferTarget' in receiver) {
              (receiver as any).jitterBufferTarget = 0.05;
            }
            if ('playoutDelayHint' in receiver) {
              (receiver as any).playoutDelayHint = 0.05;
            }
            setTimeout(() => {
              try {
                if ('jitterBufferTarget' in receiver) {
                  (receiver as any).jitterBufferTarget = 0.0;
                }
                if ('playoutDelayHint' in receiver) {
                  (receiver as any).playoutDelayHint = 0.0;
                }
              } catch (e) {}
            }, 50);
          } catch (e) {}
        });
      }
    };

    window.addEventListener("focus", handleFocusSync);
    document.addEventListener("visibilitychange", handleFocusSync);

    return () => {
      window.removeEventListener("focus", handleFocusSync);
      document.removeEventListener("visibilitychange", handleFocusSync);
    };
  }, [status]);



  // Establish WebRTC Signaling Session
  useEffect(() => {
    if (!hostId || !token) return;

    // Resolve the requested codec once before creating SDP and RequestSession.
    const codecDecision = getCodecDecision(sessionCodec(activeCodec));
    const resolvedCodec = codecDecision.resolved;
    addLog(`Codec decision: requested=${codecDecision.requested.toUpperCase()}, resolved=${resolvedCodec.toUpperCase()} (${codecSupportSummary()})`);
    if (codecDecision.fellBack) {
      addLog(`Codec fallback: ${codecDecision.reason}; using ${resolvedCodec.toUpperCase()}.`);
    } else if (codecDecision.requested === 'auto') {
      addLog(`Auto codec resolved to: ${resolvedCodec.toUpperCase()}.`);
    }

    addLog(`Initiating session with host: ${hostName} (${hostId}) using codec ${resolvedCodec}`);
    
    const protocol = getBackendProtocol().ws;
    const host = getBackendHost();
    const wsUrl = `${protocol}//${host}/ws/client?token=${encodeURIComponent(token)}`;
    
    addLog(`Connecting to signaling server...`);
    const ws = new WebSocket(wsUrl);
    wsRef.current = ws;

    ws.onopen = () => {
      if (wsRef.current !== ws) return;
      addLog("Signaling WebSocket connected.");

      // Always request host capabilities (displays & encoders)
      ws.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "GetCapabilities",
          payload: { target_id: hostId }
        }
      }));
      addLog("Sent GetCapabilities command.");

      if (selectedAppId === null) {
        setStatus("Loading Apps...");
        ws.send(JSON.stringify({
          event: "Signaling",
          data: {
            type: "GetAppList",
            payload: { target_id: hostId }
          }
        }));
        addLog("Sent GetAppList command.");
      } else {
        setStatus("Signaling...");
        let width = 1920;
        let height = 1080;
        {
          const size = resolutionSize(activeResolution);
          width = size.width;
          height = size.height;
        }

        // Request session
        pictureSizeRef.current = typeof width === 'number' && typeof height === 'number' ? { width, height } : null;
        ws.send(JSON.stringify({
          event: "Signaling",
          data: {
            type: "RequestSession",
            payload: { 
              host_id: hostId,
              width,
              height,
              fps: activeFps,
              bitrate: activeBitrate,
              codec: resolvedCodec,
              app_id: selectedAppId,
              encoder: activeEncoder !== 'auto' ? activeEncoder : undefined,
              display_id: activeDisplay !== 'default' ? activeDisplay : undefined,
              source_id: activeSource || undefined,
              decode_codecs: decodableCodecs(),
              chroma: activeChroma === '444' ? '444' : undefined,
              microphone: (() => { try { return localStorage.getItem('lunaris_stream_microphone') === 'true'; } catch { return false; } })() ? true : undefined
            }
          }
        }));
        addLog(`Sent RequestSession command for app ${selectedAppId} (res: ${activeResolution}, fps: ${activeFps}, bitrate: ${activeBitrate}Kbps, codec: ${resolvedCodec}, chroma: ${activeChroma}).`);
      }
    };

    ws.onmessage = async (event) => {
      if (wsRef.current !== ws) return;
      try {
        const message = JSON.parse(event.data);
        if (message.event !== "Signaling") return;
        
        const payload = message.data.payload;
        const type = message.data.type;
        
        switch (type) {
          case "AppListResponse":
            addLog("Received App list.");
            setAppList(payload.apps);
            setCurrentGameId(payload.current_game_id);
            setStatus("Select App");
            break;

          case "CapabilitiesResponse":
            addLog(`Received capabilities: ${payload.displays?.length || 0} displays, ${payload.encoders?.length || 0} encoders`);
            // While a source is shown the display list is the source's own
            // (it comes on the general channel as "source_displays").
            if (payload.displays && !activeSource) setAvailableDisplays(payload.displays);
            if (payload.encoders) setAvailableEncoders(payload.encoders);
            if (payload.gpu_info) setAgentGpuInfo(payload.gpu_info);
            if (payload.host_os) setAgentHostOs(String(payload.host_os).toLowerCase());
            break;

          case "EncoderStatus":
            setActiveEncoderStatus({
              encoder: payload.encoder || 'Unknown',
              hwType: payload.hw_type || 'Unknown',
              gpuInfo: payload.gpu_info || agentGpuInfo || '',
              requestedEncoder: payload.requested_encoder || 'auto',
              displayId: payload.display_id || '',
              displayName: payload.display_name || ''
            });
            if (payload.gpu_info) setAgentGpuInfo(payload.gpu_info);
            if (payload.host_os) setAgentHostOs(String(payload.host_os).toLowerCase());
            if (Number(payload.width) > 0 && Number(payload.height) > 0) {
              pictureSizeRef.current = { width: Number(payload.width), height: Number(payload.height) };
              updatePictureCrop();
            }
            addLog(`Agent encoder active: ${payload.encoder || 'unknown'} (${payload.hw_type || 'unknown'}) on ${payload.gpu_info || 'unknown GPU'}${payload.display_id ? ', display: ' + payload.display_id : ''}`);
            break;

          case "StopActiveStreamResponse":
            setIsStoppingStream(false);
            if (payload.success) {
              addLog("Active stream stopped.");
              setCurrentGameId(0);
              // refresh app list
              ws.send(JSON.stringify({
                event: "Signaling",
                data: {
                  type: "GetAppList",
                  payload: { target_id: hostId }
                }
              }));
            } else {
              addLog(`Failed to stop active stream: ${payload.error}`);
              alert(`Failed to stop active stream: ${payload.error || 'Unknown error'}`);
            }
            break;

          case "Sdp":
            if (payload.sdp.ty === "offer") {
              addLog("Received SDP Offer from host agent.");
              await handleSdpOffer(
                payload.target_id,
                payload.sdp.sdp,
                payload.ice_servers,
                payload.webtransport_port,
                payload.webtransport_cert_hash,
                payload.webtransport_token
              );
            }
            break;
            
          case "IceCandidate":
            addLog("Received remote ICE candidate.");
            const candIp = parseCandidateIp(payload.candidate.candidate);
            if (candIp && !agentIpsRef.current.includes(candIp)) {
              agentIpsRef.current.push(candIp);
              addLog(`Found remote candidate IP: ${candIp}`);
            }
            if (pcRef.current) {
              try {
                await pcRef.current.addIceCandidate(new RTCIceCandidate({
                  candidate: payload.candidate.candidate,
                  sdpMid: payload.candidate.sdp_mid,
                  sdpMLineIndex: payload.candidate.sdp_mline_index,
                  usernameFragment: payload.candidate.username_fragment
                }));
              } catch (e) {
                addLog(`Failed to add ICE candidate: ${e}`);
              }
            }
            break;
            
          case "EndSession":
            addLog("Session ended by host.");
            cleanup();
            setStatus("Disconnected");
            setErrorMsg("The remote host ended the streaming session.");
            break;
            
          case "SourceStatus": {
            // About the computer the host shows through a connector.
            const reason = String(payload?.reason || '');
            addLog(`Source ${payload?.status}: ${reason}`);
            // Restarts close together count; one after a while starts the count again.
            if (Date.now() - lastSourceRestartRef.current > 30000) sourceRestartsRef.current = 0;
            if (payload?.status === 'restart' && sourceRestartsRef.current < 3) {
              // Its video codec changed: the same session again finds the new one.
              sourceRestartsRef.current += 1;
              lastSourceRestartRef.current = Date.now();
              setStatus("Connecting...");
              setSourceRestart(n => n + 1);
            } else {
              cleanup();
              setErrorMsg(payload?.status === 'failed'
                ? t('Could not connect to the other computer: %1', t(reason))
                : t('The other computer ended the session: %1', t(reason)));
              setStatus("Error");
            }
            break;
          }

          case "AccessRequest": {
            // Added without its password: the person there accepts or not.
            const refusal = accessRefusal(String(payload?.status || ''));
            addLog(`Access request: ${payload?.status}`);
            if (refusal) {
              cleanup();
              setErrorMsg(refusal);
              setStatus("Error");
            } else if (payload?.status === 'waiting') {
              setStatus(t('Waiting for the person at the remote computer to accept…'));
            }
            break;
          }

          case "Error":
            addLog(`Error from signaling: ${payload.message}`);
            setErrorMsg(payload.message);
            setStatus("Error");
            break;
            
          default:
            addLog(`Unknown signaling message type: ${type}`);
        }
      } catch (err) {
        addLog(`Error parsing WS message: ${err}`);
      }
    };

    ws.onerror = () => {
      if (wsRef.current !== ws) return;
      addLog(`Signaling WebSocket error.`);
      setErrorMsg("WebSocket connection error.");
      setStatus("Error");
    };

    ws.onclose = () => {
      if (wsRef.current !== ws) return;
      addLog("Signaling WebSocket closed.");
      if (status !== "Disconnected" && status !== "Error") {
        setStatus("Disconnected");
      }
    };

    return () => {
      cleanup();
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostId, activeResolution, activeCodec, token, hostName, selectedAppId, activeInputProtocol, useCanvasRenderer, activeEncoder, activeDisplay, activeChroma, activeSource, sourceRestart]);

  // Fetch display/encoder capabilities from agent when host is selected (before stream starts)
  useEffect(() => {
    if (!hostId || !token || status === 'Streaming') return;

    const protocol = getBackendProtocol().ws;
    const host = getBackendHost();
    const wsUrl = `${protocol}//${host}/ws/client?token=${encodeURIComponent(token)}`;

    const ws = new WebSocket(wsUrl);
    ws.onopen = () => {
      ws.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "GetCapabilities",
          payload: { target_id: hostId }
        }
      }));
    };
    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.data?.type === 'CapabilitiesResponse' && msg.data?.payload) {
          const payload = msg.data.payload;
          // A source's displays are its own (sent in its session), not the host's.
          if (payload.displays && !sourceId) setAvailableDisplays(payload.displays);
          if (payload.encoders) setAvailableEncoders(payload.encoders);
          if (payload.gpu_info) setAgentGpuInfo(payload.gpu_info);
        }
      } catch {}
      ws.close();
    };
    ws.onerror = () => ws.close();
    ws.onclose = () => {};

    return () => ws.close();
  }, [hostId, token, status]);

  // Helper to send dynamic pipeline commands via the general data channel.
  // Format: u16 length prefix + JSON string, matching the agent's InboundPacket::deserialize.
  const sendDynamicCommand = (type: string, value: number) => {
    const generalChannel = channelsRef.current["general"];
    if (!generalChannel || generalChannel.readyState !== "open") return;
    const json = JSON.stringify({ type, value });
    const encoder = new TextEncoder();
    const encoded = encoder.encode(json);
    const buffer = new ArrayBuffer(2 + encoded.length);
    const view = new DataView(buffer);
    view.setUint16(0, encoded.length, false); // big-endian u16 length
    new Uint8Array(buffer, 2).set(encoded);
    generalChannel.send(buffer);
    addLog(`[Dynamic] Sent ${type}=${value} via general channel`);
  };

  // Send dynamic bitrate/FPS changes without reconnecting
  useEffect(() => {
    if (status !== "Streaming") return;
    sendDynamicCommand("set_bitrate", activeBitrate);
  }, [activeBitrate, status]);

  useEffect(() => {
    if (status !== "Streaming") return;
    sendDynamicCommand("set_fps", activeFps);
  }, [activeFps, status]);

  // Send keyboard event helper (global/unified)
  const sendKeyEvent = (code: string, shiftKey: boolean, ctrlKey: boolean, altKey: boolean, metaKey: boolean, isDown: boolean) => {
    const keyboardChannel = channelsRef.current["keyboard"];
    if (keyboardChannel && keyboardChannel.readyState === "open") {
      const vk = vkOfCode(code);
      if (vk === 0) return; // Unknown key

      let modifiers = 0;
      if (shiftKey) modifiers |= 1;
      if (ctrlKey) modifiers |= 2;
      if (altKey) modifiers |= 4;
      if (metaKey) modifiers |= 8;

      const buffer = new ArrayBuffer(5);
      const view = new DataView(buffer);
      view.setUint8(0, 0); // Type 0: Key Event
      view.setUint8(1, isDown ? 1 : 0);
      view.setUint8(2, modifiers);
      view.setUint16(3, vk, false); // big-endian
      keyboardChannel.send(buffer);
    }
  };

  const sendRawKeyEvent = (vk: number, isDown: boolean, modifiers: number) => {
    const keyboardChannel = channelsRef.current["keyboard"];
    if (keyboardChannel && keyboardChannel.readyState === "open") {
      const buffer = new ArrayBuffer(5);
      const view = new DataView(buffer);
      view.setUint8(0, 0); // Type 0: Key Event
      view.setUint8(1, isDown ? 1 : 0);
      view.setUint8(2, modifiers);
      view.setUint16(3, vk, false); // big-endian
      keyboardChannel.send(buffer);
    }
  };



  // The keys held on the remote computer, let go there when the page stops
  // getting the keyboard (keyboard.ts PressedKeys).
  const pressedKeysRef = useRef(new PressedKeys());
  const releaseAllKeys = () => {
    for (const [vk, isDown] of pressedKeysRef.current.releaseAll()) sendRawKeyEvent(vk, isDown, 0);
  };

  // "System shortcuts" may have been changed in the settings.
  useEffect(() => {
    if (!showSettingsModal) syncKeyboardLock();
  }, [showSettingsModal]);

  // Global window-level keyboard listeners when streaming is active
  useEffect(() => {
    if (status !== "Streaming") return;

    const isMac = /Mac|iPhone|iPad/.test(navigator.platform || '');

    const handleGlobalKeyDown = (e: KeyboardEvent) => {
      // Only real text-entry fields keep the keyboard. Dropdowns/checkboxes
      // (e.g. the FPS selector in the stream toolbar) stay focused after a
      // change; treating them as text fields used to swallow every key.
      if (isTextEntryElement(document.activeElement)) {
        return;
      }

      if (showSettingsModal || fileManager) return;

      // ESC hold-to-exit for pointer lock (requires keyboard.lock to be active).
      // When the browser has granted keyboard lock, pressing ESC does not
      // immediately exit pointer lock — instead we require a 3‑second hold.
      if (e.code === 'Escape' && isPointerLockedRef.current) {
        e.preventDefault();
        if (escapeHoldTimerRef.current === null) {
          const HOLD_MS = 3000;
          escapeHoldStartRef.current = performance.now();
          escapeHoldLastTickRef.current = escapeHoldStartRef.current;
          setEscapeHoldProgress(0.001); // > 0 so UI knows we started

          const tick = () => {
            const elapsed = performance.now() - escapeHoldStartRef.current;
            if (elapsed >= HOLD_MS) {
              // Hold complete — release pointer lock and fullscreen
              escapeHoldTimerRef.current = null;
              setEscapeHoldProgress(0);
              exitPointerLock();
              exitFullscreen().catch(() => {});
            } else {
              setEscapeHoldProgress(Math.min(0.99, elapsed / HOLD_MS));
              escapeHoldTimerRef.current = window.setTimeout(tick, 50);
            }
          };
          escapeHoldTimerRef.current = window.setTimeout(tick, 50);
        }
        return;
      }

      // Remote text the browser didn't let us copy earlier: a key press may.
      shareRef.current?.flushPendingCopy();
      // macOS sends no release for a key let go under Command: tap it.
      const tap = tapsUnderMeta(isMac, e.metaKey, e.code);
      const vk = vkOfCode(e.code);
      if (vk !== 0 && !tap) pressedKeysRef.current.note(vk, true);
      const sendDown = () => {
        sendKeyEvent(e.code, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, true);
        if (tap) sendKeyEvent(e.code, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, false);
      };
      // Ctrl/Cmd+V: let the browser fire `paste` (its text goes first).
      if (shareRef.current?.holdPasteKey(e, sendDown)) return;
      e.preventDefault();
      if (shareRef.current?.queueAfterPaste(sendDown)) return;
      sendDown();
    };

    const handleGlobalKeyUp = (e: KeyboardEvent) => {
      // Only real text-entry fields keep the keyboard. Dropdowns/checkboxes
      // (e.g. the FPS selector in the stream toolbar) stay focused after a
      // change; treating them as text fields used to swallow every key.
      if (isTextEntryElement(document.activeElement)) {
        return;
      }

      if (showSettingsModal || fileManager) return;

      // Cancel the ESC hold timer on release
      if (e.code === 'Escape' && isPointerLockedRef.current) {
        e.preventDefault();
        if (escapeHoldTimerRef.current !== null) {
          window.clearTimeout(escapeHoldTimerRef.current);
          escapeHoldTimerRef.current = null;
          // Let go before the hold left the lock: an Escape press for the
          // remote computer (a game's menu), sent as a tap.
          sendKeyEvent('Escape', e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, true);
          sendKeyEvent('Escape', e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, false);
        }
        setEscapeHoldProgress(0);
        return;
      }

      e.preventDefault();
      const upVk = vkOfCode(e.code);
      if (upVk !== 0) pressedKeysRef.current.note(upVk, false);
      const sendUp = () => sendKeyEvent(e.code, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, false);
      if (shareRef.current?.queueAfterPaste(sendUp)) return;
      sendUp();
    };

    // Another window or tab has the keyboard now (Alt+Tab, the Windows
    // key, a click elsewhere): the releases will not come here.
    const handleKeyboardLost = () => {
      if (document.visibilityState === 'hidden' || !document.hasFocus()) releaseAllKeys();
    };

    window.addEventListener('keydown', handleGlobalKeyDown);
    window.addEventListener('keyup', handleGlobalKeyUp);
    window.addEventListener('blur', handleKeyboardLost);
    document.addEventListener('visibilitychange', handleKeyboardLost);

    return () => {
      window.removeEventListener('keydown', handleGlobalKeyDown);
      window.removeEventListener('keyup', handleGlobalKeyUp);
      window.removeEventListener('blur', handleKeyboardLost);
      document.removeEventListener('visibilitychange', handleKeyboardLost);
      // The settings or the file manager opened, or the stream ended.
      releaseAllKeys();
    };
  }, [status, showSettingsModal, fileManager]);

  // WebRTC Stats Polling (Ping/RTT, Decode Latency, FPS, Bitrate)
  useEffect(() => {
    if (status !== "Streaming") return;

    let lastDecodeTime = 0;
    let lastFramesDecoded = 0;
    let lastFramesDecodedTimestamp = 0;
    let lastBytesReceived = 0;
    let lastTimestamp = 0;
    let lastJbDelay = 0;
    let lastJbEmitted = 0;

    const interval = setInterval(async () => {
      if (!pcRef.current) return;
      try {
        // Force WebRTC receivers to lowest latency settings
        if (pcRef.current) {
          pcRef.current.getReceivers().forEach(receiver => {
            try {
              if ('jitterBufferTarget' in receiver && (receiver as any).jitterBufferTarget !== 0) {
                (receiver as any).jitterBufferTarget = 0;
              }
              if ('playoutDelayHint' in receiver && (receiver as any).playoutDelayHint !== 0) {
                (receiver as any).playoutDelayHint = 0;
              }
            } catch (e) {}
          });
        }

        // Ensure video play state is active if paused
        const video = getActiveVideoElement();
        if (video && video.paused) {
          video.play().catch(e => console.error("Play error in auto-sync:", e));
        }

        const statsReport = await pcRef.current.getStats();
        let currentRtt = 0;
        let videoDecodeLatency = 0;
        let videoDecoder = '';
        let videoFps = 0;
        let videoDecodedFps = 0;
        let videoBitrate = 0;
        let videoJitter = 0;
        let connectionType = "P2P (Direct)";

        statsReport.forEach((report) => {
          if (report.type === 'candidate-pair' && report.selected === true) {
            const localCandidate = statsReport.get(report.localCandidateId);
            const remoteCandidate = statsReport.get(report.remoteCandidateId);
            if (localCandidate && remoteCandidate) {
              const localType = localCandidate.candidateType;
              const remoteType = remoteCandidate.candidateType;
              if (localType === 'relay' || remoteType === 'relay') {
                connectionType = 'TURN Relay';
              }
            }
          }
          if (report.type === 'candidate-pair') {
            const rtt = report.currentRoundTripTime !== undefined 
              ? report.currentRoundTripTime 
              : (report.roundTripTime !== undefined ? report.roundTripTime : report.googRtt);
            if (rtt !== undefined) {
              const rttVal = Number(rtt);
              if (rttVal > 0) {
                currentRtt = rttVal < 5.0 ? rttVal * 1000.0 : rttVal;
              }
            } else if (report.totalRoundTripTime !== undefined && report.responsesReceived > 0) {
              const avgRtt = Number(report.totalRoundTripTime) / Number(report.responsesReceived);
              if (avgRtt > 0) {
                currentRtt = avgRtt < 5.0 ? avgRtt * 1000.0 : avgRtt;
              }
            }
          } else if (report.type === 'remote-inbound-rtp') {
            const rtt = report.roundTripTime !== undefined ? report.roundTripTime : report.googRtt;
            if (rtt !== undefined) {
              const rttVal = Number(rtt);
              if (rttVal > 0) {
                currentRtt = rttVal < 5.0 ? rttVal * 1000.0 : rttVal;
              }
            }
          } else if (report.type === 'inbound-rtp' && report.kind === 'video') {
            if (report.framesPerSecond !== undefined) {
              videoFps = report.framesPerSecond;
            }
            if (report.jitter !== undefined) {
              videoJitter = Number(report.jitter) * 1000.0;
            }
            // What decodes, as far as the browser tells.
            videoDecoder = decoderName(report.decoderImplementation, report.powerEfficientDecoder);

            // Check for jitter buffer delay drift and auto-reset if it exceeds 60ms
            const jbDelay = report.jitterBufferDelay;
            const jbEmitted = report.jitterBufferEmittedCount;
            if (jbDelay !== undefined && jbEmitted !== undefined && jbEmitted > 0) {
              let avgDelay = 0;
              if (lastJbEmitted > 0 && jbEmitted > lastJbEmitted) {
                const deltaDelay = Number(jbDelay) - lastJbDelay;
                const deltaEmitted = Number(jbEmitted) - lastJbEmitted;
                avgDelay = deltaDelay / deltaEmitted; // in seconds
              } else {
                avgDelay = Number(jbDelay) / Number(jbEmitted);
              }
              lastJbDelay = Number(jbDelay);
              lastJbEmitted = Number(jbEmitted);

              if (avgDelay > 0.120) { // 120ms threshold (prevent false resets on normal 30-40ms stream buffering)
                const now = performance.now();
                if (now - lastJitterResetTimeRef.current > 5000) { // 5s cooldown
                  lastJitterResetTimeRef.current = now;
                  addLog(`Auto-resetting WebRTC jitter buffer (detected drift delay: ${(avgDelay * 1000).toFixed(1)}ms)`);
                  if (pcRef.current) {
                    pcRef.current.getReceivers().forEach(receiver => {
                      if ('playoutDelayHint' in receiver) {
                        (receiver as any).playoutDelayHint = 0.05;
                        setTimeout(() => {
                          (receiver as any).playoutDelayHint = 0;
                        }, 50);
                      }
                    });
                  }
                }
              }
            }

            const bytes = report.bytesReceived || 0;
            const timestamp = report.timestamp;
            if (lastTimestamp > 0 && timestamp > lastTimestamp) {
              const deltaBytes = bytes - lastBytesReceived;
              const deltaTimeMs = timestamp - lastTimestamp;
              videoBitrate = Math.round((deltaBytes * 8) / deltaTimeMs);
            }
            lastBytesReceived = bytes;
            lastTimestamp = timestamp;

            const totalDecodeTime = report.totalDecodeTime || 0;
            const framesDecoded = report.framesDecoded || 0;
            const decodeTimestamp = report.timestamp || performance.now();
            if (lastFramesDecoded > 0 && framesDecoded > lastFramesDecoded) {
              const deltaDecodeTime = totalDecodeTime - lastDecodeTime;
              const deltaFrames = framesDecoded - lastFramesDecoded;
              videoDecodeLatency = (deltaDecodeTime / deltaFrames) * 1000;
              if (lastFramesDecodedTimestamp > 0 && decodeTimestamp > lastFramesDecodedTimestamp) {
                videoDecodedFps = Math.round((deltaFrames * 1000) / (decodeTimestamp - lastFramesDecodedTimestamp));
              }
            }
            lastDecodeTime = totalDecodeTime;
            lastFramesDecoded = framesDecoded;
            lastFramesDecodedTimestamp = decodeTimestamp;
          }
        });

        // The pair in use, not whichever pair the loop above saw last.
        const allReports: any[] = [];
        statsReport.forEach((report) => allReports.push(report));
        const measuredRtt = agentRoundTripMs(allReports);
        if (measuredRtt !== null) currentRtt = measuredRtt;

        setStats(prev => ({
          ...prev,
          // To the agent and back, plus from the agent to the computer it
          // shows and back when it shows another.
          ping: shownPingMs(currentRtt, sourceLatencyRef.current),
          decodeLatency: videoDecodeLatency,
          decoder: videoDecoder || prev.decoder,
          fps: videoFps || prev.fps,
          decodedFps: videoDecodedFps || prev.decodedFps,
          renderFps: renderFpsRef.current || prev.renderFps,
          bitrate: videoBitrate || prev.bitrate,
          jitter: videoJitter || prev.jitter,
          connectionType
        }));
      } catch (err) {
        console.error("Error fetching WebRTC stats:", err);
      }
    }, 1000);

    return () => clearInterval(interval);
  }, [status]);

  useEffect(() => {
    if (status !== "Streaming" || useCanvasRenderer) return;
    const video = getActiveVideoElement();
    if (!video || !("requestVideoFrameCallback" in video)) return;

    let active = true;
    let callbackId = 0;
    const onFrame = () => {
      recordRenderedFrame();
      if (active) {
        callbackId = (video as any).requestVideoFrameCallback(onFrame);
      }
    };
    callbackId = (video as any).requestVideoFrameCallback(onFrame);

    return () => {
      active = false;
      if ("cancelVideoFrameCallback" in video && callbackId) {
        (video as any).cancelVideoFrameCallback(callbackId);
      }
    };
  }, [status, useCanvasRenderer]);

  const parseCandidateIp = (candidateStr: string) => {
    const parts = candidateStr.split(' ');
    if (parts.length >= 5) {
      const ip = parts[4];
      if (ip.match(/^[0-9.]+$/) || ip.includes(':')) {
        return ip;
      }
    }
    return null;
  };

  const hexToUint8Array = (hexString: string) => {
    const clean = hexString.replace(/:/g, '');
    const arr = new Uint8Array(clean.length / 2);
    for (let i = 0; i < clean.length; i += 2) {
      arr[i / 2] = parseInt(clean.substring(i, i + 2), 16);
    }
    return arr;
  };

  const createWebTransportChannelWrapper = (label: string, channelId: number, originalChannel: RTCDataChannel | null) => {
    let pendingWrite: Promise<void> | null = null;

    return {
      label,
      readyState: "open",
      isWrapped: true,
      get bufferedAmount() {
        return 0;
      },
      send: (buffer: any) => {
        if (wtRef.current && wtDatagramWriterRef.current) {
          const bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
          const wtBuffer = new Uint8Array(bytes.byteLength + 1);
          wtBuffer[0] = channelId;
          wtBuffer.set(bytes, 1);
          // Serialize writes: only one in-flight at a time to prevent promise flooding.
          // If a write is still pending, skip this packet (fire-and-forget OK for mouse).
          if (!pendingWrite) {
            pendingWrite = wtDatagramWriterRef.current.write(wtBuffer).then(() => {
              pendingWrite = null;
            }).catch(() => {
              pendingWrite = null;
            });
          }
        } else if (originalChannel && originalChannel.readyState === "open") {
          originalChannel.send(buffer);
        }
      },
      close: () => {
        if (originalChannel) {
          originalChannel.close();
        }
      }
    };
  };

  /** Remote address of the ICE candidate pair in use (not a TURN relay). */
  const selectedRemoteIp = async (): Promise<string | null> => {
    const pc = pcRef.current;
    if (!pc) return null;
    try {
      const stats = await pc.getStats();
      let ip: string | null = null;
      const fromPair = (pair: any) => {
        const remote = pair && stats.get(pair.remoteCandidateId);
        if (remote && remote.candidateType !== 'relay') ip = remote.address || remote.ip || null;
      };
      stats.forEach((s: any) => { if (!ip && s.type === 'transport' && s.selectedCandidatePairId) fromPair(stats.get(s.selectedCandidatePairId)); });
      stats.forEach((s: any) => { if (!ip && s.type === 'candidate-pair' && s.nominated && s.state === 'succeeded') fromPair(s); });
      return ip;
    } catch {
      return null;
    }
  };

  const attemptWebTransport = async () => {
    if (wtRef.current || wtConnectingRef.current) return;
    if (!wtPortRef.current || !wtCertHashRef.current) return;
    
    wtConnectingRef.current = true;
    const port = wtPortRef.current;
    const certHash = wtCertHashRef.current;
    // Newer agents accept only the session's token (hex) as the URL path.
    const token = wtTokenRef.current;
    const path = token && /^[0-9a-fA-F]+$/.test(token) ? `/${token}` : '/';
    
    // The agent address ICE connected to comes first (it is reachable);
    // then the agent's other candidate addresses and the server's host.
    const ips: string[] = [];
    const add = (ip: string | null | undefined) => { if (ip && !ips.includes(ip)) ips.push(ip); };
    add(await selectedRemoteIp());
    agentIpsRef.current.forEach(add);
    add(getBackendHost().split(':')[0]);
    
    addLog(`Attempting WebTransport connection with IPs: ${ips.join(', ')} on port ${port}`);
    
    const hashArray = hexToUint8Array(certHash);
    
    for (const ip of ips) {
      try {
        const url = `https://${ip}:${port}${path}`;
        addLog(`WebTransport: Connecting to https://${ip}:${port}...`);
        const transport = new (window as any).WebTransport(url, {
          serverCertificateHashes: [
            {
              algorithm: 'sha-256',
              value: hashArray
            }
          ]
        });
        
        await transport.ready;
        addLog(`WebTransport: Connected successfully to https://${ip}:${port}!`);
        
        wtRef.current = transport;
        wtDatagramWriterRef.current = transport.datagrams.writable.getWriter();
        setIsWebTransportConnected(true);
        
        // Handle transport close to revert to WebRTC automatically: the
        // channel wrappers send over WebTransport only while wtRef is set.
        const revertToWebRtc = () => {
          if (wtRef.current === transport) {
            wtRef.current = null;
            wtDatagramWriterRef.current = null;
          }
          setIsWebTransportConnected(false);
        };
        transport.closed.then(() => {
          addLog("WebTransport: Connection closed, reverting to WebRTC.");
          revertToWebRtc();
        }).catch(() => {
          revertToWebRtc();
        });
        
        // Wrap existing data channels to send via WebTransport
        // Not the keyboard: a datagram may be lost or arrive out of order,
        // and a lost release leaves the key held on the host. Keys stay on
        // their reliable, ordered data channel.
        const WT_CHANNELS = {
          mouse_absolute: 5,
          mouse_reliable: 4,
          mouse_relative: 6,
        };
        for (const [label, channelId] of Object.entries(WT_CHANNELS)) {
          const existing = channelsRef.current[label];
          if (existing && !(existing as any).isWrapped) {
            channelsRef.current[label] = createWebTransportChannelWrapper(label, channelId, existing) as any;
          }
        }
        
        wtConnectingRef.current = false;
        return;
      } catch (e) {
        addLog(`WebTransport connection to ${ip}:${port} failed: ${e}`);
      }
    }
    
    addLog("WebTransport: All connection attempts failed. Falling back to WebRTC.");
    wtConnectingRef.current = false;
    setIsWebTransportConnected(false);
  };

  const decodeDataChannelText = async (data: any): Promise<string | null> => {
    if (typeof data === 'string') return data;
    if (data instanceof ArrayBuffer) return new TextDecoder().decode(data);
    if (data instanceof Blob) return data.text();
    if (ArrayBuffer.isView(data)) {
      return new TextDecoder().decode(data as ArrayBufferView);
    }
    return null;
  };

  const updateHostCssCursor = (kind: HostCursorKind, image: HostCursorImagePayload | null) => {
    let css: string | null;
    if (image) {
      css = hostCursorImageToCss(image);
      hostCssCursorNativeRef.current = css !== null;
    } else if (hostCssCursorNativeRef.current) {
      return; // shape unchanged; the last native image still applies
    } else {
      const asset = HOST_CURSOR_ASSETS[kind];
      css = `url("${asset.src}") ${asset.hotspotX} ${asset.hotspotY}, default`;
    }
    if (css !== hostCssCursorRef.current) {
      hostCssCursorRef.current = css;
      setHostCssCursor(css);
    }
  };

  const applyNativeCursorImage = (
    imageEl: HTMLImageElement | null,
    metricsRef: React.MutableRefObject<HostCursorImageMetrics | null>,
    kind: HostCursorKind,
    image: HostCursorImagePayload | null,
  ) => {
    if (!image || !imageEl) return;
    try {
      const binary = atob(image.rgba);
      const bytes = new Uint8ClampedArray(binary.length);
      for (let i = 0; i < binary.length; i += 1) {
        bytes[i] = binary.charCodeAt(i);
      }
      if (bytes.length !== image.width * image.height * 4) return;

      const canvas = document.createElement('canvas');
      canvas.width = image.width;
      canvas.height = image.height;
      const ctx = canvas.getContext('2d');
      if (!ctx) return;

      ctx.putImageData(new ImageData(bytes, image.width, image.height), 0, 0);
      imageEl.src = canvas.toDataURL('image/png');
      imageEl.style.width = `${image.width}px`;
      imageEl.style.height = `${image.height}px`;
      imageEl.style.filter = 'none';
      imageEl.style.imageRendering = 'auto';
      hasNativeCursorImageRef.current = true;
      metricsRef.current = {
        hotspotX: image.hotspotX,
        hotspotY: image.hotspotY,
        native: true,
        kind,
      };
    } catch {
      // Keep the previous cursor image if native cursor decoding fails.
    }
  };

  const handleGeneralDataChannelMessage = async (data: any) => {
    const text = await decodeDataChannelText(data);
    if (!text) return;

    try {
      const message = JSON.parse(text);
      if (message?.type === 'source_displays' && Array.isArray(message.displays)) {
        // The displays of the computer shown through a connector.
        setAvailableDisplays(message.displays.map((d: any) => ({
          id: String(d.id), name: String(d.name), width: Number(d.width) || 0, height: Number(d.height) || 0,
          refresh_rate: 0, is_primary: String(d.id) === String(message.current),
        })));
        return;
      }
      const latency = sourceLatencyMs(message);
      if (latency !== null) {
        sourceLatencyRef.current = latency;
        return;
      }
      if (message?.type !== 'host_cursor') return;

      const x = Number(message.x);
      const y = Number(message.y);
      const visible = Boolean(message.visible);
      const kind = normalizeHostCursorKind(message.kind);
      const image = parseHostCursorImage(message.image);
      const inWindowMoveSize = Boolean(message.in_window_move_size);
      if (!Number.isFinite(x) || !Number.isFinite(y)) return;

      // A late message (unordered channel): its position would pull the
      // cursor back. Only a new picture of the cursor is taken from it.
      if (isStaleCursorSeq(hostCursorSeqRef.current, message.seq)) {
        if (image) {
          hostCursorImagePayloadRef.current = image;
          applyNativeCursorImage(localCursorImageRef.current, localCursorImageMetricsRef, kind, image);
          const now = hostCursorPosRef.current;
          updateHostCursorDOM(now.x, now.y, now.visible, kind, image, now.inWindowMoveSize);
        }
        return;
      }
      if (typeof message.seq === 'number') hostCursorSeqRef.current = message.seq;

      // A host that cannot know whether its pointer is in its picture: the
      // picture is looked at while the left button is held (pointerInPicture.ts).
      if (image) hostCursorImagePayloadRef.current = image;
      const alwaysChanged = notePointerAlwaysInPicture(saysPointerAlwaysInPicture(message));
      const maybeInPicture = message.pointer_maybe_in_picture === true;
      if (maybeInPicture !== pointerMaybeInPictureRef.current) {
        pointerMaybeInPictureRef.current = maybeInPicture;
        syncPointerInPictureDrag();
      }

      applyNativeCursorImage(localCursorImageRef.current, localCursorImageMetricsRef, kind, image);
      updateHostCssCursor(kind, image);

      const localPredictionFresh = performance.now() - lastHostCursorLocalPredictionAtRef.current < 140;
      // Always keep the reference position current so that future syncs
      // (e.g. pointer-lock entry) use the real host cursor position.
      hostCursorPosRef.current = { x, y, visible, kind, inWindowMoveSize };
      if (localPredictionFresh && visible && !alwaysChanged) {
        // Prediction is active — skip DOM host-cursor update to avoid jitter.
        // The hostCursorPosRef is already current (set above).
      } else {
        updateHostCursorDOM(x, y, visible, kind, image, inWindowMoveSize);
      }
      updateVirtualCursorDOMRef.current();
    } catch {
      // Ignore non-JSON control messages on the shared general channel.
    }
  };

  const handleSdpOffer = async (
    agentId: string, 
    offerSdp: string, 
    iceServers?: { urls: string[], username?: string, credential?: string }[],
    webtransportPort?: number,
    webtransportCertHash?: string,
    webtransportToken?: string
  ) => {
    setStatus("Establishing WebRTC...");
    
    wtPortRef.current = webtransportPort;
    wtCertHashRef.current = webtransportCertHash;
    wtTokenRef.current = webtransportToken;
    // WebTransport starts once ICE is connected (oniceconnectionstatechange):
    // the selected candidate pair then tells which agent address works.
    
    // Create RTCPeerConnection
    // A new session: what the last one's source said of its delay is not this one's.
    sourceLatencyRef.current = 0;
    const pc = new RTCPeerConnection({
      iceServers: iceServers && iceServers.length > 0 ? iceServers.map(s => ({
        urls: s.urls,
        username: s.username,
        credential: s.credential
      })) : [
        { urls: 'stun:stun.l.google.com:19302' },
        { urls: 'stun:stun1.l.google.com:19302' }
      ]
    });
    pcRef.current = pc;
    (window as any).pc = pc;

    // Track state changes for diagnostic overlay
    pc.oniceconnectionstatechange = () => {
      addLog(`ICE Connection State: ${pc.iceConnectionState}`);
      setStats(prev => ({ ...prev, iceState: pc.iceConnectionState }));
      if (pc.iceConnectionState === 'connected') {
        setStatus("Streaming");
        if (activeInputProtocol === "webtransport" && !wtRef.current && wtPortRef.current && wtCertHashRef.current) {
          attemptWebTransport();
        }
      }
    };

    pc.onconnectionstatechange = () => {
      addLog(`Connection State: ${pc.connectionState}`);
      setStats(prev => ({ ...prev, connState: pc.connectionState }));
      if (pc.connectionState === 'failed') {
        setErrorMsg("WebRTC Connection failed. Check network routing.");
        setStatus("Error");
      }
    };

    // Listen for data channels created by the agent
    pc.ondatachannel = (event) => {
      const channel = event.channel;
      addLog(`Data Channel established: ${channel.label}`);
      
      const label = channel.label;
      if (shareRef.current?.attach(label, channel)) {
        addLog(`Data Channel ${label} ready (sharing).`);
        return;
      }
      if (label === 'gamepad') {
        gamepadRef.current?.attach(channel);
        channel.addEventListener('close', () => gamepadRef.current?.detach());
        addLog('Data Channel gamepad ready (controllers).');
        return;
      }
      // Not the keyboard (see where the WebTransport session is opened).
      const WT_CHANNELS = {
        mouse_absolute: 5,
        mouse_reliable: 4,
        mouse_relative: 6,
      };
      const channelId = WT_CHANNELS[label as keyof typeof WT_CHANNELS];
      if (wtRef.current && channelId !== undefined) {
        channelsRef.current[label] = createWebTransportChannelWrapper(label, channelId, channel) as any;
      } else {
        channelsRef.current[label] = channel;
      }
      channel.onopen = () => {
        addLog(`Data Channel ${channel.label} opened.`);
      };
      channel.onmessage = (messageEvent) => {
        if (label === 'general' || label === 'cursor') {
          void handleGeneralDataChannelMessage(messageEvent.data);
        }
      };
      channel.onclose = () => {
        addLog(`Data Channel ${channel.label} closed.`);
        if (label === 'general' || label === 'cursor') {
          hostCursorImageMetricsRef.current = null;
          localCursorImageMetricsRef.current = null;
          hasNativeCursorImageRef.current = false;
          hostCursorImagePayloadRef.current = null;
          pointerTemplatesRef.current = null;
          notePointerAlwaysInPicture(false);
          if (pointerMaybeInPictureRef.current) {
            pointerMaybeInPictureRef.current = false;
            syncPointerInPictureDrag();
          }
          hostCssCursorRef.current = null;
          hostCssCursorNativeRef.current = false;
          setHostCssCursor(null);
          if (hostCursorImageRef.current) {
            hostCursorImageRef.current.removeAttribute('src');
            hostCursorImageRef.current.style.width = '32px';
            hostCursorImageRef.current.style.height = '32px';
          }
          if (localCursorImageRef.current) {
            localCursorImageRef.current.removeAttribute('src');
            localCursorImageRef.current.style.width = '32px';
            localCursorImageRef.current.style.height = '32px';
          }
          updateHostCursorDOM(0, 0, false, 'arrow', null);
        }
      };
      channel.onerror = (e) => addLog(`Data Channel ${channel.label} error: ${e}`);
    };

    // Handle incoming media tracks
    pc.ontrack = (event) => {
      addLog(`Media track received: ${event.track.kind}`);
      
      if (event.track.kind === 'audio') {
        const targetVideo = useCanvasRenderer ? hiddenVideoRef.current : (videoRef.current as HTMLVideoElement | null);
        if (targetVideo) {
          let stream = targetVideo.srcObject as MediaStream | null;
          if (!stream || !(stream instanceof MediaStream)) {
            stream = new MediaStream();
            targetVideo.srcObject = stream;
          }
          stream.addTrack(event.track);
          targetVideo.play().catch(e => addLog(`Autoplay audio track prevented: ${e}`));
        }
      } else if (event.track.kind === 'video') {
        if (useCanvasRenderer) {
          // Attach video track to hidden video element to force decoding in Chromium
          const targetVideo = hiddenVideoRef.current;
          if (targetVideo) {
            let stream = targetVideo.srcObject as MediaStream | null;
            if (!stream || !(stream instanceof MediaStream)) {
              stream = new MediaStream();
              targetVideo.srcObject = stream;
            }
            stream.addTrack(event.track);
            targetVideo.play().catch(e => addLog(`Autoplay hidden video track prevented: ${e}`));
          }
          startCanvasRender(event.track);
        } else {
          const targetVideo = videoRef.current as HTMLVideoElement | null;
          if (targetVideo) {
            let stream = targetVideo.srcObject as MediaStream | null;
            if (!stream || !(stream instanceof MediaStream)) {
              stream = new MediaStream();
              targetVideo.srcObject = stream;
            }
            stream.addTrack(event.track);
            targetVideo.play().catch(e => addLog(`Autoplay video track prevented: ${e}`));
          }
        }
      }
      
      if (event.receiver) {
        try {
          if ('jitterBufferTarget' in event.receiver) {
            (event.receiver as any).jitterBufferTarget = 0;
            addLog(`Set jitterBufferTarget = 0 on receiver for track kind: ${event.track.kind}`);
          }
          if ('playoutDelayHint' in event.receiver) {
            (event.receiver as any).playoutDelayHint = 0;
            addLog(`Set playoutDelayHint = 0 on receiver for track kind: ${event.track.kind}`);
          }
        } catch (e) {
          addLog(`Error setting receiver latency hints: ${e}`);
        }
      }
    };

    // Send local ICE candidates to agent
    pc.onicecandidate = (event) => {
      if (event.candidate && wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({
          event: "Signaling",
          data: {
            type: "IceCandidate",
            payload: {
              target_id: agentId,
              candidate: {
                candidate: event.candidate.candidate,
                sdp_mid: event.candidate.sdpMid,
                sdp_mline_index: event.candidate.sdpMLineIndex,
                username_fragment: event.candidate.usernameFragment || null
              }
            }
          }
        }));
      }
    };

    console.log("Offer SDP from agent:", offerSdp);

    // Set remote SDP Offer
    await pc.setRemoteDescription(new RTCSessionDescription({
      type: 'offer',
      sdp: offerSdp
    }));
    addLog("Remote description (Offer) set.");

    // Apply codec preference filter using setCodecPreferences.
    // This ensures Chrome's SDP answer only includes the negotiated codec,
    // preventing profile-level-id mismatches when multiple H264 variants exist.
    const codecForSession = getCodecDecision(sessionCodec(activeCodec)).resolved;
    try {
      const transceivers = pc.getTransceivers();
      const videoTransceiver = transceivers.find(t => t.receiver.track.kind === 'video');
      if (videoTransceiver && RTCRtpReceiver.getCapabilities) {
        const caps = RTCRtpReceiver.getCapabilities('video');
        if (caps && caps.codecs) {
          // Build ordered codec preference: put selected codec first, keep RED/ULPFEC/FlexFEC
          // A computer shown through a connector sends what it encodes, which can be
          // VP8 or VP9: the offer then names that codec and the answer must take it.
          const offeredVpx = /^a=rtpmap:\d+ (VP8|VP9)\/90000/im.exec(offerSdp)?.[1].toLowerCase();
          const mimeTarget = offeredVpx ? [`video/${offeredVpx}`]
            : codecForSession === 'h265' ? ['video/h265', 'video/hevc']
            : codecForSession === 'av1' ? ['video/av1']
            : ['video/h264'];
          const preferred = caps.codecs.filter(c =>
            mimeTarget.some(m => c.mimeType.toLowerCase() === m)
          );
          const rest = caps.codecs.filter(c =>
            !mimeTarget.some(m => c.mimeType.toLowerCase() === m) &&
            !['video/vp8', 'video/vp9'].includes(c.mimeType.toLowerCase())
          );
          const ordered = [...preferred, ...rest];
          if (ordered.length > 0 && (videoTransceiver as any).setCodecPreferences) {
            (videoTransceiver as any).setCodecPreferences(ordered);
            addLog(`Set codec preference: ${codecForSession.toUpperCase()} (${preferred.length} variant(s) preferred)`);
          }
        }
      }
    } catch (e) {
      addLog(`setCodecPreferences not supported or failed: ${e}`);
    }

    // Create SDP Answer
    const answer = await pc.createAnswer();
    console.log("Answer SDP from browser:", answer.sdp);
    await pc.setLocalDescription(answer);
    addLog("Local description (Answer) created and set.");

    // Send SDP Answer to Agent
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "Sdp",
          payload: {
            target_id: agentId,
            sdp: {
              ty: "answer",
              sdp: answer.sdp
            },
            ice_servers: null,
            webtransport_port: null,
            webtransport_cert_hash: null
          }
        }
      }));
      addLog("SDP Answer sent back to host agent.");
    }
  };

  const getActiveVideoElement = () => {
    return useCanvasRenderer ? hiddenVideoRef.current : (videoRef.current as HTMLVideoElement | null);
  };

  /** Width and height of the picture in a decoded frame (shownPictureSize:
   *  without the lines a decoder may add, H.265 on Windows), for pointer
   *  coordinates. */
  const shownSize = (activeVideo: any, video: any): [number, number] => {
    const decodedW = activeVideo?.videoWidth > 0 ? activeVideo.videoWidth : video?.width || 1920;
    const decodedH = activeVideo?.videoHeight > 0 ? activeVideo.videoHeight : video?.height || 1080;
    if (!framesCropped()) return [decodedW, decodedH];
    const shown = shownPictureSize(decodedW, decodedH, pictureSizeRef.current);
    return [shown.width, shown.height];
  };

  /** Whether the lines a decoder added are left out of what is shown (the
   *  canvas renderer, or a <video> with object-view-box); else the whole
   *  frame is shown, with them clipped off (videoClip). */
  const framesCropped = () => useCanvasRendererRef.current || VIDEO_CAN_CROP;

  /** Crops what the decoder added to the picture (shownPictureSize), in the
   *  <video> (object-view-box) and in the canvas renderer's worker. */
  const updatePictureCrop = () => {
    const video = getActiveVideoElement();
    const picture = pictureSizeRef.current;
    workerRef.current?.postMessage({ type: 'picture', width: picture?.width || 0, height: picture?.height || 0 });
    if (!video || video.videoWidth <= 0 || video.videoHeight <= 0) return;
    const shown = shownPictureSize(video.videoWidth, video.videoHeight, picture);
    const crop = { right: video.videoWidth - shown.width, bottom: video.videoHeight - shown.height };
    if (!VIDEO_CAN_CROP) {
      const el = videoRef.current;
      const clip = el instanceof HTMLVideoElement
        ? pictureClip({ width: el.clientWidth, height: el.clientHeight }, { width: video.videoWidth, height: video.videoHeight }, picture, objectFitFor(displayModeRef.current))
        : { right: 0, bottom: 0 };
      setVideoClip((old) => (old.right === clip.right && old.bottom === clip.bottom ? old : clip));
    }
    setPictureCrop((old) => {
      if (old.right === crop.right && old.bottom === crop.bottom) return old;
      if (crop.right || crop.bottom) addLog(`The decoder gives ${video.videoWidth}x${video.videoHeight} for a ${shown.width}x${shown.height} picture: showing the picture only`);
      return crop;
    });
  };
  // The clip follows the element's size and the display mode.
  const updatePictureCropRef = useRef(updatePictureCrop);
  updatePictureCropRef.current = updatePictureCrop;
  useEffect(() => {
    if (VIDEO_CAN_CROP || useCanvasRenderer) return;
    const el = videoRef.current;
    if (!(el instanceof HTMLVideoElement) || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => updatePictureCropRef.current());
    observer.observe(el);
    return () => observer.disconnect();
  }, [useCanvasRenderer]);
  useEffect(() => {
    if (!VIDEO_CAN_CROP) updatePictureCropRef.current();
  }, [displayMode]);

  const pictureViewBox = (): React.CSSProperties => {
    if (!VIDEO_CAN_CROP) {
      return videoClip.right || videoClip.bottom
        ? { clipPath: `inset(0px ${videoClip.right}px ${videoClip.bottom}px 0px)` }
        : {};
    }
    return pictureCrop.right || pictureCrop.bottom
      ? ({ objectViewBox: `inset(0px ${pictureCrop.right}px ${pictureCrop.bottom}px 0px)` } as React.CSSProperties)
      : {};
  };

  const startCanvasRender = (track: MediaStreamTrack) => {
    if (workerRef.current) {
      try {
        workerRef.current.terminate();
      } catch (e) {}
      workerRef.current = null;
    }
    if (canvasReaderRef.current) {
      try {
        canvasReaderRef.current.cancel();
      } catch (e) {}
      canvasReaderRef.current = null;
    }

    const canvas = videoRef.current as HTMLCanvasElement | null;
    if (!canvas) {
      addLog("Canvas element not ready for rendering.");
      return;
    }

    addLog("Starting Canvas Web Worker rendering loop...");

    const workerCode = `
      let canvas = null;
      let ctx = null;
      let reader = null;
      let active = false;
      // The picture's size (shownPictureSize in streamOptions.ts): the lines
      // and columns a decoder adds beyond it (at most 64) are not drawn.
      let picture = null;
      const draw = (frame) => {
        let w = frame.displayWidth;
        let h = frame.displayHeight;
        if (picture && picture.width > 0 && picture.height > 0
            && w >= picture.width && h >= picture.height
            && w - picture.width <= 64 && h - picture.height <= 64) {
          // And the line (column) whose colour blends with the padding's.
          if (w > picture.width) w = picture.width - 2;
          if (h > picture.height) h = picture.height - 2;
        }
        if (canvas.width !== w || canvas.height !== h) {
          canvas.width = w;
          canvas.height = h;
          self.postMessage({ type: 'resize', width: w, height: h });
        }
        if (ctx) {
          ctx.drawImage(frame, 0, 0, w, h, 0, 0, w, h);
          self.postMessage({ type: 'frameDrawn' });
        }
      };

      self.onmessage = async (e) => {
        const { type } = e.data;
        if (type === 'picture') {
          picture = e.data.width > 0 && e.data.height > 0 ? { width: e.data.width, height: e.data.height } : null;
          return;
        }
        if (type === 'start') {
          active = true;
          canvas = e.data.canvas;
          if (canvas) {
            ctx = canvas.getContext('2d', { desynchronized: true });
          }
          const readable = e.data.readable;
          if (readable) {
            try {
              reader = readable.getReader();
              while (active) {
                const { done, value: frame } = await reader.read();
                if (done) {
                  break;
                }
                if (!frame) continue;

                try {
                  if (canvas) draw(frame);
                } catch (err) {
                  console.error("Worker drawImage error:", err);
                } finally {
                  frame.close();
                }
              }
            } catch (err) {
              console.error("Worker stream read error:", err);
            }
          }
        } else if (type === 'frame') {
          const frame = e.data.frame;
          if (frame) {
            try {
              if (canvas) draw(frame);
            } catch (err) {
              console.error("Worker drawImage error (fallback):", err);
            } finally {
              frame.close();
            }
          }
        } else if (type === 'stop') {
          active = false;
          if (reader) {
            try {
              reader.cancel();
            } catch (err) {}
            reader = null;
          }
        }
      };
    `;

    const blob = new Blob([workerCode], { type: 'application/javascript' });
    const workerUrl = URL.createObjectURL(blob);
    const worker = new Worker(workerUrl);
    workerRef.current = worker;

    worker.onmessage = (e) => {
      if (e.data && e.data.type === 'resize') {
        updateVideoRect();
      } else if (e.data && e.data.type === 'frameDrawn') {
        recordRenderedFrame();
      }
    };

    let offscreenCanvas: OffscreenCanvas | null = null;
    if (lastCanvasRef.current !== canvas) {
      lastCanvasRef.current = canvas;
      canvasTransferredRef.current = false;
    }

    if (!canvasTransferredRef.current) {
      try {
        offscreenCanvas = canvas.transferControlToOffscreen();
        canvasTransferredRef.current = true;
      } catch (err) {
        console.error("Failed to transfer canvas control to offscreen:", err);
      }
    }

    // Try to create MediaStreamTrackProcessor and obtain its readable stream
    let trackProcessor: any = null;
    let readableStream: any = null;
    try {
      trackProcessor = new (window as any).MediaStreamTrackProcessor({ track });
      readableStream = trackProcessor.readable;
    } catch (err) {
      console.error("Failed to create MediaStreamTrackProcessor:", err);
    }

    // The picture's size, for the frames the decoder makes larger.
    const picture = pictureSizeRef.current;
    worker.postMessage({ type: 'picture', width: picture?.width || 0, height: picture?.height || 0 });

    let streamTransferred = false;
    if (offscreenCanvas && readableStream) {
      try {
        worker.postMessage({
          type: 'start',
          canvas: offscreenCanvas,
          readable: readableStream
        }, [offscreenCanvas, readableStream]);
        streamTransferred = true;
        addLog("Transferred OffscreenCanvas and ReadableStream to Web Worker successfully.");
      } catch (err) {
        console.warn("Failed to transfer ReadableStream directly to worker, using main-thread fallback:", err);
      }
    }

    if (!streamTransferred) {
      // Fallback: Read frames on main thread and forward to worker
      if (offscreenCanvas) {
        worker.postMessage({
          type: 'start',
          canvas: offscreenCanvas
        }, [offscreenCanvas]);
      }

      if (readableStream) {
        canvasRenderLoopActiveRef.current = true;
        const reader = readableStream.getReader();
        canvasReaderRef.current = reader;

        const readFrame = async () => {
          try {
            while (canvasRenderLoopActiveRef.current) {
              const { done, value: frame } = await reader.read();
              if (done) {
                addLog("Canvas track reader stream done.");
                break;
              }
              if (!frame) continue;

              try {
                if (workerRef.current && canvasRenderLoopActiveRef.current) {
                  workerRef.current.postMessage({
                    type: 'frame',
                    frame: frame
                  }, [frame]);
                } else {
                  frame.close();
                }
              } catch (postErr) {
                console.error("Failed to post frame to worker:", postErr);
                frame.close();
              }
            }
          } catch (err) {
            console.error("Error reading video frame from track:", err);
          }
        };

        readFrame();
      }
    }
  };

  const stopCanvasRender = () => {
    canvasRenderLoopActiveRef.current = false;
    if (canvasReaderRef.current) {
      try {
        canvasReaderRef.current.cancel();
      } catch (e) {}
      canvasReaderRef.current = null;
    }
    if (workerRef.current) {
      try {
        workerRef.current.postMessage({ type: 'stop' });
        workerRef.current.terminate();
      } catch (e) {}
      workerRef.current = null;
    }
    canvasTransferredRef.current = false;
    lastCanvasRef.current = null;
    setCanvasKey(prev => prev + 1);
  };

  const cleanup = () => {
    // Clear mouse flush timer
    if (mouseFlushTimeoutRef.current) {
      clearTimeout(mouseFlushTimeoutRef.current);
      mouseFlushTimeoutRef.current = null;
    }

    // 1. Send EndSession signal if websocket is open
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "EndSession",
          payload: { target_id: hostId }
        }
      }));
    }

    // Close WebSocket
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }

    // Close Data Channels
    Object.values(channelsRef.current).forEach(ch => ch.close());
    channelsRef.current = {};

    // Close Peer Connection
    if (pcRef.current) {
      pcRef.current.close();
      pcRef.current = null;
    }
    sourceLatencyRef.current = 0;

    // Clear video src and stop canvas loop
    if (videoRef.current && videoRef.current instanceof HTMLVideoElement) {
      videoRef.current.srcObject = null;
    }
    if (hiddenVideoRef.current) {
      hiddenVideoRef.current.srcObject = null;
    }
    if (pointerMaybeInPictureRef.current) {
      pointerMaybeInPictureRef.current = false;
      syncPointerInPictureDrag();
    }
    notePointerAlwaysInPicture(false);
    stopPointerCheckLoop();
    pointerPictureUnreadableRef.current = false;
    stopCanvasRender();
    
    // Close WebTransport
    if (wtDatagramWriterRef.current) {
      try {
        wtDatagramWriterRef.current.releaseLock();
      } catch (e) {}
      wtDatagramWriterRef.current = null;
    }
    if (wtRef.current) {
      try {
        wtRef.current.close();
      } catch (e) {}
      wtRef.current = null;
    }
    setIsWebTransportConnected(false);
    agentIpsRef.current = [];

    exitPointerLock();
  };

  // Request pointer lock for relative controls.
  // navigator.keyboard.lock (required for the 3 s ESC hold) only takes
  // effect while fullscreen is active, so we enter fullscreen alongside
  // pointer lock.  The fullscreenchange handler (which fires shortly
  // after) calls keyboard.lock to intercept ESC.
  // The game mouse (src/gameMouse.ts): the viewer's choice for this
  // computer, sent to its agent when the stream starts and when it changes.
  const [gameMouseOn, setGameMouseOnState] = useState<boolean>(() => readGameMouseOn(hostId));
  const gameMouseActive = gameMouseOn && gameMouse === 'usbip' && !sourceId;
  const gameMouseActiveRef = useRef(gameMouseActive);
  useEffect(() => { gameMouseActiveRef.current = gameMouseActive; }, [gameMouseActive]);
  const setGameMouseOn = (on: boolean) => {
    saveGameMouseOn(hostId, on);
    setGameMouseOnState(on);
  };
  useEffect(() => {
    if (status !== "Streaming" || gameMouse !== 'usbip' || sourceId) return;
    const send = () => {
      const channel = channelsRef.current["mouse_reliable"];
      if (channel && channel.readyState === "open") channel.send(gameMousePacket(gameMouseActive));
    };
    send();
    // Said again while it is on: a new session (a codec change reconnects)
    // starts with it off on the agent, and the end of the previous one
    // turns it off; the agent does nothing when nothing changes.
    if (!gameMouseActive) return;
    const timer = window.setInterval(send, 2000);
    return () => window.clearInterval(timer);
  }, [status, gameMouseActive, gameMouse, sourceId]);

  const togglePointerLock = () => {
    if (!videoRef.current) return;
    if (document.pointerLockElement === videoRef.current) {
      exitPointerLock();
    } else {
      // Enter fullscreen in parallel — keyboard.lock requires it.
      if (!document.fullscreenElement && containerRef.current) {
        requestFullscreen(containerRef.current).catch(() => {});
      }
      if (typeof videoRef.current.requestPointerLock !== 'function') return;
      const promise = (videoRef.current as any).requestPointerLock({
        unadjustedMovement: true,
      });
      if (promise && (promise as any).catch) {
        (promise as any).catch(() => {
          videoRef.current?.requestPointerLock();
        });
      }
    }
  };

  // ---- Settings dialog (StreamSettingsDialog) ----
  // This device's display and sound: applied right away and remembered.
  const viewerSettings: ViewerSettings = {
    displayMode,
    setDisplayMode: (m) => { setDisplayMode(m); localStorage.setItem('lunaris_display_mode', m); },
    muted: isMuted,
    setMuted: (m) => { setIsMuted(m); localStorage.setItem('lunaris_stream_muted', String(m)); },
    volume,
    setVolume: (v) => { setVolume(v); localStorage.setItem('lunaris_stream_volume', String(v)); },
    showStats,
    setShowStats: (v) => { setShowStats(v); localStorage.setItem('lunaris_show_stats', String(v)); },
    clipboardSync,
    setClipboardSync: (v) => { setClipboardSync(v); localStorage.setItem('lunaris_clipboard_sync', String(v)); },
    fileTransfer,
    setFileTransfer: (v) => { setFileTransfer(v); localStorage.setItem('lunaris_file_transfer', String(v)); },
  };

  const draftSettingsValues: StreamSettingsValues = {
    resolution: draftResolution,
    fps: draftFps,
    codec: draftCodec,
    chroma: draftChroma,
    bitrate: draftBitrate,
    mouseQueueLimit: draftMouseQueueLimit,
    inputProtocol: draftInputProtocol,
    encoder: draftEncoder,
    display: draftDisplay,
    useCanvasRenderer: draftUseCanvasRenderer,
  };

  const setDraftSettingValue = <K extends keyof StreamSettingsValues>(key: K, value: StreamSettingsValues[K]) => {
    switch (key) {
      case 'resolution': setDraftResolution(value as string); break;
      case 'fps': setDraftFps(value as number); break;
      case 'codec': setDraftCodec(value as string); break;
      case 'chroma': setDraftChroma(value === '444' ? '444' : '420'); break;
      case 'bitrate': setDraftBitrate(value as number); break;
      case 'mouseQueueLimit': setDraftMouseQueueLimit(value as number); break;
      case 'inputProtocol': setDraftInputProtocol(value as string); break;
      case 'encoder': setDraftEncoder(value as string); break;
      case 'display': setDraftDisplay(value as string); break;
      case 'useCanvasRenderer': setDraftUseCanvasRenderer(value as boolean); break;
    }
  };

  const cancelDraftSettings = () => {
    setDraftResolution(activeResolution);
    setDraftFps(activeFps);
    setDraftBitrate(activeBitrate);
    setDraftCodec(activeCodec);
    setDraftMouseQueueLimit(mouseQueueLimit);
    setDraftInputProtocol(activeInputProtocol);
    setDraftUseCanvasRenderer(useCanvasRenderer);
    setDraftEncoder(activeEncoder);
    setDraftDisplay(activeDisplay);
    setDraftChroma(activeChroma);
    setShowSettingsModal(false);
  };

  const applyDraftSettings = () => {
    // The picture's settings are kept for this computer; the rest are this browser's.
    rememberPrefs({
      resolution: draftResolution,
      fps: draftFps,
      bitrate: draftBitrate,
      codec: draftCodec,
      chroma: draftChroma,
      encoder: draftEncoder,
      display: draftDisplay,
    });
    localStorage.setItem('lunaris_mouse_queue_limit', String(draftMouseQueueLimit));
    localStorage.setItem('lunaris_input_protocol', draftInputProtocol);
    localStorage.setItem('lunaris_canvas_renderer_v2', String(draftUseCanvasRenderer));
    
    // Check if settings requiring reconnect changed
    const reconnectNeeded = 
      draftResolution !== activeResolution ||
      draftCodec !== activeCodec ||
      draftInputProtocol !== activeInputProtocol ||
      draftUseCanvasRenderer !== useCanvasRenderer ||
      draftEncoder !== activeEncoder ||
      draftDisplay !== activeDisplay ||
      draftChroma !== activeChroma;

    setMouseQueueLimit(draftMouseQueueLimit);

    // Always update FPS/bitrate (dynamic, no reconnect needed)
    setActiveFps(draftFps);
    setActiveBitrate(draftBitrate);

    if (reconnectNeeded) {
      // Update active values to trigger reconnect
      setActiveResolution(draftResolution);
      setActiveCodec(draftCodec);
      setActiveInputProtocol(draftInputProtocol);
      setUseCanvasRenderer(draftUseCanvasRenderer);
      setActiveEncoder(draftEncoder);
      setActiveDisplay(draftDisplay);
      setActiveChroma(draftChroma);
      
      // Reset states for connection indicator
      setStatus("Connecting...");
    }
    
    setShowSettingsModal(false);
  };

  const updateSetting = (key: string, value: any) => {
    if (key === 'res') {
      rememberPrefs({ resolution: value });
      setActiveResolution(value);
      setDraftResolution(value);
      if (value !== activeResolution) setStatus("Connecting...");
    } else if (key === 'fps') {
      const numValue = Number(value);
      rememberPrefs({ fps: numValue });
      setActiveFps(numValue);
      setDraftFps(numValue);
      // Dynamic: no reconnect needed, useEffect sends command via data channel
    } else if (key === 'bitrate') {
      const numValue = Number(value);
      rememberPrefs({ bitrate: numValue });
      setActiveBitrate(numValue);
      setDraftBitrate(numValue);
      // Dynamic: no reconnect needed, useEffect sends command via data channel
    } else if (key === 'codec') {
      rememberPrefs({ codec: value });
      setActiveCodec(value);
      setDraftCodec(value);
      if (value !== activeCodec) setStatus("Connecting...");
    } else if (key === 'chroma') {
      const chroma = value === '444' ? '444' : '420';
      rememberPrefs({ chroma });
      setActiveChroma(chroma);
      setDraftChroma(chroma);
      if (chroma !== activeChroma) setStatus("Connecting...");
    } else if (key === 'mouseQueueLimit') {
      const numValue = Number(value);
      localStorage.setItem('lunaris_mouse_queue_limit', String(numValue));
      setMouseQueueLimit(numValue);
      setDraftMouseQueueLimit(numValue);
    } else if (key === 'inputProtocol') {
      localStorage.setItem('lunaris_input_protocol', value);
      setActiveInputProtocol(value);
      setDraftInputProtocol(value);
      if (value !== activeInputProtocol) setStatus("Connecting...");
    } else if (key === 'display') {
      rememberPrefs({ display: value });
      setActiveDisplay(value);
      setDraftDisplay(value);
      if (value !== activeDisplay) setStatus("Connecting...");
    }
  };

  // What decides which pointers are shown (pointerShown.ts), as it is now.
  const pointerView = (over: Partial<PointerView> = {}): PointerView => ({
    streaming: status === 'Streaming',
    hostOs: agentHostOs,
    alwaysInPicture: pointerAlwaysInPictureRef.current,
    buttonHeld: hostCursorMouseDownRef.current,
    inWindowMoveSize: hostCursorPosRef.current.inWindowMoveSize,
    foundInPicture: pointerDragRef.current.inPicture,
    hostCursorVisible: hostCursorPosRef.current.visible,
    hardwareMouse: isHardwareMouseActiveRef.current,
    trackpad: touchModeRef.current === 'trackpad',
    hideLocalCursor,
    pointerLocked: isPointerLockedRef.current,
    hostCssCursor: hostCssCursorRef.current,
    hasNativeCursorImage: hasNativeCursorImageRef.current,
    gameMouse: gameMouseActiveRef.current && isPointerLockedRef.current,
    ...over,
  });

  // Diagnostics (2026-11-24, a cursor blinking with the game mouse): a drawn
  // cursor that was shown and is hidden says why in the console, a few times
  // a second at most. `at` names the check in updateHostCursorDOM /
  // updateVirtualCursorDOM that hid it.
  const hideCursorFor = (el: HTMLElement, at: number) => {
    if (el.style.display === 'none') return;
    el.style.display = 'none';
    const log = cursorHideLogRef.current;
    const now = performance.now();
    if (now - log.since > 1000) { log.since = now; log.count = 0; }
    if (++log.count > 4) return;
    const v = pointerView();
    console.info(`[Lunaris] ${el.className} hidden at ${at}: gameMouse=${!!v.gameMouse} locked=${v.pointerLocked} hostVisible=${v.hostCursorVisible} hardwareMouse=${v.hardwareMouse} buttonHeld=${v.buttonHeld} inPicture=${hostPointerInPicture(v)} nativeImage=${v.hasNativeCursorImage} kind=${hostCursorPosRef.current.kind}`);
  };

  const updateHostCursorDOM = (
    x: number,
    y: number,
    visible: boolean,
    kind: HostCursorKind = 'arrow',
    image: HostCursorImagePayload | null = null,
    inWindowMoveSize: boolean = hostCursorPosRef.current.inWindowMoveSize,
  ) => {
    const cursorEl = hostCursorRef.current;
    const imageEl = hostCursorImageRef.current;
    const video = videoRef.current;
    const wrapper = viewportWrapperRef.current;
    const cursorKind = normalizeHostCursorKind(kind);
    hostCursorPosRef.current = { x, y, visible, kind: cursorKind, inWindowMoveSize };

    // Touchpad on a phone: only the cursor drawn here (shaped like the
    // host's) is shown, as with a mouse on a PC. When no finger is on the
    // screen it follows the host's cursor, so the two don't drift apart.
    if (touchModeRef.current === 'trackpad' && !isHardwareMouseActiveRef.current) {
      if (cursorEl) hideCursorFor(cursorEl, 3290);
      if (visible && performance.now() - lastTouchAtRef.current > 600) {
        localCursorPosRef.current = { x, y };
      }
      updateVirtualCursorDOMRef.current();
      return;
    }

    // A locked pointer moves by relative steps: the cursor drawn here is
    // where the host says its pointer is (no guess, see the send above).
    if (document.pointerLockElement && visible) {
      localCursorPosRef.current = { x, y };
      updateVirtualCursorDOMRef.current();
    }

    if (!cursorEl || !video || !wrapper || !showsHostCursor(pointerView())) {
      if (cursorEl) hideCursorFor(cursorEl, 3306);
      return;
    }

    applyNativeCursorImage(imageEl, hostCursorImageMetricsRef, cursorKind, image);
    let cursorMetrics = hostCursorImageMetricsRef.current;

    if (cursorMetrics?.native && cursorMetrics.kind !== cursorKind && image) {
      cursorMetrics = null;
      hostCursorImageMetricsRef.current = null;
    }

    if (!cursorMetrics || !cursorMetrics.native) {
      const allowAssetFallback = !hasNativeCursorImageRef.current;
      if (!allowAssetFallback) {
        hideCursorFor(cursorEl, 3321);
        return;
      }

      const cursorAsset = HOST_CURSOR_ASSETS[cursorKind];
      if (imageEl && !imageEl.src.endsWith(cursorAsset.src)) {
        imageEl.src = cursorAsset.src;
        imageEl.style.width = '32px';
        imageEl.style.height = '32px';
        imageEl.style.filter = 'none'; // the cursors carry their own white outline
      }
      cursorMetrics = {
        hotspotX: cursorAsset.hotspotX,
        hotspotY: cursorAsset.hotspotY,
        native: false,
        kind: cursorKind,
      };
      hostCursorImageMetricsRef.current = cursorMetrics;
    }

    const rect = video.getBoundingClientRect();
    const wrapperRect = wrapper.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0 || wrapperRect.width <= 0 || wrapperRect.height <= 0) {
      hideCursorFor(cursorEl, 3344);
      return;
    }

    const activeVideo = getActiveVideoElement();
    const [vidWidth, vidHeight] = shownSize(activeVideo, video);

    let actualVidWidth = rect.width;
    let actualVidHeight = rect.height;
    let offsetX = 0;
    let offsetY = 0;

    {
      // Picture area for the display mode (fit, fill, stretch, native size).
      const box = fitVideoBox(rect.width, rect.height, vidWidth, vidHeight);
      actualVidWidth = box.w;
      actualVidHeight = box.h;
      offsetX = box.x;
      offsetY = box.y;
    }

    const xNorm = Math.max(0, Math.min(1, x / vidWidth));
    const yNorm = Math.max(0, Math.min(1, y / vidHeight));
    const clientX = xNorm * actualVidWidth + offsetX + rect.left;
    const clientY = yNorm * actualVidHeight + offsetY + rect.top;

    const left = clientX - wrapperRect.left - cursorMetrics.hotspotX;
    const top = clientY - wrapperRect.top - cursorMetrics.hotspotY;
    cursorEl.style.transform = `translate3d(${left}px, ${top}px, 0)`;
    cursorEl.style.display = 'block';
  };

  useEffect(() => {
    const releaseHostCursorSuppression = () => {
      if (leftButtonHeldRef.current) {
        leftButtonHeldRef.current = false;
        if (pointerDragRef.current.active) syncPointerInPictureDrag();
      }
      if (!hostCursorMouseDownRef.current) return;
      hostCursorMouseDownRef.current = false;
      const hostCursor = hostCursorPosRef.current;
      updateHostCursorDOM(hostCursor.x, hostCursor.y, hostCursor.visible, hostCursor.kind);
      updateVirtualCursorDOMRef.current();
    };

    window.addEventListener('mouseup', releaseHostCursorSuppression);
    window.addEventListener('blur', releaseHostCursorSuppression);
    return () => {
      window.removeEventListener('mouseup', releaseHostCursorSuppression);
      window.removeEventListener('blur', releaseHostCursorSuppression);
    };
  }, []);

  const updateVideoRect = () => {
    updateVirtualCursorDOM();
    const hostCursor = hostCursorPosRef.current;
    updateHostCursorDOM(hostCursor.x, hostCursor.y, hostCursor.visible, hostCursor.kind);
  };

  const updateVirtualCursorDOM = () => {
    const cursorEl = localCursorRef.current;
    const imageEl = localCursorImageRef.current;
    const video = videoRef.current;
    const wrapper = viewportWrapperRef.current;
    if (!cursorEl || !video || !wrapper) return;

    const hostCursor = hostCursorPosRef.current;
    if (!showsOwnCursor(pointerView())) {
      hideCursorFor(cursorEl, 3412);
      return;
    }

    const rect = video.getBoundingClientRect();
    const wrapperRect = wrapper.getBoundingClientRect();

    if (rect.width <= 0 || rect.height <= 0 || wrapperRect.width <= 0 || wrapperRect.height <= 0) {
      hideCursorFor(cursorEl, 3420);
      return;
    }

    const activeVideo = getActiveVideoElement();
    const [vidWidth, vidHeight] = shownSize(activeVideo, video);

    const elWidth = rect.width;
    const elHeight = rect.height;

    let actualVidWidth = elWidth;
    let actualVidHeight = elHeight;
    let offsetX = 0;
    let offsetY = 0;


    {
      // Picture area for the display mode (fit, fill, stretch, native size).
      const box = fitVideoBox(elWidth, elHeight, vidWidth, vidHeight);
      actualVidWidth = box.w;
      actualVidHeight = box.h;
      offsetX = box.x;
      offsetY = box.y;
    }

    const cursorKind = normalizeHostCursorKind(hostCursor.kind);
    let cursorMetrics = localCursorImageMetricsRef.current;
    if (cursorMetrics?.native && cursorMetrics.kind !== cursorKind) {
      // Keep the previous native image briefly instead of flashing back to a PNG asset.
      // The next host_cursor image update will replace it with the exact new native shape.
    }
    if (!cursorMetrics || !cursorMetrics.native) {
      const allowAssetFallback = !hasNativeCursorImageRef.current;
      if (!allowAssetFallback) {
        hideCursorFor(cursorEl, 3454);
        return;
      }

      const cursorAsset = HOST_CURSOR_ASSETS[cursorKind];
      if (imageEl && !imageEl.src.endsWith(cursorAsset.src)) {
        imageEl.src = cursorAsset.src;
        imageEl.style.width = '32px';
        imageEl.style.height = '32px';
        imageEl.style.filter = 'none'; // the cursors carry their own white outline
      }
      cursorMetrics = {
        hotspotX: cursorAsset.hotspotX,
        hotspotY: cursorAsset.hotspotY,
        native: false,
        kind: cursorKind,
      };
      localCursorImageMetricsRef.current = cursorMetrics;
    }

    const xNorm = Math.max(0, Math.min(1, localCursorPosRef.current.x / vidWidth));
    const yNorm = Math.max(0, Math.min(1, localCursorPosRef.current.y / vidHeight));

    const clientX = xNorm * actualVidWidth + offsetX + rect.left;
    const clientY = yNorm * actualVidHeight + offsetY + rect.top;

    const leftWrapper = clientX - wrapperRect.left - cursorMetrics.hotspotX;
    const topWrapper = clientY - wrapperRect.top - cursorMetrics.hotspotY;

    cursorEl.style.transform = `translate3d(${leftWrapper}px, ${topWrapper}px, 0)`;
    cursorEl.style.display = 'block';
  };

  const handleVideoLoadedMetadata = () => {
    updatePictureCrop();
    const video = getActiveVideoElement();
    if (video) {
      localCursorPosRef.current = {
        x: video.videoWidth > 0 ? Math.round(video.videoWidth / 2) : 960,
        y: video.videoHeight > 0 ? Math.round(video.videoHeight / 2) : 540
      };
      updateVirtualCursorDOM();
    }
  };

  useEffect(() => {
    updateVirtualCursorDOM();
  }, [zoomScale, zoomPan, touchMode, status, hideLocalCursor]);

  useEffect(() => {
    window.addEventListener("resize", updateVideoRect);
    window.addEventListener("scroll", updateVideoRect, true);
    return () => {
      window.removeEventListener("resize", updateVideoRect);
      window.removeEventListener("scroll", updateVideoRect, true);
    };
  }, []);

  // Mobile Touch Helpers & Handlers
  const sendTouchAbsolutePos = (clientX: number, clientY: number) => {
    const video = videoRef.current;
    const activeVideo = getActiveVideoElement();
    const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
    if (video && mouseAbsoluteChannel && mouseAbsoluteChannel.readyState === "open") {
      const rect = video.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;

      const xLocal = clientX - rect.left;
      const adjustedClientY = useTouchOffset ? (clientY - 40) : clientY;
      const yLocal = adjustedClientY - rect.top;

      const elWidth = rect.width;
      const elHeight = rect.height;
      const [vidWidth, vidHeight] = shownSize(activeVideo, video);

      let xNorm = 0.5;
      let yNorm = 0.5;


      let actualVidWidth = elWidth;
      let actualVidHeight = elHeight;
      let offsetX = 0;
      let offsetY = 0;

      {
        // Picture area for the display mode (fit, fill, stretch, native size).
        const box = fitVideoBox(elWidth, elHeight, vidWidth, vidHeight);
        actualVidWidth = box.w;
        actualVidHeight = box.h;
        offsetX = box.x;
        offsetY = box.y;
      }

      xNorm = (xLocal - offsetX) / actualVidWidth;
      yNorm = (yLocal - offsetY) / actualVidHeight;

      xNorm = Math.max(0, Math.min(1, xNorm));
      yNorm = Math.max(0, Math.min(1, yNorm));

      const x16 = Math.round(xNorm * 4096.0);
      const y16 = Math.round(yNorm * 4096.0);

      // Keep local cursor position ref synchronized
      localCursorPosRef.current = { x: Math.round(xNorm * vidWidth), y: Math.round(yNorm * vidHeight) };
      if (touchMode === "trackpad") {
        updateVirtualCursorDOM();
      }

      const buffer = new ArrayBuffer(13);
      const view = new DataView(buffer);
      view.setUint8(0, 1); // Type 1: MousePosition
      view.setInt16(1, x16, false);
      view.setInt16(3, y16, false);
      writePointerReference(view);
      const seq = (mouseSeqRef.current++) >>> 0;
      view.setUint32(9, seq, false);
      mouseAbsoluteChannel.send(buffer);
    }
  };

  const sendMouseClickAction = (button: number, isDown: boolean) => {
    const mouseReliableChannel = channelsRef.current["mouse_reliable"];
    if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
      const buffer = new ArrayBuffer(3);
      const view = new DataView(buffer);
      view.setUint8(0, 2); // Type 2: MouseButton
      view.setUint8(1, isDown ? 1 : 0);
      view.setUint8(2, button);
      mouseReliableChannel.send(buffer);
    }
  };

  // ---- Touch control (touchGestures.ts, like RustDesk's mobile mouse) ----
  // Zoom as last rendered (the element's on-screen box matches it) and as
  // last requested by a gesture (state updates land on the next render).
  const renderedZoomRef = useRef({ scale: 1, pan: { x: 0, y: 0 } });
  const viewZoomRef = useRef({ scale: 1, pan: { x: 0, y: 0 } });
  renderedZoomRef.current = { scale: zoomScale, pan: zoomPan };
  viewZoomRef.current = { scale: zoomScale, pan: zoomPan };
  const pinchStartRef = useRef<{ scale: number; pan: { x: number; y: number }; mid: { x: number; y: number } } | null>(null);
  const touchScrollRef = useRef({ x: 0, y: 0 });

  /** The picture's layout without zoom, in screen pixels. */
  const touchLayout = () => {
    const video = videoRef.current;
    const wrapper = viewportWrapperRef.current;
    if (!video || !wrapper) return null;
    const rect = video.getBoundingClientRect();
    const r = renderedZoomRef.current;
    if (rect.width <= 0 || rect.height <= 0 || r.scale <= 0) return null;
    const activeVideo = getActiveVideoElement();
    const [vidW, vidH] = shownSize(activeVideo, video);
    const baseW = rect.width / r.scale;
    const baseH = rect.height / r.scale;
    return {
      originX: rect.left - r.pan.x,
      originY: rect.top - r.pan.y,
      box: fitVideoBox(baseW, baseH, vidW, vidH),
      vidW,
      vidH,
      wrapper: wrapper.getBoundingClientRect(),
    };
  };

  /** Puts the zoom on the picture element now (the state follows on the next
   *  render with the same value), so the cursor drawn right after a move is
   *  placed against the picture as it will be shown. */
  const applyZoomNow = (scale: number, pan: { x: number; y: number }) => {
    viewZoomRef.current = { scale, pan };
    renderedZoomRef.current = { scale, pan };
    const el = videoRef.current as HTMLElement | null;
    if (el) el.style.transform = `translate(${pan.x}px, ${pan.y}px) scale(${scale})`;
    setZoomScale(scale);
    setZoomPan(pan);
  };

  /** Sets the zoom, keeping the picture over the viewport. */
  const setTouchView = (scale: number, pan: { x: number; y: number }) => {
    const L = touchLayout();
    if (!L || scale <= 1) {
      applyZoomNow(1, { x: 0, y: 0 });
      return;
    }
    const clampAxis = (value: number, origin: number, start: number, size: number, viewLo: number, viewHi: number) => {
      // Picture edges on screen: origin + pan + scale * start ... + scale * size.
      const a = viewLo - origin - scale * start;
      const b = viewHi - scale * size - origin - scale * start;
      return Math.max(Math.min(a, b), Math.min(Math.max(a, b), value));
    };
    const clamped = {
      x: clampAxis(pan.x, L.originX, L.box.x, L.box.w, L.wrapper.left, L.wrapper.right),
      y: clampAxis(pan.y, L.originY, L.box.y, L.box.h, L.wrapper.top, L.wrapper.bottom),
    };
    applyZoomNow(scale, clamped);
  };

  /** Touchpad mode, zoomed in: the cursor stays in the middle of the view and
   *  the picture moves under it (like RustDesk). At the picture's edges the
   *  view stops and the cursor goes on to the edge. */
  const centerViewOnCursor = (scale: number) => {
    const L = touchLayout();
    if (!L || scale <= 1) {
      setTouchView(1, { x: 0, y: 0 });
      return;
    }
    const cur = localCursorPosRef.current;
    const cx = (L.wrapper.left + L.wrapper.right) / 2;
    const cy = (L.wrapper.top + L.wrapper.bottom) / 2;
    setTouchView(scale, {
      x: cx - L.originX - scale * (L.box.x + (cur.x / L.vidW) * L.box.w),
      y: cy - L.originY - scale * (L.box.y + (cur.y / L.vidH) * L.box.h),
    });
  };
  const touchpadOwnsView = () => touchModeRef.current === 'trackpad' && !isHardwareMouseActiveRef.current;

  const sendMouseAbsoluteNorm = (xNorm: number, yNorm: number) => {
    const channel = channelsRef.current["mouse_absolute"];
    if (!channel || channel.readyState !== "open") return;
    const buffer = new ArrayBuffer(13);
    const view = new DataView(buffer);
    view.setUint8(0, 1); // Type 1: MousePosition
    view.setInt16(1, Math.round(Math.max(0, Math.min(1, xNorm)) * 4096), false);
    view.setInt16(3, Math.round(Math.max(0, Math.min(1, yNorm)) * 4096), false);
    writePointerReference(view);
    view.setUint32(9, (mouseSeqRef.current++) >>> 0, false);
    channel.send(buffer);
  };

  const sendScrollSteps = (dx: number, dy: number) => {
    const channel = channelsRef.current["mouse_reliable"];
    if (!channel || channel.readyState !== "open" || (dx === 0 && dy === 0)) return;
    const buffer = new ArrayBuffer(3);
    const view = new DataView(buffer);
    view.setUint8(0, 4); // Type 4: Scroll
    view.setInt8(1, Math.max(-127, Math.min(127, dx)));
    view.setInt8(2, Math.max(-127, Math.min(127, dy)));
    channel.send(buffer);
  };

  const touchActions: GestureActions = {
    moveBy: (dx, dy) => {
      const L = touchLayout();
      if (!L) return;
      const z = viewZoomRef.current;
      // The cursor moves as far as the finger on the picture, a bit
      // further on fast swipes.
      const speed = Math.hypot(dx, dy);
      const gain = speed > 6 ? Math.min(2, 1 + (speed - 6) * 0.05) : 1;
      const hostPerPx = L.vidW / (z.scale * L.box.w);
      const cur = localCursorPosRef.current;
      const x = Math.max(0, Math.min(L.vidW - 1, cur.x + dx * gain * hostPerPx));
      const y = Math.max(0, Math.min(L.vidH - 1, cur.y + dy * gain * hostPerPx));
      localCursorPosRef.current = { x, y };
      sendMouseAbsoluteNorm(x / L.vidW, y / L.vidH);
      // Zoomed in: the picture moves, the cursor stays in the middle.
      if (z.scale > 1) centerViewOnCursor(z.scale);
      updateVirtualCursorDOM();
    },
    moveTo: (x, y) => sendTouchAbsolutePos(x, y),
    button: (button, down) => sendMouseClickAction(button, down),
    scroll: (dx, dy) => {
      // A notch per 30 px of finger travel; the content follows the fingers.
      const acc = touchScrollRef.current;
      acc.x -= dx;
      acc.y += dy;
      const sx = Math.trunc(acc.x / 30);
      const sy = Math.trunc(acc.y / 30);
      acc.x -= sx * 30;
      acc.y -= sy * 30;
      sendScrollSteps(sx, sy);
    },
    pan: (dx, dy) => {
      const z = viewZoomRef.current;
      if (z.scale > 1) setTouchView(z.scale, { x: z.pan.x + dx, y: z.pan.y + dy });
    },
    pinchStart: (mid) => {
      const z = viewZoomRef.current;
      pinchStartRef.current = { scale: z.scale, pan: { ...z.pan }, mid };
    },
    pinch: (ratio, midNow) => {
      const start = pinchStartRef.current;
      const L = touchLayout();
      if (!start || !L) return;
      const scale = Math.max(1, Math.min(5, start.scale * ratio));
      if (touchpadOwnsView()) {
        // Touchpad: zoom around the cursor, which stays in the middle.
        centerViewOnCursor(scale);
        updateVirtualCursorDOM();
        return;
      }
      // The picture point under the fingers when the pinch began stays
      // under them (and follows them as they move).
      const lx = (start.mid.x - L.originX - start.pan.x) / start.scale;
      const ly = (start.mid.y - L.originY - start.pan.y) / start.scale;
      setTouchView(scale, { x: midNow.x - L.originX - scale * lx, y: midNow.y - L.originY - scale * ly });
    },
    pinchEnd: () => { pinchStartRef.current = null; },
    haptic: (ms) => { try { navigator.vibrate?.(ms); } catch { /* not supported */ } },
  };
  const touchActionsRef = useRef(touchActions);
  touchActionsRef.current = touchActions;
  const gesturesRef = useRef<TouchGestures | null>(null);
  if (!gesturesRef.current) {
    // Calls through the ref so the gestures always use this render's helpers.
    const forward = {} as GestureActions;
    (Object.keys(touchActions) as (keyof GestureActions)[]).forEach((k) => {
      (forward as any)[k] = (...args: unknown[]) => (touchActionsRef.current as any)[k](...args);
    });
    gesturesRef.current = new TouchGestures(touchMode, forward);
  }

  useEffect(() => {
    const g = gesturesRef.current;
    if (!g) return;
    g.reset();
    g.mode = touchMode;
  }, [touchMode]);

  useEffect(() => {
    const wrapper = viewportWrapperRef.current;
    if (!wrapper) return;

    const bypassSelectors = '.stream-header-bar, .stream-header-pull-tab, .mobile-footer-bar, .mobile-footer-pull-tab, .mobile-controls-drawer, .stream-settings-overlay, .mobile-keys-bar';
    const points = (list: TouchList): TouchPoint[] =>
      Array.from(list).map((t) => ({ id: t.identifier, x: t.clientX, y: t.clientY }));
    const accept = (e: TouchEvent) => {
      if (e.target && (e.target as HTMLElement).closest(bypassSelectors)) return false;
      if (status !== "Streaming") return false;
      // No emulated mouse events, page scrolling or browser zoom.
      e.preventDefault();
      return true;
    };
    const onStart = (e: TouchEvent) => {
      if (!accept(e)) return;
      lastTouchAtRef.current = performance.now();
      isHardwareMouseActiveRef.current = false;
      gesturesRef.current?.start(points(e.touches));
    };
    const onMove = (e: TouchEvent) => {
      if (!accept(e)) return;
      lastTouchAtRef.current = performance.now();
      gesturesRef.current?.move(points(e.touches));
    };
    const onEnd = (e: TouchEvent) => {
      if (!accept(e)) return;
      lastTouchAtRef.current = performance.now();
      gesturesRef.current?.end(points(e.touches));
    };
    const onCancel = (e: TouchEvent) => {
      if (accept(e)) gesturesRef.current?.end(points(e.touches), true);
    };

    wrapper.addEventListener('touchstart', onStart, { passive: false });
    wrapper.addEventListener('touchmove', onMove, { passive: false });
    wrapper.addEventListener('touchend', onEnd, { passive: false });
    wrapper.addEventListener('touchcancel', onCancel, { passive: false });

    return () => {
      wrapper.removeEventListener('touchstart', onStart);
      wrapper.removeEventListener('touchmove', onMove);
      wrapper.removeEventListener('touchend', onEnd);
      wrapper.removeEventListener('touchcancel', onCancel);
      gesturesRef.current?.reset();
    };
  }, [status]);

  // Mobile Virtual Keyboard Handlers
  // ---- Phone keyboard ----
  // Modifiers on the keys bar apply to the next key (then let go).
  const modifierKeysRef = useRef(modifierKeys);
  modifierKeysRef.current = modifierKeys;

  const sendTextRemote = (text: string) => {
    const keyboardChannel = channelsRef.current["keyboard"];
    if (!keyboardChannel || keyboardChannel.readyState !== "open" || !text) return;
    const encodedText = new TextEncoder().encode(text);
    const buffer = new ArrayBuffer(3 + encodedText.length);
    const view = new DataView(buffer);
    view.setUint8(0, 1); // Type 1: Text Event
    view.setUint16(1, encodedText.length, false);
    new Uint8Array(buffer, 3).set(encodedText);
    keyboardChannel.send(buffer);
  };

  /** Presses and releases `vk` with the keys bar's modifiers (plus `extra`). */
  const pressKeyRemote = (vk: number, extra: Partial<typeof modifierKeys> = {}) => {
    const m = { ...modifierKeysRef.current };
    (Object.keys(extra) as (keyof typeof m)[]).forEach((k) => { if (extra[k]) m[k] = true; });
    const held: [boolean, number, number][] = [[m.ctrl, 17, 2], [m.alt, 18, 4], [m.shift, 16, 1], [m.meta, 91, 8]];
    let bits = 0;
    for (const [on, mvk, bit] of held) {
      if (!on) continue;
      bits |= bit;
      sendRawKeyEvent(mvk, true, bits);
    }
    sendRawKeyEvent(vk, true, bits);
    sendRawKeyEvent(vk, false, bits);
    for (const [on, mvk, bit] of [...held].reverse()) {
      if (!on) continue;
      bits &= ~bit;
      sendRawKeyEvent(mvk, false, bits);
    }
    if (m.ctrl || m.alt || m.shift || m.meta) {
      const none = { ctrl: false, alt: false, shift: false, meta: false };
      modifierKeysRef.current = none;
      setModifierKeys(none);
    }
  };

  /** Typed text: a shortcut when Ctrl/Alt/Win is on (Ctrl+C), else text. */
  const typeRemote = (text: string) => {
    const m = modifierKeysRef.current;
    if ((m.ctrl || m.alt || m.meta) && [...text].length === 1) {
      const c = text.toUpperCase();
      if (/^[A-Z0-9]$/.test(c)) {
        pressKeyRemote(c.charCodeAt(0));
        return;
      }
    }
    sendTextRemote(text);
  };

  const sendSpecialKeyRemote = (key: string) => {
    const vk = PHONE_KEY_CODES[key];
    if (vk) pressKeyRemote(vk);
  };

  // The hidden field the phone's keyboard types into. Text is sent once the
  // keyboard commits it (so Vietnamese Telex/VNI and other input methods
  // compose properly); the field keeps two placeholder characters so the
  // delete key still reports deletions when nothing was typed.
  // The latest senders, for listeners attached once (re-attaching them on
  // every render reset the field mid-word, which dropped what the phone's
  // keyboard was composing).
  const typeRemoteRef = useRef(typeRemote);
  typeRemoteRef.current = typeRemote;
  const pressKeyRemoteRef = useRef(pressKeyRemote);
  pressKeyRemoteRef.current = pressKeyRemote;

  useEffect(() => {
    const el = keyboardInputRef.current;
    if (!el) return;
    const FILL = '  ';
    let composing = false;
    const typeRemote = (text: string) => typeRemoteRef.current(text);
    const pressKeyRemote = (vk: number) => pressKeyRemoteRef.current(vk);
    const reset = () => {
      if (composing) return;
      el.value = FILL;
      try { el.setSelectionRange(FILL.length, FILL.length); } catch { /* not focused */ }
    };
    reset();
    // What of the word being composed was already sent: the remote side
    // shows each letter right away (like RustDesk), corrected as the
    // keyboard changes the word (Telex: v, vi, vie, viê, việt).
    let sent = '';
    const follow = (now: string) => {
      const a = [...sent];
      const b = [...now];
      let same = 0;
      while (same < a.length && same < b.length && a[same] === b[same]) same++;
      for (let i = same; i < a.length; i++) pressKeyRemote(8);
      if (same < b.length) typeRemote(b.slice(same).join(''));
      sent = now;
    };
    const onCompositionStart = () => { composing = true; sent = ''; };
    const onCompositionUpdate = (e: CompositionEvent) => follow(e.data ?? '');
    const onCompositionEnd = (e: CompositionEvent) => {
      follow(e.data ?? '');
      sent = '';
      composing = false;
      reset();
    };
    const onInput = (e: Event) => {
      const ie = e as InputEvent;
      if (composing || ie.isComposing) return;
      switch (ie.inputType) {
        case 'deleteContentBackward':
        case 'deleteWordBackward':
          pressKeyRemote(8);
          break;
        case 'deleteContentForward':
          pressKeyRemote(46);
          break;
        case 'insertLineBreak':
        case 'insertParagraph':
          pressKeyRemote(13);
          break;
        default: {
          const text = ie.data ?? (el.value.startsWith(FILL) ? el.value.slice(FILL.length) : el.value.trim());
          if (text) typeRemote(text);
        }
      }
      reset();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (composing || e.isComposing) return;
      // Keys a keyboard reports as keys (a Bluetooth keyboard, or Enter and
      // delete on most phone keyboards).
      const vk = PHONE_KEY_CODES[e.key];
      if (vk) {
        e.preventDefault();
        pressKeyRemote(vk);
      }
    };
    el.addEventListener('compositionstart', onCompositionStart);
    el.addEventListener('compositionupdate', onCompositionUpdate);
    el.addEventListener('compositionend', onCompositionEnd);
    el.addEventListener('input', onInput);
    el.addEventListener('keydown', onKeyDown);
    return () => {
      el.removeEventListener('compositionstart', onCompositionStart);
      el.removeEventListener('compositionupdate', onCompositionUpdate);
      el.removeEventListener('compositionend', onCompositionEnd);
      el.removeEventListener('input', onInput);
      el.removeEventListener('keydown', onKeyDown);
    };
  }, [status]);

  // While the phone's keyboard is open the whole stream view takes the part
  // of the screen the keyboard leaves free (the visual viewport: phones do
  // not resize the page for the keyboard). The keys bar is the last row of
  // that area, so it sits on top of the keyboard, and the picture is fitted
  // into what is left above it.
  const [keyboardArea, setKeyboardArea] = useState<{ top: number; height: number } | null>(null);
  useEffect(() => {
    if (!isKeyboardActive) { setKeyboardArea(null); return; }
    const vv = window.visualViewport;
    if (!vv) return;
    const update = () => {
      const top = Math.max(0, Math.round(vv.offsetTop));
      const height = Math.round(vv.height);
      setKeyboardArea((prev) => (prev && prev.top === top && prev.height === height ? prev : { top, height }));
    };
    update();
    vv.addEventListener('resize', update);
    vv.addEventListener('scroll', update);
    return () => {
      vv.removeEventListener('resize', update);
      vv.removeEventListener('scroll', update);
    };
  }, [isKeyboardActive]);
  // The view changed size: fit the zoom to it again.
  useLayoutEffect(() => {
    const z = viewZoomRef.current;
    if (z.scale > 1) {
      if (touchpadOwnsView()) centerViewOnCursor(z.scale);
      else setTouchView(z.scale, z.pan);
    }
    updateVirtualCursorDOM();
  }, [isKeyboardActive, keyboardArea?.top, keyboardArea?.height]);

  // Send mouse position (absolute) — used only when pointer is NOT locked.
  // When pointer IS locked, the prediction cursor useEffect handles input via
  // pointerrawupdate/pointermove → absolute position on mouseAbsolute channel.
  const handleMouseMove = (e: React.MouseEvent<HTMLVideoElement | HTMLCanvasElement>) => {
    // Ignore emulated mouse events from touches
    if ((e.nativeEvent as any).sourceCapabilities?.firesTouchEvents) {
      return;
    }

    if (status !== "Streaming") return;

    if (!isHardwareMouseActiveRef.current) {
      isHardwareMouseActiveRef.current = true;
      updateVirtualCursorDOM();
      // Hardware mouse detected
    }

    // When pointer locked, prediction cursor useEffect handles everything
    // via pointerrawupdate → relative moves (src/relativePointer.ts). Skip React handler.
    if (isPointerLocked) {
      return;
    }

    // Absolute mouse mode (no pointer lock): update latest position ref
    latestAbsoluteMousePosRef.current = { clientX: e.clientX, clientY: e.clientY };

    const shouldUseWindowsPrediction = agentHostOs === "windows" && hasNativeCursorImageRef.current;
    const hostPoint = getHostPointFromClient(e.clientX, e.clientY);
    if (hostPoint) {
      localCursorPosRef.current = { x: hostPoint.x, y: hostPoint.y };
      if (hideLocalCursor || shouldUseWindowsPrediction) {
        updateVirtualCursorDOMRef.current();
      }
    }
  };

  // Send mouse click event
  const handleMouseButton = (e: React.MouseEvent<HTMLVideoElement | HTMLCanvasElement>, isDown: boolean) => {
    // Ignore emulated mouse events from touches
    if ((e.nativeEvent as any).sourceCapabilities?.firesTouchEvents) {
      return;
    }

    // Prevent context menu from right clicks
    e.preventDefault();
    // preventDefault() on mousedown also stops the browser from moving focus,
    // so a settings control used before (e.g. the FPS dropdown) would keep it
    // and swallow the keyboard. Clicking into the stream releases it.
    if (isDown) {
      const focused = document.activeElement as HTMLElement | null;
      if (focused && focused !== document.body && typeof focused.blur === 'function') {
        focused.blur();
      }
    }

    // The game mouse moves only a locked pointer: clicking the picture locks it.
    if (isDown && gameMouseActiveRef.current && !document.pointerLockElement) {
      togglePointerLock();
      return;
    }

    hostCursorMouseDownRef.current = isDown ? true : e.buttons !== 0;
    const leftHeld = e.button === 0 ? isDown : (e.buttons & 1) !== 0;
    if (leftHeld !== leftButtonHeldRef.current) {
      leftButtonHeldRef.current = leftHeld;
      if (leftHeld) {
        // Where the button went down: the last position sent.
        sentPositionsRef.current.clear();
        recordSentPointer(localCursorPosRef.current.x, localCursorPosRef.current.y);
      }
      if (looksForPointerInPicture(pointerMaybeInPictureRef.current, agentHostOsRef.current) || pointerDragRef.current.active) syncPointerInPictureDrag();
    }
    const hostCursor = hostCursorPosRef.current;
    updateHostCursorDOM(hostCursor.x, hostCursor.y, hostCursor.visible, hostCursor.kind);
    updateVirtualCursorDOMRef.current();
    
    const mouseReliableChannel = channelsRef.current["mouse_reliable"];
    if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
      const buttonMap: Record<number, number> = { 0: 1, 1: 2, 2: 3, 3: 4, 4: 5 };
      const button = buttonMap[e.button] || 1;
      const action = isDown ? 1 : 0; // 1 = Press, 0 = Release

      const buffer = new ArrayBuffer(3);
      const view = new DataView(buffer);
      view.setUint8(0, 2); // Type 2: MouseButton
      view.setUint8(1, action);
      view.setUint8(2, button);
      mouseReliableChannel.send(buffer);
    }
  };

  // Send scroll wheel event
  const handleWheel = (e: React.WheelEvent<HTMLVideoElement | HTMLCanvasElement>) => {
    const mouseReliableChannel = channelsRef.current["mouse_reliable"];
    if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
      scrollXAccumulatorRef.current += e.deltaX;
      scrollYAccumulatorRef.current += -e.deltaY; // Invert deltaY to match system scroll direction

      let dx = 0;
      let dy = 0;

      if (Math.abs(scrollXAccumulatorRef.current) >= 120) {
        dx = Math.trunc(scrollXAccumulatorRef.current / 120);
        scrollXAccumulatorRef.current -= dx * 120;
      }

      if (Math.abs(scrollYAccumulatorRef.current) >= 120) {
        dy = Math.trunc(scrollYAccumulatorRef.current / 120);
        scrollYAccumulatorRef.current -= dy * 120;
      }

      if (dx !== 0 || dy !== 0) {
        const clampedDx = Math.max(-127, Math.min(127, dx));
        const clampedDy = Math.max(-127, Math.min(127, dy));

        const buffer = new ArrayBuffer(3);
        const view = new DataView(buffer);
        view.setUint8(0, 4); // Type 4: Scroll
        view.setInt8(1, clampedDx);
        view.setInt8(2, clampedDy);
        mouseReliableChannel.send(buffer);
      }
    }
  };


  const toggleFullscreen = () => {
    if (!containerRef.current) return;
    if (!document.fullscreenElement) {
      requestFullscreen(containerRef.current).then(() => {
        setIsFullscreen(true);
      }).catch(err => {
        console.error("Error entering fullscreen:", err);
      });
    } else {
      exitFullscreen().then(() => {
        setIsFullscreen(false);
      }).catch(err => {
        console.error("Error exiting fullscreen:", err);
      });
    }
  };

  // Back to the list of computers. Full screen is left first: on phones the
  // stream is usually full screen, and the list must not open in it.
  const leaveSession = () => {
    setShowMobileMenu(false);
    exitFullscreen().catch(() => {});
    onBack();
  };

  const handleStopActiveStream = () => {
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      setIsStoppingStream(true);
      wsRef.current.send(JSON.stringify({
        event: "Signaling",
        data: {
          type: "StopActiveStream",
          payload: { target_id: hostId }
        }
      }));
      addLog("Sent StopActiveStream command.");
    }
  };

  const handleLaunchApp = (appId: number) => {
    setSelectedAppId(appId);
  };

  const isStreaming = status === "Streaming";
  // Hardware-drawn host cursor when possible (smooth as a native pointer);
  // otherwise hide the OS cursor and draw the DOM cursor as before.
  // A host whose pointer is always part of its picture: no pointer at all.
  const streamCursorStyle = streamCursorCss(pointerView({
    streaming: isStreaming,
    alwaysInPicture: pointerAlwaysInPicture,
    pointerLocked: isPointerLocked,
    hostCssCursor,
  }));

  if (selectedAppId === null) {
    return (
      <div className="stream-container" style={{ display: 'flex', flexDirection: 'column', minHeight: '100vh' }}>

        {/* Header/Navbar */}
        <header className="navbar" style={{ position: 'static', background: 'transparent', borderBottom: '1px solid rgb(var(--ink-rgb) / 0.05)' }}>
          <div className="nav-brand">
            <button onClick={leaveSession} className="btn-secondary stream-back-btn" title="Leave" style={{ marginRight: '0.5rem', background: 'rgb(var(--ink-rgb) / 0.03)', border: '1px solid rgb(var(--ink-rgb) / 0.08)' }}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                <path d="M19 12H5M12 19l-7-7 7-7" />
              </svg>
            </button>
            <span className="brand-name" style={{ fontSize: '1.25rem' }}>{hostName}</span>
            <span className="badge-tech">APPS</span>
          </div>
          <div className="nav-user-panel">
            <button onClick={() => setShowSettingsModal(true)} className="btn-secondary" style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', background: 'rgb(var(--ink-rgb) / 0.03)', border: '1px solid rgb(var(--ink-rgb) / 0.08)', padding: '0.5rem 1rem', borderRadius: '8px', fontSize: '0.85rem' }}>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06-.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l.06-.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
              Settings
            </button>
          </div>
        </header>

        {/* Settings Modal Overlay */}
        {showSettingsModal && (
          <StreamSettingsDialog
            hostName={hostName}
            hostAddress={hostAddress}
            sourceOnly={!!sourceId}
            values={draftSettingsValues}
            onChange={setDraftSettingValue}
            onCancel={cancelDraftSettings}
            onSave={applyDraftSettings}
            saveLabel={t('Apply')}
            footerNote={t('Applies to this session and is kept for this computer.')}
            displays={availableDisplays.map(d => ({
              id: d.id,
              label: `${d.name} (${d.width}x${d.height} @ ${Math.round(d.refresh_rate)}Hz)${d.is_primary ? ' ★' : ''}`,
            }))}
            unsupportedCodecs={(['h264', 'h265', 'av1'] as const).filter(c => !supportedCodecs[c])}
          codecLabels={{
            auto: `${t('Auto')} (${resolveAutoCodec().toUpperCase()})`,
            h264: getCodecLabel('H.264', browserCodecs.h264, hostH264Supported),
            h265: getCodecLabel('H.265', browserCodecs.h265, hostH265Supported),
            av1: getCodecLabel('AV1', browserCodecs.av1, hostAv1Supported),
          }}
          encoders={availableEncoders}
          viewer={viewerSettings}
          gameMouse={windowsHost && !sourceId ? { available: gameMouse, on: gameMouseOn, setOn: setGameMouseOn } : undefined}
          />
        )}

        {/* Content Area */}
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '2rem' }}>
          <div className="app-selection-card" style={{ maxWidth: '900px', width: '100%', zIndex: 10, background: 'var(--bg-glass)', border: '1px solid var(--border-color)', borderRadius: '24px', padding: '2.5rem', boxShadow: 'var(--shadow-card)', backdropFilter: 'blur(20px)' }}>
            
            <div style={{ textAlign: 'center', marginBottom: '2.5rem' }}>
              <h2 style={{ fontSize: '1.75rem', fontWeight: 650, color: 'var(--text-strong)', letterSpacing: '-0.02em', marginBottom: '0.5rem' }}>
                Select App to Stream
              </h2>
              <p style={{ color: 'var(--text-muted)', fontSize: '1.05rem' }}>
                Choose an application configured on {hostName} to launch the WebRTC stream.
              </p>
            </div>

            {errorMsg && (
              <div className="auth-error-banner" style={{ marginBottom: '2rem' }}>
                <span className="error-icon">⚠️</span>
                <span>{errorMsg}</span>
              </div>
            )}

            {appList === null ? (
              <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '5rem 0' }}>
                <div className="tech-loader" style={{ marginBottom: '2rem' }}></div>
                <p style={{ color: 'var(--text-muted)', fontSize: '1.1rem', letterSpacing: '0.5px' }}>{status === 'Initializing...' ? 'Connecting to signaling server...' : 'Querying host app list...'}</p>
              </div>
            ) : (
              <div className="apps-grid">
                {appList.map((app) => {
                  const isActive = currentGameId === app.id;
                  const isAnyActive = currentGameId !== 0;
                  
                  return (
                    <div key={app.id} className={`app-card ${isActive ? 'active' : ''}`}>
                      {isActive && (
                        <div className="active-badge">
                          <span className="badge-pulse"></span>
                          Active
                        </div>
                      )}
                      
                      <div className="app-icon-wrapper">
                        {app.icon_base64 ? (
                          <img 
                            src={`data:image/png;base64,${app.icon_base64}`} 
                            alt={app.title} 
                            style={{
                              width: '100%',
                              height: '100%',
                              objectFit: 'cover',
                              borderRadius: '8px'
                            }}
                          />
                        ) : app.title.toLowerCase().includes('desktop') ? (
                          <svg width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                            <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
                            <line x1="8" y1="21" x2="16" y2="21" />
                            <line x1="12" y1="17" x2="12" y2="21" />
                          </svg>
                        ) : app.title.toLowerCase().includes('steam') ? (
                          <svg width="36" height="36" viewBox="0 0 24 24" fill="currentColor">
                            <path d="M12 0C5.378 0 0 5.352 0 11.952c0 4.548 2.562 8.5 6.324 10.518L6.03 19.32a3.864 3.864 0 0 1 1.77-5.184l3.15-4.476c.036-1.572 1.152-2.85 2.652-3.15l1.698-5.328a.534.534 0 0 1 .636-.354.522.522 0 0 1 .36.63L14.73 7.332c1.374.45 2.37 1.692 2.454 3.192l4.824 2.19c.75-.492 1.674-.636 2.544-.378a3.918 3.918 0 0 1 2.766 4.788c-.6 2.394-3.036 3.84-5.46 3.24a3.882 3.882 0 0 1-2.736-3.324l-4.788-2.172c-.426.684-1.128 1.176-1.932 1.344l-3.144 4.464a3.903 3.903 0 0 1-5.184 1.74l3.12 3.12c5.964.882 11.232-3.18 11.232-9.456C24 5.352 18.622 0 12 0zm3.504 12c-1.38 0-2.502-1.122-2.502-2.502S14.124 7 15.504 7s2.502 1.122 2.502 2.502-1.122 2.496-2.502 2.496z"/>
                          </svg>
                        ) : (
                          <svg width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                            <line x1="6" y1="12" x2="10" y2="12" />
                            <line x1="8" y1="10" x2="8" y2="14" />
                            <line x1="15" y1="13" x2="15.01" y2="13" />
                            <line x1="18" y1="11" x2="18.01" y2="11" />
                            <rect x="2" y="6" width="20" height="12" rx="3" />
                          </svg>
                        )}
                      </div>
                      
                      <div className="app-info-section">
                        <h3 className="app-title-text">{app.title}</h3>
                        <span className="app-id-label">App ID: {app.id}</span>
                      </div>

                      <div className="app-actions-panel">
                        {isActive ? (
                          <>
                            <button 
                              onClick={() => handleLaunchApp(app.id)}
                              className="btn-primary app-btn-resume"
                            >
                              Resume Stream
                            </button>
                            <button 
                              onClick={handleStopActiveStream}
                              disabled={isStoppingStream}
                              className="btn-danger app-btn-stop"
                            >
                              {isStoppingStream ? 'Stopping...' : 'Stop Stream'}
                            </button>
                          </>
                        ) : (
                          <button 
                            onClick={() => handleLaunchApp(app.id)}
                            className="btn-secondary app-btn-launch"
                          >
                            {isAnyActive ? 'Switch to App' : 'Launch App'}
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      className={`stream-container is-live ${isKeyboardActive ? 'keyboard-open' : ''}`}
      style={keyboardArea ? { top: keyboardArea.top, height: keyboardArea.height } : undefined}
      ref={containerRef}
      onDragOver={(e) => { if (filesOffered && e.dataTransfer.types.includes('Files')) { e.preventDefault(); setIsDraggingFiles(true); } }}
      onDragLeave={(e) => { if (e.currentTarget === e.target) setIsDraggingFiles(false); }}
      onDrop={(e) => {
        if (!e.dataTransfer.files.length) return;
        e.preventDefault();
        if (!filesOffered) return;
        setIsDraggingFiles(false);
        if (!fileTransfer) { showShareToast(t('File transfer is turned off in Settings')); return; }
        setFileManager({ uploads: Array.from(e.dataTransfer.files) });
      }}
    >
      {isDraggingFiles && (
        <div className="share-drop-hint">{fileTransfer ? t('Drop to send to the remote computer (Downloads/Lunaris)') : t('File transfer is turned off in Settings')}</div>
      )}
      {shareToast && <div className="share-toast" role="status">{shareToast}</div>}
      {fileManager && (
        <FileManager
          hostId={hostId}
          hostName={hostName}
          wsUrl={`${getBackendProtocol().ws}//${getBackendHost()}/ws/client?token=${encodeURIComponent(token)}`}
          sourceId={sourceId}
          initialUploads={fileManager.uploads}
          onClose={() => setFileManager(null)}
        />
      )}
      {/* Settings Modal Overlay */}
      {showSettingsModal && (
        <StreamSettingsDialog
          hostName={hostName}
          hostAddress={hostAddress}
          sourceOnly={!!sourceId}
          values={draftSettingsValues}
          onChange={setDraftSettingValue}
          onCancel={cancelDraftSettings}
          onSave={applyDraftSettings}
          saveLabel={t('Apply')}
          footerNote={t('Applies to this session and is kept for this computer.')}
          displays={availableDisplays.map(d => ({
            id: d.id,
            label: `${d.name} (${d.width}x${d.height} @ ${Math.round(d.refresh_rate)}Hz)${d.is_primary ? ' ★' : ''}`,
          }))}
          unsupportedCodecs={(['h264', 'h265', 'av1'] as const).filter(c => !supportedCodecs[c])}
          codecLabels={{
            auto: `${t('Auto')} (${resolveAutoCodec().toUpperCase()})`,
            h264: getCodecLabel('H.264', browserCodecs.h264, hostH264Supported),
            h265: getCodecLabel('H.265', browserCodecs.h265, hostH265Supported),
            av1: getCodecLabel('AV1', browserCodecs.av1, hostAv1Supported),
          }}
          encoders={availableEncoders}
          viewer={viewerSettings}
          gameMouse={windowsHost && !sourceId ? { available: gameMouse, on: gameMouseOn, setOn: setGameMouseOn } : undefined}
        />
      )}

      {/* Pull tab to show header when hidden */}
      {!isHeaderVisible && isStreaming && (
        <button 
          onClick={() => {
            setIsHeaderVisible(true);
          }}
          className="stream-header-pull-tab"
          title="Show Menu"
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
      )}

      {/* Stream Controls Panel */}
      <div 
        ref={headerBarRef}
        className={`stream-header-bar ${!isHeaderVisible ? 'hidden' : ''}`}
        onMouseEnter={() => {
          if (headerTimeoutRef.current) {
            clearTimeout(headerTimeoutRef.current);
            headerTimeoutRef.current = null;
          }
        }}
        onMouseLeave={() => {
          if (status === "Streaming" && !showSettingsModal && !isHeaderPinned) {
            if (headerTimeoutRef.current) {
              clearTimeout(headerTimeoutRef.current);
            }
            headerTimeoutRef.current = setTimeout(() => {
              setIsHeaderVisible(false);
            }, 3000);
          }
        }}
      >
        <button onClick={leaveSession} className="stream-action-btn" title="Leave Session" style={{ marginRight: '0.25rem' }}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <polyline points="15 18 9 12 15 6" />
          </svg>
        </button>

        {/* Resolution Dropdown */}
        <select
          value={activeResolution}
          onChange={(e) => updateSetting('res', e.target.value)}
          className="stream-select"
          title="Resolution"
        >
          {RESOLUTIONS.map(r => <option key={r.value} value={r.value}>{r.label}</option>)}
        </select>

        {/* FPS Dropdown */}
        <select
          value={activeFps}
          onChange={(e) => updateSetting('fps', e.target.value)}
          className="stream-select"
          title="Frame Rate"
        >
          <option value={240}>240 FPS</option>
          <option value={144}>144 FPS</option>
          <option value={120}>120 FPS</option>
          <option value={90}>90 FPS</option>
          <option value={75}>75 FPS</option>
          <option value={60}>60 FPS</option>
          <option value={30}>30 FPS</option>
        </select>

        {/* Bitrate Dropdown */}
        <select
          value={activeBitrate}
          onChange={(e) => updateSetting('bitrate', e.target.value)}
          className="stream-select"
          title="Bitrate"
        >
          {BITRATES.map(b => <option key={b} value={b}>{bitrateLabel(b)}</option>)}
          {!BITRATES.includes(activeBitrate) && (
            <option value={activeBitrate}>{(activeBitrate / 1000).toFixed(1)} Mbps</option>
          )}
        </select>

        {/* Codec Dropdown */}
        <select
          value={activeCodec}
          onChange={(e) => updateSetting('codec', e.target.value)}
          className="stream-select"
          title={`Video Codec${activeCodec === 'auto' ? ` (Auto → ${resolvedActiveCodec.toUpperCase()})` : ''}`}
        >
          <option value="auto">Auto ({resolvedActiveCodec.toUpperCase()})</option>
          <option value="h264" disabled={!supportedCodecs.h264}>H264</option>
          <option value="h265" disabled={!supportedCodecs.h265}>H265</option>
          <option value="av1" disabled={!supportedCodecs.av1}>AV1</option>
        </select>

        {/* Chroma Dropdown (4:4:4 is H.264 only, like the desktop app) */}
        <select
          value={activeChroma}
          onChange={(e) => updateSetting('chroma', e.target.value)}
          className="stream-select"
          title="Chroma (4:4:4: sharp coloured text, H.264 only)"
        >
          {CHROMAS.map(c => <option key={c.value} value={c.value}>{c.label}</option>)}
        </select>

        {/* Mouse Queue Limit Dropdown */}
        <select
          value={mouseQueueLimit}
          onChange={(e) => updateSetting('mouseQueueLimit', e.target.value)}
          className="stream-select"
          title="Mouse Queue Limit"
        >
          <option value={0}>0 B</option>
          <option value={64}>64 B</option>
          <option value={256}>256 B</option>
          <option value={1024}>1024 B</option>
          <option value={4096}>4096 B</option>
          <option value={16384}>16 KB</option>
        </select>

        {/* Input Protocol Dropdown */}
        <select
          value={activeInputProtocol}
          onChange={(e) => updateSetting('inputProtocol', e.target.value)}
          className="stream-select"
          title="Input Protocol"
        >
          <option value="webrtc">WebRTC</option>
          <option value="webtransport" disabled={typeof (window as any).WebTransport === 'undefined'}>
            WebTransport
          </option>
        </select>

        {/* Display Dropdown */}
        <select
          value={activeDisplay}
          onChange={(e) => updateSetting('display', e.target.value)}
          className="stream-select"
          title="Display"
        >
          <option value="default">Display</option>
          {availableDisplays.map(d => (
            <option key={d.id} value={d.id} title={`${d.name} (${d.width}x${d.height})`}>
              {d.name}{d.is_primary ? ' ★' : ''}
            </option>
          ))}
        </select>

        {/* Key combinations the browser or this computer takes before the
            page sees them (Alt+Tab, Ctrl+W...): pressed on the remote computer. */}
        <select
          value=""
          onChange={(e) => {
            const combo = KEY_COMBOS.find((c) => c.id === e.target.value);
            if (combo) for (const [vk, isDown] of comboStrokes(combo.keys)) sendRawKeyEvent(vk, isDown, 0);
            e.currentTarget.blur();
          }}
          className="stream-select"
          title={t('Send a key combination')}
          aria-label={t('Send a key combination')}
        >
          <option value="">{t('Send keys')}</option>
          {combosFor(agentHostOs).map((combo) => (
            <option key={combo.id} value={combo.id}>{combo.id === 'win' ? t('Windows key (Super)') : combo.label}</option>
          ))}
        </select>

        {/* Separator */}
        <div className="stream-menu-separator"></div>

        {/* Pointer Lock Action */}
        <button 
          onClick={togglePointerLock}
          className={`stream-action-btn ${isPointerLocked ? 'active' : ''}`}
          title={isPointerLocked ? "Release Pointer Lock (Hold ESC 3s)" : "Lock Pointer"}
        >
          {isPointerLocked ? (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
              <path d="M7 11V7a5 5 0 0 1 10 0v4" />
            </svg>
          ) : (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
              <path d="M7 11V7a5 5 0 0 1 9.9-1" />
            </svg>
          )}
        </button>

        {/* Local Cursor Action */}
        <button 
          onClick={() => {
            const newValue = !hideLocalCursor;
            setHideLocalCursor(newValue);
            localStorage.setItem('lunaris_stream_hide_cursor', String(newValue));

          }}
          className={`stream-action-btn ${!hideLocalCursor ? 'active' : ''}`}
          title={hideLocalCursor ? "Show Local Cursor" : "Hide Local Cursor"}
        >
          {hideLocalCursor ? (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" opacity="0.4">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
              <circle cx="12" cy="12" r="3" />
              <line x1="1" y1="1" x2="23" y2="23" />
            </svg>
          ) : (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
          )}
        </button>

        {/* Stats Action */}
        <button 
          onClick={() => {
            const newValue = !showStats;
            setShowStats(newValue);
            localStorage.setItem('lunaris_show_stats', String(newValue));
          }}
          className={`stream-action-btn ${showStats ? 'active' : ''}`}
          title={showStats ? "Hide Stats" : "Show Stats"}
        >
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <line x1="18" y1="20" x2="18" y2="10" />
            <line x1="12" y1="20" x2="12" y2="4" />
            <line x1="6" y1="20" x2="6" y2="14" />
          </svg>
        </button>

        {/* Pin Header Action */}
        <button 
          onClick={() => {
            const newValue = !isHeaderPinned;
            setIsHeaderPinned(newValue);
            localStorage.setItem('lunaris_header_pinned', String(newValue));
          }}
          className={`stream-action-btn ${isHeaderPinned ? 'active' : ''}`}
          title={isHeaderPinned ? "Unlock menu (auto-hide)" : "Lock menu (always visible)"}
        >
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <line x1="12" y1="17" x2="12" y2="22" />
            <path d="M5 12h14" />
            <path d="M19 5H5v3l3 4h8l3-4z" />
          </svg>
        </button>

        {/* Mute/Unmute Action */}
        <button 
          onClick={() => {
            const newValue = !isMuted;
            setIsMuted(newValue);
            localStorage.setItem('lunaris_stream_muted', String(newValue));
          }}
          className={`stream-action-btn ${!isMuted ? 'active' : ''}`}
          title={isMuted ? t('Sound off') : t('Sound on')} aria-label={isMuted ? t('Sound off') : t('Sound on')}
        >
          {isMuted ? (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <path d="M11 5L6 9H2v6h4l5 4V5z" fill="none" />
              <line x1="22" y1="9" x2="16" y2="15" />
              <line x1="16" y1="9" x2="22" y2="15" />
            </svg>
          ) : (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <path d="M11 5L6 9H2v6h4l5 4V5z" fill="none" />
              <path d="M19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07" />
            </svg>
          )}
        </button>

        {/* Fullscreen/Maximize Action */}
        {canFullscreen() && <button 
          onClick={toggleFullscreen}
          className={`stream-action-btn ${isFullscreen ? 'active' : ''}`}
          title={isFullscreen ? "Exit Fullscreen" : "Fullscreen"}
        >
          {isFullscreen ? (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <rect x="4" y="4" width="16" height="16" rx="2" />
              <rect x="9" y="9" width="11" height="11" rx="2" fill="var(--bg-primary)" style={{ opacity: 0.8 }} />
            </svg>
          ) : (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
            </svg>
          )}
        </button>}

        {/* Send files to the remote computer */}
        <button
          style={filesOffered ? undefined : { display: 'none' }}
          onClick={() => fileTransfer ? setFileManager({}) : showShareToast(t('File transfer is turned off in Settings'))}
          className="stream-action-btn"
          title={t('Files on the remote computer (or drop files on the stream)')}
          aria-label={t('Files on the remote computer')}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M12 16V4M7 9l5-5 5 5M4 16v4h16v-4" />
          </svg>
        </button>

        {/* Settings Desktop Action */}
        <button 
          onClick={() => {
            setShowSettingsModal(prev => !prev);
          }}
          className={`stream-action-btn ${showSettingsModal ? 'active' : ''}`}
          title="Stream Settings"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
        </button>

        {/* Collapse Header Action */}
        <button 
          onClick={() => {
            setIsHeaderVisible(false);
            if (headerTimeoutRef.current) {
              clearTimeout(headerTimeoutRef.current);
            }
          }}
          className="stream-action-btn"
          title="Collapse Menu"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <polyline points="18 15 12 9 6 15" />
          </svg>
        </button>
      </div>

      {/* Main Stream Area */}
      <div 
        ref={viewportWrapperRef}
        className="stream-viewport-wrapper"
      >
        {/* Hidden input for mobile virtual keyboard */}
        <input
          ref={keyboardInputRef}
          type="text"
          autoCapitalize="off"
          autoCorrect="off"
          autoComplete="off"
          spellCheck={false}
          enterKeyHint="enter"
          aria-label={t('Keyboard')}
          style={{
            // Inside the view, so the phone does not scroll the page to
            // reach it, and 16px, so iOS does not zoom in on focus.
            position: 'absolute',
            top: 0,
            left: 0,
            width: '1px',
            height: '1px',
            padding: 0,
            border: 0,
            fontSize: '16px',
            caretColor: 'transparent',
            opacity: 0,
            zIndex: -100,
            pointerEvents: 'none'
          }}
          onBlur={() => setIsKeyboardActive(false)}
        />

        {useCanvasRenderer ? (
          <>
            <canvas
              key={canvasKey}
              ref={videoRef as any}
              onMouseMove={handleMouseMove}
              onMouseDown={(e) => handleMouseButton(e, true)}
              onMouseUp={(e) => handleMouseButton(e, false)}
              onMouseEnter={updateVideoRect}
              onContextMenu={(e) => e.preventDefault()}
              onWheel={handleWheel}
              className={`stream-video-view ${isStreaming ? 'visible' : 'hidden'}`}
              style={{ 
                cursor: streamCursorStyle,
                transform: `translate(${zoomPan.x}px, ${zoomPan.y}px) scale(${zoomScale})`,
                transformOrigin: '0 0',
                objectFit: objectFitFor(displayMode)
              }}
            />
            <video
              ref={hiddenVideoRef}
              autoPlay
              playsInline
              muted={isMuted}
              onLoadedMetadata={handleVideoLoadedMetadata}
              onResize={updatePictureCrop}
              style={{ display: 'none' }}
            />
          </>
        ) : (
          <video
            ref={videoRef as any}
            onMouseMove={handleMouseMove}
            onMouseDown={(e) => handleMouseButton(e, true)}
            onMouseUp={(e) => handleMouseButton(e, false)}
            onMouseEnter={updateVideoRect}
            onContextMenu={(e) => e.preventDefault()}
            onWheel={handleWheel}
            className={`stream-video-view ${isStreaming ? 'visible' : 'hidden'}`}
            autoPlay
            playsInline
            muted={isMuted}
            onLoadedMetadata={handleVideoLoadedMetadata}
            onResize={updatePictureCrop}
            style={{ 
              cursor: streamCursorStyle,
              transform: `translate(${zoomPan.x}px, ${zoomPan.y}px) scale(${zoomScale})`,
              transformOrigin: '0 0',
              objectFit: objectFitFor(displayMode),
              ...pictureViewBox()
            }}
          />
        )}

        {/* Host cursor forwarded by the agent. It stays outside the video frame to preserve GPU zero-copy. */}
        <div
          ref={hostCursorRef}
          className="host-remote-cursor"
          style={{
            position: 'absolute',
            width: '32px',
            height: '32px',
            pointerEvents: 'none',
            zIndex: 145,
            left: 0,
            top: 0,
            transform: 'translate3d(0, 0, 0)',
            transition: 'none',
            display: 'none',
            willChange: 'transform',
            // No paint containment: a host cursor is often larger than 32 px
            // (League of Legends'), and the box would cut its corner off.
            contain: 'layout style',
          }}
        >
          <img
            ref={hostCursorImageRef}
            alt=""
            draggable={false}
            style={{ width: '32px', height: '32px', display: 'block', maxWidth: 'none' }}
          />
        </div>

        {escapeNotice && (
          <div role="status" style={{
            position: 'absolute',
            top: '16px',
            left: '50%',
            transform: 'translateX(-50%)',
            zIndex: 200,
            maxWidth: 'min(560px, calc(100% - 32px))',
            background: 'rgba(0, 0, 0, 0.8)',
            color: '#fff',
            borderRadius: '8px',
            padding: '8px 14px',
            fontSize: '13px',
            lineHeight: 1.4,
            textAlign: 'center',
            pointerEvents: 'none',
            userSelect: 'none',
          }}>{escapeNotice}</div>
        )}

        {/* ESC hold-to-exit progress bar overlay */}
        {escapeHoldProgress > 0 && (
          <div style={{
            position: 'absolute',
            bottom: '12px',
            left: '50%',
            transform: 'translateX(-50%)',
            zIndex: 200,
            background: 'rgba(0, 0, 0, 0.75)',
            borderRadius: '6px',
            padding: '6px 14px',
            display: 'flex',
            alignItems: 'center',
            gap: '8px',
            pointerEvents: 'none',
            userSelect: 'none',
          }}>
            <span style={{
              color: 'var(--text-muted)',
              fontSize: '12px',
              fontFamily: 'system-ui, sans-serif',
              whiteSpace: 'nowrap',
            }}>Hold ESC to exit</span>
            <div style={{
              width: '80px',
              height: '4px',
              background: 'rgb(var(--ink-rgb) / 0.2)',
              borderRadius: '2px',
              overflow: 'hidden',
            }}>
              <div style={{
                width: `${Math.round(escapeHoldProgress * 100)}%`,
                height: '100%',
                background: '#f44',
                borderRadius: '2px',
                transition: 'width 50ms linear',
              }} />
            </div>
          </div>
        )}

        {/* Client-side predicted cursor for local mouse/trackpad feedback. */}
        <div
          ref={localCursorRef}
          className="client-virtual-cursor"
          style={{
            position: 'absolute',
            width: '32px',
            height: '32px',
            pointerEvents: 'none',
            zIndex: 150,
            left: 0,
            top: 0,
            transform: 'translate3d(0, 0, 0)',
            transition: 'none',
            display: "none",
            willChange: 'transform',
            // No paint containment: a host cursor is often larger than 32 px
            // (League of Legends'), and the box would cut its corner off.
            contain: 'layout style',
          }}
        >
          <img
            ref={localCursorImageRef}
            alt=""
            draggable={false}
            style={{ width: '32px', height: '32px', display: 'block', maxWidth: 'none' }}
          />
        </div>



        {/* Mobile controls drawer & footer menu bar for touch screens */}
        {isStreaming && (
          <>
            {/* Phone controls sheet: how touch controls the computer, quick
                actions, and the way to the full stream settings. */}
            {showMobileMenu && (
              <div
                className="mobile-sheet-backdrop"
                onClick={(e) => { e.stopPropagation(); setShowMobileMenu(false); }}
                onTouchStart={(e) => e.stopPropagation()}
                onTouchMove={(e) => e.stopPropagation()}
                onTouchEnd={(e) => e.stopPropagation()}
                onMouseDown={(e) => e.stopPropagation()}
                onMouseUp={(e) => e.stopPropagation()}
              >
                <div
                  className="mobile-controls-drawer mobile-sheet"
                  role="dialog"
                  aria-label={t('Controls')}
                  onClick={(e) => e.stopPropagation()}
                >
                  <div className="mobile-sheet-handle" aria-hidden="true" />
                  <div className="mobile-sheet-header">
                    <h4>{t('Controls')}</h4>
                    <button type="button" className="mobile-sheet-close" aria-label={t('Close')} onClick={() => setShowMobileMenu(false)}>
                      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round"><path d="M6 6l12 12M18 6L6 18" /></svg>
                    </button>
                  </div>

                  <div className="mobile-sheet-label">{t('Touch control')}</div>
                  <div className="mobile-segmented" role="radiogroup" aria-label={t('Touch control')}>
                    {([
                      ['trackpad', t('Touchpad'), t('Move the cursor like a laptop touchpad')],
                      ['direct', t('Direct touch'), t('Click where you touch')],
                    ] as const).map(([mode, label, hint]) => (
                      <button
                        key={mode}
                        type="button"
                        role="radio"
                        aria-checked={touchMode === mode}
                        className={`mobile-segment ${touchMode === mode ? 'active' : ''}`}
                        onClick={() => {
                          setTouchMode(mode);
                          localStorage.setItem('lunaris_mobile_touch_mode', mode);
                        }}
                      >
                        <span className="mobile-segment-title">{label}</span>
                        <span className="mobile-segment-hint">{hint}</span>
                      </button>
                    ))}
                  </div>

                  <ul className="mobile-gesture-list">
                    {(touchMode === 'trackpad'
                      ? [
                        [t('Drag one finger'), t('Move the cursor')],
                        [t('Tap'), t('Click')],
                        [t('Tap, then touch and drag'), t('Drag and drop')],
                        [t('Long press or two-finger tap'), t('Right click')],
                        [t('Drag two fingers'), t('Scroll')],
                        [t('Pinch'), t('Zoom')],
                      ]
                      : [
                        [t('Tap'), t('Click there')],
                        [t('Tap, then touch and drag'), t('Drag and drop')],
                        [t('Long press or two-finger tap'), t('Right click')],
                        [t('Drag one finger'), t('Move around when zoomed in')],
                        [t('Drag two fingers'), t('Scroll')],
                        [t('Pinch'), t('Zoom')],
                      ]).map(([gesture, effect]) => (
                      <li key={gesture}><span>{gesture}</span><span>{effect}</span></li>
                    ))}
                  </ul>

                  {touchMode === 'direct' && (
                    <label className="mobile-sheet-switch">
                      <span>
                        <span className="mobile-sheet-switch-title">{t('Click above the finger')}</span>
                        <span className="mobile-sheet-switch-hint">{t('The click lands 40 px above where you touch, so the finger does not hide it.')}</span>
                      </span>
                      <input
                        type="checkbox"
                        checked={useTouchOffset}
                        onChange={(e) => {
                          setUseTouchOffset(e.target.checked);
                          localStorage.setItem('lunaris_mobile_touch_offset', String(e.target.checked));
                        }}
                      />
                    </label>
                  )}

                  <div className="mobile-sheet-label">{t('Actions')}</div>
                  <div className="mobile-action-grid">
                    <button
                      type="button"
                      className={`mobile-action ${isKeyboardActive ? 'active' : ''}`}
                      onClick={() => {
                        setShowMobileMenu(false);
                        keyboardInputRef.current?.focus();
                        setIsKeyboardActive(true);
                      }}
                    >
                      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><rect x="2" y="6" width="20" height="12" rx="2" /><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10" /></svg>
                      <span>{t('Keyboard')}</span>
                    </button>
                    <button
                      type="button"
                      className="mobile-action"
                      disabled={zoomScale === 1}
                      onClick={() => { setZoomScale(1); setZoomPan({ x: 0, y: 0 }); }}
                    >
                      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><circle cx="11" cy="11" r="7" /><path d="M21 21l-4.3-4.3M8 11h6" /></svg>
                      <span>{t('Reset zoom')}</span>
                    </button>
                    <button
                      type="button"
                      className={`mobile-action ${!isMuted ? 'active' : ''}`}
                      onClick={() => {
                        const newValue = !isMuted;
                        setIsMuted(newValue);
                        localStorage.setItem('lunaris_stream_muted', String(newValue));
                      }}
                    >
                      {isMuted ? (
                        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M4 9v6h4l5 4V5L8 9H4z" /><path d="M22 9l-6 6M16 9l6 6" /></svg>
                      ) : (
                        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M4 9v6h4l5 4V5L8 9H4z" /><path d="M16 9a3.5 3.5 0 0 1 0 6M18.5 6.5a7 7 0 0 1 0 11" /></svg>
                      )}
                      <span>{isMuted ? t('Sound off') : t('Sound on')}</span>
                    </button>
                    {canFullscreen() && <button type="button" className={`mobile-action ${isFullscreen ? 'active' : ''}`} onClick={toggleFullscreen}>
                      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" /></svg>
                      <span>{isFullscreen ? t('Exit full screen') : t('Full screen')}</span>
                    </button>}
                    <button
                      type="button"
                      className="mobile-action"
                      onClick={() => { setShowMobileMenu(false); setShowSettingsModal(true); }}
                    >
                      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0" /><circle cx="16" cy="6" r="2" /><circle cx="10" cy="12" r="2" /><circle cx="18" cy="18" r="2" /></svg>
                      <span>{t('Stream settings')}</span>
                    </button>
                    {/* Leaves the session, as Leave does on a desktop: the
                        player unmounts and its cleanup ends the session.
                        (Stopping the stream alone left a black picture with
                        no way back but reloading the page.) */}
                    <button type="button" className="mobile-action danger" onClick={leaveSession}>
                      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M12 3v9M6.3 7.5a8 8 0 1 0 11.4 0" /></svg>
                      <span>{t('Disconnect')}</span>
                    </button>
                  </div>
                </div>
              </div>
            )}

            {/* Pull tab to show footer when collapsed */}
            {!isMobileFooterVisible && (
              <button
                className="mobile-footer-pull-tab"
                title="Show Menu"
                onTouchStart={(e) => e.stopPropagation()}
                onTouchMove={(e) => e.stopPropagation()}
                onTouchEnd={(e) => e.stopPropagation()}
                onMouseDown={(e) => e.stopPropagation()}
                onMouseUp={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  setIsMobileFooterVisible(true);
                }}
              >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="18 15 12 9 6 15"></polyline>
                </svg>
              </button>
            )}

            {/* Footer Bar Container */}
            <div 
              className={`mobile-footer-bar ${!isMobileFooterVisible ? 'collapsed' : ''}`}
              onTouchStart={(e) => e.stopPropagation()}
              onTouchMove={(e) => e.stopPropagation()}
              onTouchEnd={(e) => e.stopPropagation()}
              onMouseDown={(e) => e.stopPropagation()}
              onMouseUp={(e) => e.stopPropagation()}
              onClick={(e) => e.stopPropagation()}
            >
              {/* Close Button */}
              <button 
                onClick={leaveSession}
                className="mobile-footer-btn"
                title={t('Disconnect')} aria-label={t('Disconnect')}
              >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <line x1="18" y1="6" x2="6" y2="18"></line>
                  <line x1="6" y1="6" x2="18" y2="18"></line>
                </svg>
              </button>

              {/* Display Settings Button */}
              <button 
                onClick={() => setShowSettingsModal(true)}
                className="mobile-footer-btn"
                title={t('Stream settings')} aria-label={t('Stream settings')}
              >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="2" y="3" width="20" height="14" rx="2" ry="2"></rect>
                  <line x1="8" y1="21" x2="16" y2="21"></line>
                  <line x1="12" y1="17" x2="12" y2="21"></line>
                </svg>
              </button>

              {/* Keyboard Toggle Button */}
              <button 
                onClick={() => {
                  if (keyboardInputRef.current) {
                    if (isKeyboardActive) {
                      keyboardInputRef.current.blur();
                      setIsKeyboardActive(false);
                    } else {
                      keyboardInputRef.current.focus();
                      setIsKeyboardActive(true);
                    }
                  }
                }}
                className={`mobile-footer-btn ${isKeyboardActive ? 'active' : ''}`}
                title={t('Keyboard')} aria-label={t('Keyboard')}
              >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="2" y="4" width="20" height="16" rx="2" ry="2"></rect>
                  <path d="M6 8h.01M10 8h.01M14 8h.01M18 8h.01M6 12h.01M18 12h.01M7 16h10"></path>
                </svg>
              </button>

              {/* Mouse Mode Button */}
              <button 
                onClick={() => {
                  const nextMode = touchMode === 'trackpad' ? 'direct' : 'trackpad';
                  setTouchMode(nextMode);
                  localStorage.setItem('lunaris_mobile_touch_mode', nextMode);
                }}
                className={`mobile-footer-btn ${touchMode === 'trackpad' ? 'active-trackpad' : ''}`}
                title={touchMode === 'trackpad' ? t('Touchpad') : t('Direct touch')} aria-label={touchMode === 'trackpad' ? t('Touchpad') : t('Direct touch')}
              >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="5" y="2" width="14" height="20" rx="7" ry="7"></rect>
                  <line x1="12" y1="2" x2="12" y2="12"></line>
                  <line x1="5" y1="10" x2="19" y2="10"></line>
                </svg>
              </button>

              {/* Mute Button */}
              <button 
                onClick={() => {
                  const newValue = !isMuted;
                  setIsMuted(newValue);
                  localStorage.setItem('lunaris_stream_muted', String(newValue));
                }}
                className={`mobile-footer-btn ${!isMuted ? 'active' : ''}`}
                title={isMuted ? t('Sound off') : t('Sound on')} aria-label={isMuted ? t('Sound off') : t('Sound on')}
              >
                {isMuted ? (
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M11 5L6 9H2v6h4l5 4V5z" />
                    <line x1="22" y1="9" x2="16" y2="15" />
                    <line x1="16" y1="9" x2="22" y2="15" />
                  </svg>
                ) : (
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M11 5L6 9H2v6h4l5 4V5z" />
                    <path d="M19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07" />
                  </svg>
                )}
              </button>

              {/* Fullscreen Button (not on iPhones: Safari shows only videos full screen) */}
              {canFullscreen() && <button
                onClick={toggleFullscreen}
                className={`mobile-footer-btn ${isFullscreen ? 'active' : ''}`}
                title={isFullscreen ? t('Exit full screen') : t('Full screen')} aria-label={isFullscreen ? t('Exit full screen') : t('Full screen')}
              >
                {isFullscreen ? (
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="4 14 10 14 10 20" />
                    <polyline points="20 10 14 10 14 4" />
                    <line x1="10" y1="14" x2="3" y2="21" />
                    <line x1="21" y1="3" x2="14" y2="10" />
                  </svg>
                ) : (
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="15 3 21 3 21 9" />
                    <polyline points="9 21 3 21 3 15" />
                    <line x1="21" y1="3" x2="14" y2="10" />
                    <line x1="3" y1="21" x2="10" y2="14" />
                  </svg>
                )}
              </button>}

              {/* Stats Overlay Toggle Button */}
              <button 
                onClick={() => {
                  const newValue = !showStats;
                  setShowStats(newValue);
                  localStorage.setItem('lunaris_show_stats', String(newValue));
                }}
                className={`mobile-footer-btn ${showStats ? 'active' : ''}`}
                title="Stats"
              >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <line x1="18" y1="20" x2="18" y2="10"></line>
                  <line x1="12" y1="20" x2="12" y2="4"></line>
                  <line x1="6" y1="20" x2="6" y2="14"></line>
                </svg>
              </button>

              {/* More Menu / Controls Drawer Button */}

              <button 
                onClick={() => setShowMobileMenu(prev => !prev)}
                className={`mobile-footer-btn ${showMobileMenu ? 'active' : ''}`}
                title={t('Controls')}
                aria-label={t('Controls')}
              >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="12" cy="5" r="1"></circle>
                  <circle cx="12" cy="12" r="1"></circle>
                  <circle cx="12" cy="19" r="1"></circle>
                </svg>
              </button>

              {/* Collapse Button */}
              <button 
                onClick={() => setIsMobileFooterVisible(false)}
                className="mobile-footer-btn collapse-btn"
                title="Hide Bar"
              >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="6 9 12 15 18 9"></polyline>
                </svg>
              </button>
            </div>
          </>
        )}

        {/* Stats overlay */}
        {isStreaming && showStats && (
          <div className="stream-stats-overlay">
            <div>Ping <span className="stat-value">{pingStatText(stats.ping)}</span></div>
            <div>FPS <span className="stat-value">{stats.decodedFps || stats.fps}</span></div>
            <div>Bitrate <span className="stat-value">{(stats.bitrate / 1000).toFixed(1)} Mbps</span></div>
            <div>Codec <span className="stat-value">{resolvedActiveCodec.toUpperCase()}</span></div>
            <div>Encode <span className="stat-value">{encodeStatText(activeEncoderStatus.encoder, activeEncoderStatus.hwType, activeEncoderStatus.gpuInfo)}</span></div>
            <div>Decode <span className="stat-value">{decodeStatText(stats.decoder, stats.decodeLatency)}</span></div>
            <div>Network <span className="stat-value" style={{ color: stats.connectionType === 'P2P (Direct)' ? '#4ade80' : '#fb923c' }}>
              {stats.connectionType === 'P2P (Direct)' ? 'Direct' : stats.connectionType}
            </span></div>
            {stats.jitter >= 30 && (
              <div>Jitter <span className="stat-value" style={{ color: '#fb923c' }}>{stats.jitter.toFixed(0)} ms</span></div>
            )}
          </div>
        )}

        {/* Non-streaming status overlay (Loading / Errors) */}
        {!isStreaming && (
          <div className="stream-status-overlay-panel">
            {errorMsg ? (
              <div className="error-panel">
                <div className="error-icon">⚠️</div>
                <h3>Connection Failure</h3>
                <p>{errorMsg}</p>
                <button onClick={leaveSession} className="btn-primary">Return to Dashboard</button>
              </div>
            ) : (
              <div className="loading-panel">
                <div className="tech-loader"></div>
                <h3>Connecting to {hostName}</h3>
                <p>{status}</p>
                <div className="connecting-subtext">Initializing WebRTC session...</div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Keys the phone keyboard lacks, in a row right above it. Buttons keep
          the focus in the hidden field so the keyboard stays open. "Hide
          keyboard" comes first and stays put; beside it the first ten
          keys fill the width and the rest are reached by sliding the row. */}
      {isStreaming && isKeyboardActive && (
        <div
          className="mobile-keys-bar"
          onPointerDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
          onTouchStart={(e) => e.stopPropagation()}
          onTouchMove={(e) => e.stopPropagation()}
          onTouchEnd={(e) => e.stopPropagation()}
        >
          {/* First, outside the sliding row: always in reach. */}
          <button
            type="button"
            className="mobile-key mobile-key-hide"
            aria-label={t('Hide keyboard')}
            title={t('Hide keyboard')}
            onClick={() => { keyboardInputRef.current?.blur(); setIsKeyboardActive(false); }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <polyline points="6 9 12 15 18 9" />
            </svg>
          </button>
          <div className="mobile-keys-row">
            {MOBILE_KEYS.map((key) => (key.mod ? (
              <button
                key={key.label}
                type="button"
                className={`mobile-key ${modifierKeys[key.mod] ? 'active' : ''}`}
                aria-pressed={modifierKeys[key.mod]}
                onClick={() => setModifierKeys((prev) => {
                  const next = { ...prev, [key.mod!]: !prev[key.mod!] };
                  modifierKeysRef.current = next;
                  return next;
                })}
              >{key.label}</button>
            ) : (
              <button key={key.label} type="button" className="mobile-key" aria-label={key.key} onClick={() => sendSpecialKeyRemote(key.key!)}>{key.label}</button>
            )))}
            <button type="button" className="mobile-key wide" onClick={() => pressKeyRemote(46, { ctrl: true, alt: true })}>Ctrl+Alt+Del</button>
          </div>
        </div>
      )}
    </div>
  );
};
