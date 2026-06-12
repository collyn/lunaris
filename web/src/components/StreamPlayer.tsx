import React, { useEffect, useRef, useState } from 'react';

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

const HOST_CURSOR_ASSETS: Record<HostCursorKind, { src: string; hotspotX: number; hotspotY: number }> = {
  arrow: { src: '/cursors/windows-aero-arrow.png', hotspotX: 0, hotspotY: 0 },
  ibeam: { src: '/cursors/windows-aero-ibeam.png', hotspotX: 16, hotspotY: 16 },
  hand: { src: '/cursors/windows-aero-hand.png', hotspotX: 6, hotspotY: 1 },
  cross: { src: '/cursors/windows-aero-cross.png', hotspotX: 16, hotspotY: 16 },
  move: { src: '/cursors/windows-aero-move.png', hotspotX: 16, hotspotY: 16 },
  resize_ns: { src: '/cursors/windows-aero-resize-ns.png', hotspotX: 16, hotspotY: 16 },
  resize_ew: { src: '/cursors/windows-aero-resize-ew.png', hotspotX: 16, hotspotY: 16 },
  resize_nesw: { src: '/cursors/windows-aero-resize-nesw.png', hotspotX: 16, hotspotY: 16 },
  resize_nwse: { src: '/cursors/windows-aero-resize-nwse.png', hotspotX: 16, hotspotY: 16 },
  unavailable: { src: '/cursors/windows-aero-unavailable.png', hotspotX: 16, hotspotY: 16 },
  unknown: { src: '/cursors/windows-aero-arrow.png', hotspotX: 0, hotspotY: 0 },
};

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
type CodecChoice = CodecName | 'auto';

interface StreamPlayerProps {
  hostId: string;
  hostName: string;
  token: string;
  serverCodecModeSupport?: number;
  onBack: () => void;
  appId?: number | null;
}

const KEY_TO_VK: Record<string, number> = {
  "Backspace": 8, "Tab": 9, "Enter": 13, "ShiftLeft": 16, "ShiftRight": 16,
  "ControlLeft": 17, "ControlRight": 17, "AltLeft": 18, "AltRight": 18,
  "Pause": 19, "CapsLock": 20, "Escape": 27, "Space": 32, "PageUp": 33,
  "PageDown": 34, "End": 35, "Home": 36, "ArrowLeft": 37, "ArrowUp": 38,
  "ArrowRight": 39, "ArrowDown": 40, "PrintScreen": 44, "Insert": 45,
  "Delete": 46, "Digit0": 48, "Digit1": 49, "Digit2": 50, "Digit3": 51,
  "Digit4": 52, "Digit5": 53, "Digit6": 54, "Digit7": 55, "Digit8": 56,
  "Digit9": 57, "KeyA": 65, "KeyB": 66, "KeyC": 67, "KeyD": 68, "KeyE": 69,
  "KeyF": 70, "KeyG": 71, "KeyH": 72, "KeyI": 73, "KeyJ": 74, "KeyK": 75,
  "KeyL": 76, "KeyM": 77, "KeyN": 78, "KeyO": 79, "KeyP": 80, "KeyQ": 81,
  "KeyR": 82, "KeyS": 83, "KeyT": 84, "KeyU": 85, "KeyV": 86, "KeyW": 87,
  "KeyX": 88, "KeyY": 89, "KeyZ": 90, "MetaLeft": 91, "MetaRight": 92,
  "Numpad0": 96, "Numpad1": 97, "Numpad2": 98, "Numpad3": 99, "Numpad4": 100,
  "Numpad5": 101, "Numpad6": 102, "Numpad7": 103, "Numpad8": 104, "Numpad9": 105,
  "NumpadMultiply": 106, "NumpadAdd": 107, "NumpadSubtract": 109, "NumpadDecimal": 110,
  "NumpadDivide": 111, "F1": 112, "F2": 113, "F3": 114, "F4": 115, "F5": 116,
  "F6": 117, "F7": 118, "F8": 119, "F9": 120, "F10": 121, "F11": 122, "F12": 123,
  "NumLock": 144, "ScrollLock": 145, "Semicolon": 186, "Equal": 187, "Comma": 188,
  "Minus": 189, "Period": 190, "Slash": 191, "Backquote": 192, "BracketLeft": 219,
  "Backslash": 220, "BracketRight": 221, "Quote": 222
};

export const StreamPlayer: React.FC<StreamPlayerProps> = ({
  hostId,
  hostName,
  token,
  serverCodecModeSupport,
  onBack,
  appId
}) => {
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
  const wtConnectingRef = useRef<boolean>(false);

  // Mobile gesture and keyboard refs
  const keyboardInputRef = useRef<HTMLInputElement>(null);
  const initialTouchDistanceRef = useRef<number>(0);
  const initialZoomScaleRef = useRef<number>(1);
  const initialZoomPanRef = useRef<{ x: number, y: number }>({ x: 0, y: 0 });
  const initialTouchMidpointRef = useRef<{ x: number, y: number }>({ x: 0, y: 0 });
  const touchStartPosRef = useRef<{ x: number, y: number }>({ x: 0, y: 0 });
  const touchStartTimeRef = useRef<number>(0);
  const lastTouchTapTimeRef = useRef<number>(0);
  const isDraggingRef = useRef<boolean>(false);
  const viewportWrapperRef = useRef<HTMLDivElement>(null);
  const touchStartInitialPosRef = useRef<{ x: number, y: number }>({ x: 0, y: 0 });
  const longPressTimerRef = useRef<any | null>(null);
  const longPressTriggeredRef = useRef<boolean>(false);
  const clickUpTimerRef = useRef<any | null>(null);
  const wasMultiTouchRef = useRef<boolean>(false);
  const isDirectClickPendingRef = useRef<boolean>(false);
  const twoFingerTouchStartTimeRef = useRef<number>(0);
  const isTwoFingerTapPendingRef = useRef<boolean>(false);
  const localCursorRef = useRef<HTMLDivElement>(null);
  const localCursorImageRef = useRef<HTMLImageElement>(null);
  const localCursorImageMetricsRef = useRef<HostCursorImageMetrics | null>(null);
  const hostCursorRef = useRef<HTMLDivElement>(null);
  const hostCursorImageRef = useRef<HTMLImageElement>(null);
  const hostCursorImageMetricsRef = useRef<HostCursorImageMetrics | null>(null);
  const hasNativeCursorImageRef = useRef<boolean>(false);
  const hostCursorMouseDownRef = useRef<boolean>(false);
  const lastHostCursorLocalPredictionAtRef = useRef<number>(0);
  const hostCursorPosRef = useRef<{ x: number, y: number, visible: boolean, kind: HostCursorKind, inWindowMoveSize: boolean }>({
    x: 0,
    y: 0,
    visible: false,
    kind: 'arrow',
    inWindowMoveSize: false,
  });
  const isHardwareMouseActiveRef = useRef<boolean>(false);
  const localCursorPosRef = useRef<{ x: number, y: number }>({ x: 960, y: 540 });
  const initialLocalCursorPosRef = useRef<{ x: number, y: number }>({ x: 960, y: 540 });
  const hasCenteredThisTouchRef = useRef<boolean>(false);
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
  const [appList, setAppList] = useState<{ id: number; title: string; icon_base64?: string | null }[] | null>(null);
  const [currentGameId, setCurrentGameId] = useState<number>(0);
  const [selectedAppId, setSelectedAppId] = useState<number | null>(appId ?? null);
  const [isStoppingStream, setIsStoppingStream] = useState<boolean>(false);
  
  // Settings States
  const [activeResolution, setActiveResolution] = useState<string>(() => localStorage.getItem('lunaris_stream_res') || '1080p');
  const [activeFps, setActiveFps] = useState<number>(() => Number(localStorage.getItem('lunaris_stream_fps') || '60'));
  const [activeBitrate, setActiveBitrate] = useState<number>(() => Number(localStorage.getItem('lunaris_stream_bitrate') || '15000'));
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
        return 'auto';
      }
    }

    return savedCodec;
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
  const [activeEncoder, setActiveEncoder] = useState<string>(() => localStorage.getItem('lunaris_stream_encoder') || 'auto');
  const [activeDisplay, setActiveDisplay] = useState<string>(() => localStorage.getItem('lunaris_stream_display') || 'default');
  const [activeVirtualDisplay, setActiveVirtualDisplay] = useState<boolean>(() => localStorage.getItem('lunaris_stream_virtual_display') === 'true');
  const [draftEncoder, setDraftEncoder] = useState<string>(activeEncoder);
  const [draftDisplay, setDraftDisplay] = useState<string>(activeDisplay);
  const [draftVirtualDisplay, setDraftVirtualDisplay] = useState<boolean>(activeVirtualDisplay);
  const [availableEncoders, setAvailableEncoders] = useState<string[]>([]);
  const [agentGpuInfo, setAgentGpuInfo] = useState<string>('');
  const [agentHostOs, setAgentHostOs] = useState<string>('unknown');
  const [activeEncoderStatus, setActiveEncoderStatus] = useState<{ encoder: string; hwType: string; gpuInfo: string; requestedEncoder: string; displayId: string; displayName: string }>({
    encoder: 'Pending',
    hwType: 'Unknown',
    gpuInfo: '',
    requestedEncoder: activeEncoder,
    displayId: '',
    displayName: ''
  });
  const [availableDisplays, setAvailableDisplays] = useState<{id: string; name: string; width: number; height: number; refresh_rate: number; is_primary: boolean}[]>([]);

  const [useNativeClient, setUseNativeClient] = useState<boolean>(() => {
    if (typeof window.RTCPeerConnection === 'undefined') {
      return true;
    }
    // Smart reset once to let the user experience WebRTC inline stream by default after this update
    if (localStorage.getItem('lunaris_reset_use_native_once') !== 'true') {
      localStorage.setItem('lunaris_reset_use_native_once', 'true');
      localStorage.setItem('lunaris_tauri_use_native', 'false');
      return false;
    }
    const val = localStorage.getItem('lunaris_tauri_use_native');
    if (val === null) {
      return false;
    }
    return val === 'true';
  });
  const [draftUseNativeClient, setDraftUseNativeClient] = useState<boolean>(useNativeClient);
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
    const saved = localStorage.getItem('lunaris_canvas_renderer');
    if (saved === 'false') return false;
    if (saved === 'true') return hasProcessor;
    
    return hasProcessor;
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
      setDraftUseNativeClient(useNativeClient);
      setDraftInputProtocol(activeInputProtocol);
      setDraftUseCanvasRenderer(useCanvasRenderer);
      setDraftEncoder(activeEncoder);
      setDraftDisplay(activeDisplay);
      setDraftVirtualDisplay(activeVirtualDisplay);
    }
  }, [showSettingsModal, activeResolution, activeFps, activeBitrate, activeCodec, mouseQueueLimit, useNativeClient, activeInputProtocol, useCanvasRenderer, activeEncoder, activeDisplay, activeVirtualDisplay]);
  const [hideLocalCursor, setHideLocalCursor] = useState<boolean>(() => localStorage.getItem('lunaris_stream_hide_cursor') !== 'false');
  const [isFullscreen, setIsFullscreen] = useState<boolean>(false);
  const [isHeaderVisible, setIsHeaderVisible] = useState<boolean>(true);
  const [isHeaderPinned, setIsHeaderPinned] = useState<boolean>(() => localStorage.getItem('lunaris_header_pinned') === 'true');
  const [showStats, setShowStats] = useState<boolean>(() => localStorage.getItem('lunaris_show_stats') !== 'false');
  const [isWebTransportConnected, setIsWebTransportConnected] = useState<boolean>(false);
  const headerTimeoutRef = useRef<any | null>(null);

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
    jitter: 0,
    connectionType: 'P2P (Direct)'
  });
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

  const getHostPointFromClient = (clientX: number, clientY: number) => {
    const video = videoRef.current;
    if (!video) return null;

    const activeVideo = getActiveVideoElement();
    const rect = cachedVideoRectRef.current.width > 0 && cachedVideoRectRef.current.height > 0
      ? cachedVideoRectRef.current
      : video.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return null;

    const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
    const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;
    const elAspectRatio = rect.width / rect.height;
    const vidAspectRatio = vidWidth / vidHeight;
    let actualVidWidth = rect.width;
    let actualVidHeight = rect.height;
    let offsetX = 0;
    let offsetY = 0;

    if (elAspectRatio > vidAspectRatio) {
      actualVidHeight = rect.height;
      actualVidWidth = rect.height * vidAspectRatio;
      offsetX = (rect.width - actualVidWidth) / 2;
    } else {
      actualVidWidth = rect.width;
      actualVidHeight = rect.width / vidAspectRatio;
      offsetY = (rect.height - actualVidHeight) / 2;
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
    const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any)?.width || 1920;
    const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any)?.height || 1080;
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
    lastHostCursorLocalPredictionAtRef.current = performance.now();
    updateVirtualCursorDOMRef.current();

    const buffer = new ArrayBuffer(13);
    const view = new DataView(buffer);
    view.setUint8(0, 1);
    view.setInt16(1, x16, false);
    view.setInt16(3, y16, false);
    view.setInt16(5, 4096, false);
    view.setInt16(7, 4096, false);
    const seq = (mouseSeqRef.current++) >>> 0;
    view.setUint32(9, seq, false);
    mouseAbsoluteChannel.send(buffer);
  };

  useEffect(() => {
    addLog(`[System Diagnostics] RTCPeerConnection supported: ${typeof window.RTCPeerConnection !== 'undefined'}`);
    addLog(`[System Diagnostics] useNativeClient is set to: ${useNativeClient}`);
  }, [useNativeClient]);

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

      // Prediction cursor: show on lock, hide on unlock
      if (locked) {
        // Lock the Escape key so the browser does not exit pointer lock
        // immediately.  Our key handlers require a 3‑second hold instead.
        if ((navigator as any).keyboard && (navigator as any).keyboard.lock) {
          (navigator as any).keyboard.lock(["Escape"]).catch(() => {});
        }
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
      
      // Implement browser Keyboard Lock when entering fullscreen.
      // In Chromium-based browsers, this forces the Escape key to behave like client-qml
      // (requiring the user to press and hold Escape for 2 seconds to exit).
      if (isFull) {
        if ((navigator as any).keyboard && (navigator as any).keyboard.lock) {
          (navigator as any).keyboard.lock(["Escape"]).catch((err: any) => {
            console.error("Failed to lock keyboard Escape:", err);
          });
        }
      } else {
        if ((navigator as any).keyboard && (navigator as any).keyboard.unlock) {
          (navigator as any).keyboard.unlock();
        }
      }
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

      // 1. Process pointer-locked mouse moves via absolute positioning.
      //    We send absolute positions (normalised to [0, 4095]) instead of
      //    raw relative deltas so the agent can map them to the hostʼs
      //    actual monitor resolution — exactly the same code path as
      //    unlocked absolute mode.  This avoids the scaling mismatch that
      //    occurs when vidWidth ≠ monitorWidth and keeps the two cursors
      //    in sync regardless of host DPI / capture resolution.
      const dx = accDxRef.current;
      const dy = accDyRef.current;
      if (dx !== 0 || dy !== 0) {
        const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
        if (mouseAbsoluteChannel && mouseAbsoluteChannel.readyState === "open") {
          const buffered = mouseAbsoluteChannel.bufferedAmount;
          if (buffered === undefined || buffered <= qLimit) {
            const activeVideo = getActiveVideoElement();
            const video = videoRef.current;
            const rect = cachedVideoRectRef.current;
            const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any)?.width || 1920;
            const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any)?.height || 1080;
            let scaledDx = dx;
            let scaledDy = dy;
            if (rect.width > 0 && rect.height > 0) {
              const elAspectRatio = rect.width / rect.height;
              const vidAspectRatio = vidWidth / vidHeight;
              let actualVidWidth = rect.width;
              let actualVidHeight = rect.height;
              if (elAspectRatio > vidAspectRatio) {
                actualVidHeight = rect.height;
                actualVidWidth = rect.height * vidAspectRatio;
              } else {
                actualVidWidth = rect.width;
                actualVidHeight = rect.width / vidAspectRatio;
              }
              scaledDx = (dx / Math.max(1, actualVidWidth)) * vidWidth;
              scaledDy = (dy / Math.max(1, actualVidHeight)) * vidHeight;
            }

            // Update the predicted cursor position from accumulated deltas
            localCursorPosRef.current = {
              x: Math.max(0, Math.min(vidWidth, localCursorPosRef.current.x + scaledDx)),
              y: Math.max(0, Math.min(vidHeight, localCursorPosRef.current.y + scaledDy)),
            };

            // Send predicted position as absolute (normalised 0..4095) so the
            // agent applies the same monitor-resolution mapping as unlocked mode.
            const x16 = Math.round((localCursorPosRef.current.x / Math.max(1, vidWidth)) * 4096.0);
            const y16 = Math.round((localCursorPosRef.current.y / Math.max(1, vidHeight)) * 4096.0);

            const buffer = new ArrayBuffer(13);
            const view = new DataView(buffer);
            view.setUint8(0, 1); // Type 1: MousePosition (Absolute)
            view.setInt16(1, x16, false);
            view.setInt16(3, y16, false);
            view.setInt16(5, 4096, false);
            view.setInt16(7, 4096, false);
            const seq = (mouseSeqRef.current++) >>> 0;
            view.setUint32(9, seq, false);
            mouseAbsoluteChannel.send(buffer);

            lastHostCursorLocalPredictionAtRef.current = performance.now();
            updateVirtualCursorDOMRef.current();

            // Clear accumulated deltas only on successful transmission
            accDxRef.current = 0;
            accDyRef.current = 0;
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
              const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
              const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;

              const elAspectRatio = elWidth / elHeight;
              const vidAspectRatio = vidWidth / vidHeight;

              let actualVidWidth = elWidth;
              let actualVidHeight = elHeight;
              let offsetX = 0;
              let offsetY = 0;

              if (elAspectRatio > vidAspectRatio) {
                actualVidHeight = elHeight;
                actualVidWidth = elHeight * vidAspectRatio;
                offsetX = (elWidth - actualVidWidth) / 2;
              } else {
                actualVidWidth = elWidth;
                actualVidHeight = elWidth / vidAspectRatio;
                offsetY = (elHeight - actualVidHeight) / 2;
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
              if (!(hostCursorMouseDownRef.current && agentHostOs === "windows" && hostCursorPosRef.current.inWindowMoveSize)) {
                const hostCursor = hostCursorPosRef.current;
                lastHostCursorLocalPredictionAtRef.current = performance.now();
                updateHostCursorDOM(predictedX, predictedY, true, hostCursor.kind, null);
              }

              const buffer = new ArrayBuffer(13);
              const view = new DataView(buffer);
              view.setUint8(0, 1); // Type 1: MousePosition
              view.setInt16(1, x16, false);
              view.setInt16(3, y16, false);
              view.setInt16(5, 4096, false); // referenceWidth
              view.setInt16(7, 4096, false); // referenceHeight
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

    const tauri = (window as any).__TAURI__;
    if (tauri && useNativeClient && selectedAppId !== null) {
      const codecDecision = getCodecDecision(activeCodec);
      const resolvedCodec = codecDecision.resolved;
      if (codecDecision.fellBack) {
        console.warn(`Native client codec fallback: requested=${codecDecision.requested}, resolved=${resolvedCodec}. ${codecDecision.reason}. ${codecSupportSummary()}`);
      }
      const resStr = activeResolution === '720p' ? '1280x720' : activeResolution === '540p' ? '960x540' : '1920x1080';
      const backendHost = getBackendHost();
      const serverUrl = `${window.location.protocol === 'https:' ? 'https:' : 'http:'}//${backendHost}`;

      tauri.core.invoke('launch_native_client', {
        hostId,
        serverUrl,
        token,
        res: resStr,
        fps: String(activeFps),
        bitrate: String(activeBitrate),
        codec: resolvedCodec,
        appId: selectedAppId,
        mouseQueueLimit: String(mouseQueueLimit),
        hostName,
        encoder: activeEncoder,
        displayId: activeDisplay,
        virtualDisplay: activeVirtualDisplay,
        inputProtocol: activeInputProtocol
      }).then(() => {
        onBack();
      }).catch((err: any) => {
        console.error("Failed to launch native client:", err);
        alert("Failed to launch native client: " + err);
      });
      return;
    }

    // Resolve the requested codec once before creating SDP and RequestSession.
    const codecDecision = getCodecDecision(activeCodec);
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
        // Also request host capabilities (displays & encoders)
        ws.send(JSON.stringify({
          event: "Signaling",
          data: {
            type: "GetCapabilities",
            payload: { target_id: hostId }
          }
        }));
        addLog("Sent GetCapabilities command.");
      } else {
        setStatus("Signaling...");
        let width = 1920;
        let height = 1080;
        if (activeResolution === '720p') {
          width = 1280;
          height = 720;
        } else if (activeResolution === '540p') {
          width = 960;
          height = 540;
        }

        // Request session
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
              virtual_display: activeVirtualDisplay ? true : undefined
            }
          }
        }));
        addLog(`Sent RequestSession command for app ${selectedAppId} (res: ${activeResolution}, fps: ${activeFps}, bitrate: ${activeBitrate}Kbps, codec: ${resolvedCodec}).`);
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
            if (payload.displays) setAvailableDisplays(payload.displays);
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
                payload.webtransport_cert_hash
              );
            }
            break;
            
          case "IceCandidate":
            addLog("Received remote ICE candidate.");
            const candIp = parseCandidateIp(payload.candidate.candidate);
            if (candIp && !agentIpsRef.current.includes(candIp)) {
              agentIpsRef.current.push(candIp);
              addLog(`Found remote candidate IP: ${candIp}`);
              if (activeInputProtocol === "webtransport" && !wtRef.current && wtPortRef.current && wtCertHashRef.current) {
                attemptWebTransport();
              }
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
  }, [hostId, activeResolution, activeCodec, token, hostName, selectedAppId, useNativeClient, activeInputProtocol, useCanvasRenderer, activeEncoder, activeDisplay, activeVirtualDisplay]);

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
      const vk = KEY_TO_VK[code] || 0;
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



  // Global window-level keyboard listeners when streaming is active
  useEffect(() => {
    if (status !== "Streaming") return;

    const handleGlobalKeyDown = (e: KeyboardEvent) => {
      const activeEl = document.activeElement;
      if (
        activeEl &&
        (activeEl.tagName === 'INPUT' ||
         activeEl.tagName === 'SELECT' ||
         activeEl.tagName === 'TEXTAREA' ||
         (activeEl as HTMLElement).isContentEditable)
      ) {
        return;
      }

      if (showSettingsModal) return;

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
              document.exitPointerLock();
              if (document.fullscreenElement) {
                document.exitFullscreen();
              }
            } else {
              setEscapeHoldProgress(Math.min(0.99, elapsed / HOLD_MS));
              escapeHoldTimerRef.current = window.setTimeout(tick, 50);
            }
          };
          escapeHoldTimerRef.current = window.setTimeout(tick, 50);
        }
        return;
      }

      e.preventDefault();
      sendKeyEvent(e.code, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, true);
    };

    const handleGlobalKeyUp = (e: KeyboardEvent) => {
      const activeEl = document.activeElement;
      if (
        activeEl &&
        (activeEl.tagName === 'INPUT' ||
         activeEl.tagName === 'SELECT' ||
         activeEl.tagName === 'TEXTAREA' ||
         (activeEl as HTMLElement).isContentEditable)
      ) {
        return;
      }

      if (showSettingsModal) return;

      // Cancel the ESC hold timer on release
      if (e.code === 'Escape' && isPointerLockedRef.current) {
        e.preventDefault();
        if (escapeHoldTimerRef.current !== null) {
          window.clearTimeout(escapeHoldTimerRef.current);
          escapeHoldTimerRef.current = null;
        }
        setEscapeHoldProgress(0);
        return;
      }

      e.preventDefault();
      sendKeyEvent(e.code, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, false);
    };

    window.addEventListener('keydown', handleGlobalKeyDown);
    window.addEventListener('keyup', handleGlobalKeyUp);

    return () => {
      window.removeEventListener('keydown', handleGlobalKeyDown);
      window.removeEventListener('keyup', handleGlobalKeyUp);
    };
  }, [status, showSettingsModal]);

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

        setStats(prev => ({
          ...prev,
          ping: currentRtt,
          decodeLatency: videoDecodeLatency,
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

  const attemptWebTransport = async () => {
    if (wtRef.current || wtConnectingRef.current) return;
    if (!wtPortRef.current || !wtCertHashRef.current) return;
    
    wtConnectingRef.current = true;
    const port = wtPortRef.current;
    const certHash = wtCertHashRef.current;
    
    // Build list of IPs to try
    const ips = [...agentIpsRef.current];
    const sigHost = getBackendHost().split(':')[0];
    if (!ips.includes(sigHost)) {
      ips.push(sigHost);
    }
    if (!ips.includes("127.0.0.1")) {
      ips.push("127.0.0.1");
    }
    
    addLog(`Attempting WebTransport connection with IPs: ${ips.join(', ')} on port ${port}`);
    
    const hashArray = hexToUint8Array(certHash);
    
    for (const ip of ips) {
      try {
        const url = `https://${ip}:${port}/`;
        addLog(`WebTransport: Connecting to ${url}...`);
        const transport = new (window as any).WebTransport(url, {
          serverCertificateHashes: [
            {
              algorithm: 'sha-256',
              value: hashArray
            }
          ]
        });
        
        await transport.ready;
        addLog(`WebTransport: Connected successfully to ${url}!`);
        
        wtRef.current = transport;
        wtDatagramWriterRef.current = transport.datagrams.writable.getWriter();
        setIsWebTransportConnected(true);
        
        // Handle transport close to revert to WebRTC automatically
        transport.closed.then(() => {
          addLog("WebTransport: Connection closed, reverting to WebRTC.");
          setIsWebTransportConnected(false);
        }).catch(() => {
          setIsWebTransportConnected(false);
        });
        
        // Wrap existing data channels to send via WebTransport
        const WT_CHANNELS = {
          keyboard: 7,
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
      if (message?.type !== 'host_cursor') return;

      const x = Number(message.x);
      const y = Number(message.y);
      const visible = Boolean(message.visible);
      const kind = normalizeHostCursorKind(message.kind);
      const image = parseHostCursorImage(message.image);
      const inWindowMoveSize = Boolean(message.in_window_move_size);
      if (!Number.isFinite(x) || !Number.isFinite(y)) return;

      applyNativeCursorImage(localCursorImageRef.current, localCursorImageMetricsRef, kind, image);

      const localPredictionFresh = performance.now() - lastHostCursorLocalPredictionAtRef.current < 140;
      // Always keep the reference position current so that future syncs
      // (e.g. pointer-lock entry) use the real host cursor position.
      hostCursorPosRef.current = { x, y, visible, kind, inWindowMoveSize };
      if (localPredictionFresh && visible) {
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
    webtransportCertHash?: string
  ) => {
    setStatus("Establishing WebRTC...");
    
    wtPortRef.current = webtransportPort;
    wtCertHashRef.current = webtransportCertHash;
    if (activeInputProtocol === "webtransport" && webtransportPort && webtransportCertHash) {
      attemptWebTransport();
    }
    
    // Create RTCPeerConnection
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
      const WT_CHANNELS = {
        keyboard: 7,
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
    const codecForSession = getCodecDecision(activeCodec).resolved;
    try {
      const transceivers = pc.getTransceivers();
      const videoTransceiver = transceivers.find(t => t.receiver.track.kind === 'video');
      if (videoTransceiver && RTCRtpReceiver.getCapabilities) {
        const caps = RTCRtpReceiver.getCapabilities('video');
        if (caps && caps.codecs) {
          // Build ordered codec preference: put selected codec first, keep RED/ULPFEC/FlexFEC
          const mimeTarget = codecForSession === 'h265' ? ['video/h265', 'video/hevc']
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

      self.onmessage = async (e) => {
        const { type } = e.data;
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
                  if (canvas) {
                    if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
                      canvas.width = frame.displayWidth;
                      canvas.height = frame.displayHeight;
                      self.postMessage({ type: 'resize', width: frame.displayWidth, height: frame.displayHeight });
                    }

                    if (ctx) {
                      ctx.drawImage(frame, 0, 0, frame.displayWidth, frame.displayHeight);
                      self.postMessage({ type: 'frameDrawn' });
                    }
                  }
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
              if (canvas) {
                if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
                  canvas.width = frame.displayWidth;
                  canvas.height = frame.displayHeight;
                  self.postMessage({ type: 'resize', width: frame.displayWidth, height: frame.displayHeight });
                }

                if (ctx) {
                  ctx.drawImage(frame, 0, 0, frame.displayWidth, frame.displayHeight);
                  self.postMessage({ type: 'frameDrawn' });
                }
              }
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

    // Clear video src and stop canvas loop
    if (videoRef.current && videoRef.current instanceof HTMLVideoElement) {
      videoRef.current.srcObject = null;
    }
    if (hiddenVideoRef.current) {
      hiddenVideoRef.current.srcObject = null;
    }
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

    document.exitPointerLock();
  };

  // Request pointer lock for relative controls.
  // navigator.keyboard.lock (required for the 3 s ESC hold) only takes
  // effect while fullscreen is active, so we enter fullscreen alongside
  // pointer lock.  The fullscreenchange handler (which fires shortly
  // after) calls keyboard.lock to intercept ESC.
  const togglePointerLock = () => {
    if (!videoRef.current) return;
    if (document.pointerLockElement === videoRef.current) {
      document.exitPointerLock();
    } else {
      // Enter fullscreen in parallel — keyboard.lock requires it.
      if (!document.fullscreenElement && containerRef.current) {
        containerRef.current.requestFullscreen().catch(() => {});
      }
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

  const updateSetting = (key: string, value: any) => {
    if (key === 'res') {
      localStorage.setItem('lunaris_stream_res', value);
      setActiveResolution(value);
      setDraftResolution(value);
      if (value !== activeResolution) setStatus("Connecting...");
    } else if (key === 'fps') {
      const numValue = Number(value);
      localStorage.setItem('lunaris_stream_fps', String(numValue));
      setActiveFps(numValue);
      setDraftFps(numValue);
      // Dynamic: no reconnect needed, useEffect sends command via data channel
    } else if (key === 'bitrate') {
      const numValue = Number(value);
      localStorage.setItem('lunaris_stream_bitrate', String(numValue));
      setActiveBitrate(numValue);
      setDraftBitrate(numValue);
      // Dynamic: no reconnect needed, useEffect sends command via data channel
    } else if (key === 'codec') {
      localStorage.setItem('lunaris_stream_codec', value);
      setActiveCodec(value);
      setDraftCodec(value);
      if (value !== activeCodec) setStatus("Connecting...");
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
      localStorage.setItem('lunaris_stream_display', value);
      setActiveDisplay(value);
      setDraftDisplay(value);
      if (value !== activeDisplay) setStatus("Connecting...");
    }
  };

  const handleMinimize = () => {
    const tauri = (window as any).__TAURI__;
    if (tauri && tauri.window) {
      try {
        const appWindow = tauri.window.getCurrentWindow();
        appWindow.minimize();
      } catch (err) {
        console.error("Failed to minimize Tauri window:", err);
      }
    }
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

    if (
      !cursorEl
      || !video
      || !wrapper
      || !visible
      || isHardwareMouseActiveRef.current
      || (hostCursorMouseDownRef.current && agentHostOs === "windows" && inWindowMoveSize)
    ) {
      if (cursorEl) cursorEl.style.display = 'none';
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
        cursorEl.style.display = 'none';
        return;
      }

      const cursorAsset = HOST_CURSOR_ASSETS[cursorKind];
      if (imageEl && !imageEl.src.endsWith(cursorAsset.src)) {
        imageEl.src = cursorAsset.src;
        imageEl.style.width = '32px';
        imageEl.style.height = '32px';
        imageEl.style.filter = 'drop-shadow(0 0 1px rgba(0,0,0,0.95)) drop-shadow(0 0 2px rgba(255,255,255,0.85))';
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
      cursorEl.style.display = 'none';
      return;
    }

    const activeVideo = getActiveVideoElement();
    const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
    const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;

    const elAspectRatio = rect.width / rect.height;
    const vidAspectRatio = vidWidth / vidHeight;
    let actualVidWidth = rect.width;
    let actualVidHeight = rect.height;
    let offsetX = 0;
    let offsetY = 0;

    if (elAspectRatio > vidAspectRatio) {
      actualVidHeight = rect.height;
      actualVidWidth = rect.height * vidAspectRatio;
      offsetX = (rect.width - actualVidWidth) / 2;
    } else {
      actualVidWidth = rect.width;
      actualVidHeight = rect.width / vidAspectRatio;
      offsetY = (rect.height - actualVidHeight) / 2;
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
    if (agentHostOs === "windows" && hostCursorMouseDownRef.current && hostCursor.inWindowMoveSize) {
      cursorEl.style.display = 'none';
      return;
    }

    const shouldUseWindowsPrediction = agentHostOs === "windows" && hasNativeCursorImageRef.current;
    const shouldShowTrackpad = touchModeRef.current === 'trackpad'
      && status === 'Streaming'
      && !isHardwareMouseActiveRef.current;
    const shouldShowHardwarePrediction = status === 'Streaming'
      && isHardwareMouseActiveRef.current
      && (hideLocalCursor || shouldUseWindowsPrediction);

    if (!shouldShowTrackpad && !shouldShowHardwarePrediction) {
      cursorEl.style.display = 'none';
      return;
    }

    const rect = video.getBoundingClientRect();
    const wrapperRect = wrapper.getBoundingClientRect();

    if (rect.width <= 0 || rect.height <= 0 || wrapperRect.width <= 0 || wrapperRect.height <= 0) {
      cursorEl.style.display = 'none';
      return;
    }

    const activeVideo = getActiveVideoElement();
    const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
    const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;

    const elWidth = rect.width;
    const elHeight = rect.height;

    let actualVidWidth = elWidth;
    let actualVidHeight = elHeight;
    let offsetX = 0;
    let offsetY = 0;

    const elAspectRatio = elWidth / elHeight;
    const vidAspectRatio = vidWidth / vidHeight;

    if (elAspectRatio > vidAspectRatio) {
      actualVidHeight = elHeight;
      actualVidWidth = elHeight * vidAspectRatio;
      offsetX = (elWidth - actualVidWidth) / 2;
    } else {
      actualVidWidth = elWidth;
      actualVidHeight = elWidth / vidAspectRatio;
      offsetY = (elHeight - actualVidHeight) / 2;
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
        cursorEl.style.display = 'none';
        return;
      }

      const cursorAsset = HOST_CURSOR_ASSETS[cursorKind];
      if (imageEl && !imageEl.src.endsWith(cursorAsset.src)) {
        imageEl.src = cursorAsset.src;
        imageEl.style.width = '32px';
        imageEl.style.height = '32px';
        imageEl.style.filter = 'drop-shadow(0 0 1px rgba(0,0,0,0.95)) drop-shadow(0 0 2px rgba(255,255,255,0.85))';
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
  const getDistance = (t1: React.Touch | Touch, t2: React.Touch | Touch) => {
    const dx = t1.clientX - t2.clientX;
    const dy = t1.clientY - t2.clientY;
    return Math.sqrt(dx * dx + dy * dy);
  };

  const getMidpoint = (t1: React.Touch | Touch, t2: React.Touch | Touch) => {
    return {
      x: (t1.clientX + t2.clientX) / 2,
      y: (t1.clientY + t2.clientY) / 2
    };
  };

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
      const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
      const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;

      let xNorm = 0.5;
      let yNorm = 0.5;

      const elAspectRatio = elWidth / elHeight;
      const vidAspectRatio = vidWidth / vidHeight;

      let actualVidWidth = elWidth;
      let actualVidHeight = elHeight;
      let offsetX = 0;
      let offsetY = 0;

      if (elAspectRatio > vidAspectRatio) {
        actualVidHeight = elHeight;
        actualVidWidth = elHeight * vidAspectRatio;
        offsetX = (elWidth - actualVidWidth) / 2;
      } else {
        actualVidWidth = elWidth;
        actualVidHeight = elWidth / vidAspectRatio;
        offsetY = (elHeight - actualVidHeight) / 2;
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
      view.setInt16(5, 4096, false); // referenceWidth
      view.setInt16(7, 4096, false); // referenceHeight
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

  useEffect(() => {
    const wrapper = viewportWrapperRef.current;
    if (!wrapper) return;

    const bypassSelectors = '.stream-header-bar, .stream-header-pull-tab, .mobile-footer-bar, .mobile-footer-pull-tab, .mobile-controls-drawer, .stream-settings-overlay';

    const onTouchStart = (e: TouchEvent) => {
      if (e.target && (e.target as HTMLElement).closest(bypassSelectors)) {
        return; // Allow button clicks inside menu/header to propagate normally
      }
      handleTouchStart(e as unknown as React.TouchEvent<HTMLDivElement>);
    };

    const onTouchMove = (e: TouchEvent) => {
      if (e.target && (e.target as HTMLElement).closest(bypassSelectors)) {
        return;
      }
      handleTouchMove(e as unknown as React.TouchEvent<HTMLDivElement>);
    };

    const onTouchEnd = (e: TouchEvent) => {
      if (e.target && (e.target as HTMLElement).closest(bypassSelectors)) {
        return;
      }
      handleTouchEnd(e as unknown as React.TouchEvent<HTMLDivElement>);
    };

    wrapper.addEventListener('touchstart', onTouchStart, { passive: false });
    wrapper.addEventListener('touchmove', onTouchMove, { passive: false });
    wrapper.addEventListener('touchend', onTouchEnd, { passive: false });

    return () => {
      wrapper.removeEventListener('touchstart', onTouchStart);
      wrapper.removeEventListener('touchmove', onTouchMove);
      wrapper.removeEventListener('touchend', onTouchEnd);
    };
  }, [status, touchMode, zoomScale, zoomPan, useTouchOffset]);

  const handleTouchStart = (e: React.TouchEvent<HTMLDivElement>) => {
    if (status !== "Streaming") return;
    
    if (isHardwareMouseActiveRef.current) {
      isHardwareMouseActiveRef.current = false;
    }
    
    // Prevent browser from emulating mouse events (mousemove, mousedown, click)
    e.preventDefault();

    const video = videoRef.current;
    if (!video) return;

    mouseAccumulatorXRef.current = 0;
    mouseAccumulatorYRef.current = 0;

    if (e.touches.length === 1) {
      hasCenteredThisTouchRef.current = false;
      // If there's a pending click-up timer from a previous tap,
      // it means the left mouse button is currently held down on the host.
      let wasPendingClickUp = false;
      if (clickUpTimerRef.current) {
        clearTimeout(clickUpTimerRef.current);
        clickUpTimerRef.current = null;
        wasPendingClickUp = true;
      }

      const touch = e.touches[0];
      const now = performance.now();
      
      wasMultiTouchRef.current = false; // Reset multi-touch flag

      const isDoubleTap = now - lastTouchTapTimeRef.current < 500;
      if (isDoubleTap) {
        lastTouchTapTimeRef.current = 0; // Prevent chaining of double tap
      }

      touchStartPosRef.current = { x: touch.clientX, y: touch.clientY };
      touchStartInitialPosRef.current = { x: touch.clientX, y: touch.clientY };
      touchStartTimeRef.current = now;
      initialZoomPanRef.current = { ...zoomPan };
      longPressTriggeredRef.current = false;

      if (touchMode === "trackpad") {
        isDraggingRef.current = false;
        
        // Cancel any existing long press timer
        if (longPressTimerRef.current) {
          clearTimeout(longPressTimerRef.current);
          longPressTimerRef.current = null;
        }

        if (isDoubleTap) {
          // Double-Tap and Hold gesture: trigger drag instantly (0ms delay)
          isDraggingRef.current = true;
          if (!wasPendingClickUp) {
            sendMouseClickAction(1, true); // left click down
          }
          if (navigator.vibrate) {
            navigator.vibrate(40); // Haptic feedback
          }
        } else {
          if (wasPendingClickUp) {
            sendMouseClickAction(1, false);
          }
          // Single touch down: Start long press timer (600ms) for right click
          longPressTimerRef.current = setTimeout(() => {
            const mouseReliableChannel = channelsRef.current["mouse_reliable"];
            if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
              sendMouseClickAction(3, true); // right click down
              setTimeout(() => sendMouseClickAction(3, false), 30); // right click up
              
              // Haptic feedback for successful right click trigger
              if (navigator.vibrate) {
                navigator.vibrate(50);
              }
              longPressTriggeredRef.current = true;
            }
          }, 600);
        }
      } else {
        // Direct Mode: Mark click as pending instead of clicking down immediately
        isDirectClickPendingRef.current = true;

        // Start long press timer for right click
        if (longPressTimerRef.current) {
          clearTimeout(longPressTimerRef.current);
        }
        longPressTimerRef.current = setTimeout(() => {
          const mouseReliableChannel = channelsRef.current["mouse_reliable"];
          if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
            sendTouchAbsolutePos(touch.clientX, touch.clientY);
            sendMouseClickAction(3, true);  // right click down
            setTimeout(() => sendMouseClickAction(3, false), 30); // right click up
            
            if (navigator.vibrate) {
              navigator.vibrate(50);
            }
            longPressTriggeredRef.current = true;
            isDirectClickPendingRef.current = false; // Cancel left click down
          }
        }, 600);
      }
    } else if (e.touches.length === 2) {
      wasMultiTouchRef.current = true; // Mark as multi-touch gesture
      isDirectClickPendingRef.current = false; // Cancel any pending direct left click

      // Cancel long press timer for multi-touch gestures
      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
        longPressTimerRef.current = null;
      }
      isDraggingRef.current = false;

      initialTouchDistanceRef.current = getDistance(e.touches[0], e.touches[1]);
      initialZoomScaleRef.current = zoomScale;
      initialZoomPanRef.current = { ...zoomPan };
      initialTouchMidpointRef.current = getMidpoint(e.touches[0], e.touches[1]);
      initialLocalCursorPosRef.current = { ...localCursorPosRef.current };

      // For 2-finger tap right click detection
      twoFingerTouchStartTimeRef.current = performance.now();
      isTwoFingerTapPendingRef.current = true;
    }
  };

  const handleTouchMove = (e: React.TouchEvent<HTMLDivElement>) => {
    if (status !== "Streaming") return;
    
    // Always prevent default page scrolling or browser navigation inside the player
    e.preventDefault();

    const video = videoRef.current;
    if (!video) return;
    const activeVideo = getActiveVideoElement();

    if (e.touches.length === 1) {
      const touch = e.touches[0];
      const dx = touch.clientX - touchStartPosRef.current.x;
      const dy = touch.clientY - touchStartPosRef.current.y;
      
      const totalDx = touch.clientX - touchStartInitialPosRef.current.x;
      const totalDy = touch.clientY - touchStartInitialPosRef.current.y;
      const totalMovement = Math.sqrt(totalDx * totalDx + totalDy * totalDy);

      // Cancel right click (600ms) timer if movement > 15px
      if (totalMovement > 15 && longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
        longPressTimerRef.current = null;
      }

      if (zoomScale > 1) {
        if (touchMode === "trackpad") {
          const wrapper = viewportWrapperRef.current;
          if (wrapper) {
            const wrapperRect = wrapper.getBoundingClientRect();
            const W_wrapper = wrapperRect.width;
            const H_wrapper = wrapperRect.height;

            const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
            const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;

            const wrapperAspectRatio = W_wrapper / H_wrapper;
            const vidAspectRatio = vidWidth / vidHeight;

            let W_base = W_wrapper;
            let H_base = H_wrapper;
            let offsetX = 0;
            let offsetY = 0;

            if (wrapperAspectRatio > vidAspectRatio) {
              H_base = H_wrapper;
              W_base = H_wrapper * vidAspectRatio;
              offsetX = (W_wrapper - W_base) / 2;
            } else {
              W_base = W_wrapper;
              H_base = W_wrapper / vidAspectRatio;
              offsetY = (H_wrapper - H_base) / 2;
            }

            const actualVidWidth = W_base * zoomScale;
            const actualVidHeight = H_base * zoomScale;

            // Apply trackpad acceleration
            const distance = Math.sqrt(dx * dx + dy * dy);
            let accel = 1.0;
            if (distance > 2) {
              accel = Math.min(3.0, 1.0 + (distance - 2) * 0.15);
            }
            const s = 2.2 * accel;

            // Convert client screen delta (dx, dy) to host resolution delta
            const rawDeltaX = (dx * s / actualVidWidth) * vidWidth;
            const rawDeltaY = (dy * s / actualVidHeight) * vidHeight;

            // Add accumulated sub-pixel movements
            const totalDeltaX = rawDeltaX + mouseAccumulatorXRef.current;
            const totalDeltaY = rawDeltaY + mouseAccumulatorYRef.current;

            // Round to integer deltas
            const relX = Math.round(totalDeltaX);
            const relY = Math.round(totalDeltaY);

            // Store the fractional remainders
            mouseAccumulatorXRef.current = totalDeltaX - relX;
            mouseAccumulatorYRef.current = totalDeltaY - relY;

            // Update local cursor position estimate in host coordinates
            let newX = localCursorPosRef.current.x + relX;
            let newY = localCursorPosRef.current.y + relY;

            newX = Math.max(0, Math.min(vidWidth, newX));
            newY = Math.max(0, Math.min(vidHeight, newY));

            localCursorPosRef.current = { x: newX, y: newY };

            // Send absolute mouse position to host instead of relative in trackpad mode to avoid drift
            const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
            if (mouseAbsoluteChannel && mouseAbsoluteChannel.readyState === "open" && (relX !== 0 || relY !== 0)) {
              const xNorm = newX / vidWidth;
              const yNorm = newY / vidHeight;
              const x16 = Math.round(xNorm * 4096.0);
              const y16 = Math.round(yNorm * 4096.0);

              const buffer = new ArrayBuffer(13);
              const view = new DataView(buffer);
              view.setUint8(0, 1); // Type 1: MousePosition (Absolute)
              view.setInt16(1, x16, false);
              view.setInt16(3, y16, false);
              view.setInt16(5, 4096, false); // referenceWidth
              view.setInt16(7, 4096, false); // referenceHeight
              const seq = (mouseSeqRef.current++) >>> 0;
              view.setUint32(9, seq, false);
              mouseAbsoluteChannel.send(buffer);
            }

            // Viewport horizontal panning keeps cursor centered
            const targetVisualX = W_wrapper / 2;
            let newPanX = targetVisualX - (newX / vidWidth) * actualVidWidth - offsetX * zoomScale;

            const limitX1 = -offsetX * zoomScale;
            const limitX2 = W_wrapper - actualVidWidth - offsetX * zoomScale;
            const minPanX = Math.min(limitX1, limitX2);
            const maxPanX = Math.max(limitX1, limitX2);

            const clampedPanX = Math.max(minPanX, Math.min(maxPanX, newPanX));
            const clampedPanY = zoomPan.y; // Keep Y-axis locked to current pan

            setZoomPan({ x: clampedPanX, y: clampedPanY });

            // Sync visual cursor DOM directly and instantly
            const cursorEl = localCursorRef.current;
            if (cursorEl) {
              const leftWrapper = (newX / vidWidth) * actualVidWidth + offsetX * zoomScale + clampedPanX;
              const topWrapper = (newY / vidHeight) * actualVidHeight + offsetY * zoomScale + clampedPanY;
              cursorEl.style.left = `${leftWrapper}px`;
              cursorEl.style.top = `${topWrapper}px`;
            }

            touchStartPosRef.current = { x: touch.clientX, y: touch.clientY };
          }
        } else {
          // Direct Mode with Zoom: standard panning using initial reference
          setZoomPan({
            x: initialZoomPanRef.current.x + totalDx,
            y: initialZoomPanRef.current.y + totalDy
          });
        }
      } else if (touchMode === "trackpad") {
        const rect = video.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
          const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;

          // Apply trackpad acceleration
          const distance = Math.sqrt(dx * dx + dy * dy);
          let accel = 1.0;
          if (distance > 2) {
            accel = Math.min(3.0, 1.0 + (distance - 2) * 0.15);
          }
          const s = 2.2 * accel;

          // Convert client screen delta (dx, dy) to host resolution delta
          const rawDeltaX = (dx * s / rect.width) * vidWidth;
          const rawDeltaY = (dy * s / rect.height) * vidHeight;

          // Add accumulated sub-pixel movements
          const totalDeltaX = rawDeltaX + mouseAccumulatorXRef.current;
          const totalDeltaY = rawDeltaY + mouseAccumulatorYRef.current;

          // Round to integer deltas
          const relX = Math.round(totalDeltaX);
          const relY = Math.round(totalDeltaY);

          // Store the fractional remainders
          mouseAccumulatorXRef.current = totalDeltaX - relX;
          mouseAccumulatorYRef.current = totalDeltaY - relY;

          // Update local cursor position estimate in host coordinates
          let newX = localCursorPosRef.current.x + relX;
          let newY = localCursorPosRef.current.y + relY;

          newX = Math.max(0, Math.min(vidWidth, newX));
          newY = Math.max(0, Math.min(vidHeight, newY));

          localCursorPosRef.current = { x: newX, y: newY };

          // Send absolute mouse position to host instead of relative in trackpad mode to avoid drift
          const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
          if (mouseAbsoluteChannel && mouseAbsoluteChannel.readyState === "open" && (relX !== 0 || relY !== 0)) {
            const xNorm = newX / vidWidth;
            const yNorm = newY / vidHeight;
            const x16 = Math.round(xNorm * 4096.0);
            const y16 = Math.round(yNorm * 4096.0);

            const buffer = new ArrayBuffer(13);
            const view = new DataView(buffer);
            view.setUint8(0, 1); // Type 1: MousePosition (Absolute)
            view.setInt16(1, x16, false);
            view.setInt16(3, y16, false);
            view.setInt16(5, 4096, false); // referenceWidth
            view.setInt16(7, 4096, false); // referenceHeight
            const seq = (mouseSeqRef.current++) >>> 0;
            view.setUint32(9, seq, false);
            mouseAbsoluteChannel.send(buffer);
          }

          updateVirtualCursorDOM();
          touchStartPosRef.current = { x: touch.clientX, y: touch.clientY };
        }
      } else {
        // Direct Touchscreen Mode:
        if (isDirectClickPendingRef.current) {
          if (totalMovement > 15) {
            // Drag started: send left click down at initial touch point first
            sendTouchAbsolutePos(touchStartInitialPosRef.current.x, touchStartInitialPosRef.current.y);
            sendMouseClickAction(1, true);
            isDirectClickPendingRef.current = false;
          }
        }
        
        if (!isDirectClickPendingRef.current) {
          sendTouchAbsolutePos(touch.clientX, touch.clientY);
        }
      }
    } else if (e.touches.length === 2) {
      const dist = getDistance(e.touches[0], e.touches[1]);
      if (initialTouchDistanceRef.current > 0) {
        const factor = dist / initialTouchDistanceRef.current;
        const newScale = Math.max(1, Math.min(5, initialZoomScaleRef.current * factor));

        // If scale changes significantly, or fingers move, cancel 2-finger tap right click
        if (Math.abs(factor - 1) > 0.08) {
          isTwoFingerTapPendingRef.current = false;
        }

        const rect = video.getBoundingClientRect();
        const wrapper = viewportWrapperRef.current;
        if (rect.width > 0 && rect.height > 0 && wrapper) {
          const wrapperRect = wrapper.getBoundingClientRect();
          const W_wrapper = wrapperRect.width;
          const H_wrapper = wrapperRect.height;

          const vidWidth = activeVideo?.videoWidth && activeVideo.videoWidth > 0 ? activeVideo.videoWidth : (video as any).width || 1920;
          const vidHeight = activeVideo?.videoHeight && activeVideo.videoHeight > 0 ? activeVideo.videoHeight : (video as any).height || 1080;

          const wrapperAspectRatio = W_wrapper / H_wrapper;
          const vidAspectRatio = vidWidth / vidHeight;

          let W_base = W_wrapper;
          let H_base = H_wrapper;
          let offsetX = 0;
          let offsetY = 0;

          if (wrapperAspectRatio > vidAspectRatio) {
            H_base = H_wrapper;
            W_base = H_wrapper * vidAspectRatio;
            offsetX = (W_wrapper - W_base) / 2;
          } else {
            W_base = W_wrapper;
            H_base = W_wrapper / vidAspectRatio;
            offsetY = (H_wrapper - H_base) / 2;
          }

          const actualVidWidth = W_base * newScale;
          const actualVidHeight = H_base * newScale;

          const currentMidpoint = getMidpoint(e.touches[0], e.touches[1]);
          const dxMid = currentMidpoint.x - initialTouchMidpointRef.current.x;
          const dyMid = currentMidpoint.y - initialTouchMidpointRef.current.y;
          if (Math.sqrt(dxMid * dxMid + dyMid * dyMid) > 15) {
            isTwoFingerTapPendingRef.current = false;
          }

          if (newScale === 1) {
            setZoomScale(1);
            setZoomPan({ x: 0, y: 0 });
          } else {
            // Apply mathematically correct combined Zoom & Pan formula in opposite direction:
            const midStartX = initialTouchMidpointRef.current.x - wrapperRect.left;
            const midStartY = initialTouchMidpointRef.current.y - wrapperRect.top;
            const midCurrentX = currentMidpoint.x - wrapperRect.left;
            const midCurrentY = currentMidpoint.y - wrapperRect.top;

            const dxMid = midCurrentX - midStartX;
            const dyMid = midCurrentY - midStartY;

            const scaleRatio = newScale / initialZoomScaleRef.current;
            const newPanX = (midStartX - dxMid) - (midStartX - initialZoomPanRef.current.x) * scaleRatio;
            const newPanY = (midStartY - dyMid) - (midStartY - initialZoomPanRef.current.y) * scaleRatio;

            // Boundary clamping so the zoomed view doesn't pan off-screen (unified Math.min/max range)
            const limitX1 = -offsetX * newScale;
            const limitX2 = W_wrapper - actualVidWidth - offsetX * newScale;
            const minPanX = Math.min(limitX1, limitX2);
            const maxPanX = Math.max(limitX1, limitX2);

            const limitY1 = -offsetY * newScale;
            const limitY2 = H_wrapper - actualVidHeight - offsetY * newScale;
            const minPanY = Math.min(limitY1, limitY2);
            const maxPanY = Math.max(limitY1, limitY2);

            const clampedPanX = Math.max(minPanX, Math.min(maxPanX, newPanX));
            const clampedPanY = Math.max(minPanY, Math.min(maxPanY, newPanY));

            setZoomScale(newScale);
            setZoomPan({ x: clampedPanX, y: clampedPanY });

            // Calculate start visual coordinate of the cursor relative to wrapper
            const V_startX = (initialLocalCursorPosRef.current.x / vidWidth) * (W_base * initialZoomScaleRef.current) + offsetX * initialZoomScaleRef.current + initialZoomPanRef.current.x;
            const V_startY = (initialLocalCursorPosRef.current.y / vidHeight) * (H_base * initialZoomScaleRef.current) + offsetY * initialZoomScaleRef.current + initialZoomPanRef.current.y;

            // Derive the new remote coordinates to keep the visual cursor locked on screen
            let newX = ((V_startX - offsetX * newScale - clampedPanX) / (W_base * newScale)) * vidWidth;
            let newY = ((V_startY - offsetY * newScale - clampedPanY) / (H_base * newScale)) * vidHeight;

            newX = Math.max(0, Math.min(vidWidth, newX));
            newY = Math.max(0, Math.min(vidHeight, newY));

            localCursorPosRef.current = { x: newX, y: newY };

            // Send mouse absolute position
            const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
            if (mouseAbsoluteChannel && mouseAbsoluteChannel.readyState === "open") {
              const xNorm = newX / vidWidth;
              const yNorm = newY / vidHeight;
              const x16 = Math.round(xNorm * 4096.0);
              const y16 = Math.round(yNorm * 4096.0);

              const buffer = new ArrayBuffer(13);
              const view = new DataView(buffer);
              view.setUint8(0, 1); // Type 1: MousePosition
              view.setInt16(1, x16, false);
              view.setInt16(3, y16, false);
              view.setInt16(5, 4096, false); // referenceWidth
              view.setInt16(7, 4096, false); // referenceHeight
              const seq = (mouseSeqRef.current++) >>> 0;
              view.setUint32(9, seq, false);
              mouseAbsoluteChannel.send(buffer);
            }

            // Sync visual cursor DOM directly and instantly
            const cursorEl = localCursorRef.current;
            if (cursorEl) {
              const leftWrapper = (newX / vidWidth) * actualVidWidth + offsetX * newScale + clampedPanX;
              const topWrapper = (newY / vidHeight) * actualVidHeight + offsetY * newScale + clampedPanY;
              cursorEl.style.left = `${leftWrapper}px`;
              cursorEl.style.top = `${topWrapper}px`;
            }
          }
        }
      }
    }
  };

  const handleTouchEnd = (e: React.TouchEvent<HTMLDivElement>) => {
    if (status !== "Streaming") return;
    
    // Prevent browser from emulating mouseup and click events
    e.preventDefault();

    if (longPressTimerRef.current) {
      clearTimeout(longPressTimerRef.current);
      longPressTimerRef.current = null;
    }

    if (longPressTriggeredRef.current) {
      longPressTriggeredRef.current = false;
      return; // Already triggered right click during hold
    }

    if (touchMode === "trackpad") {
      if (isDraggingRef.current) {
        isDraggingRef.current = false;
        sendMouseClickAction(1, false); // left click up
        return;
      }

      // Handle two-finger tap for right-click in trackpad mode
      if (wasMultiTouchRef.current) {
        const duration = performance.now() - twoFingerTouchStartTimeRef.current;
        if (isTwoFingerTapPendingRef.current && duration < 350) {
          const mouseReliableChannel = channelsRef.current["mouse_reliable"];
          if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
            sendMouseClickAction(3, true); // right click down
            setTimeout(() => sendMouseClickAction(3, false), 30); // right click up
            if (navigator.vibrate) {
              navigator.vibrate(50);
            }
          }
        }
        isTwoFingerTapPendingRef.current = false;
        return;
      }
    } else {
      // Ignore clicks if the touch session was a multi-touch (zoom/pan) gesture in Direct Mode
      if (wasMultiTouchRef.current) {
        return;
      }
    }

    const touch = e.changedTouches[0];
    const duration = performance.now() - touchStartTimeRef.current;
    const dx = touch.clientX - touchStartInitialPosRef.current.x;
    const dy = touch.clientY - touchStartInitialPosRef.current.y;
    const movement = Math.sqrt(dx * dx + dy * dy);

    if (touchMode === "trackpad") {
      // Forgiving dynamic tap threshold to accommodate quick finger wobbles on touch screens
      const maxTapMovement = duration < 250 ? 40 : 25;
      const isTap = duration < 450 && movement < maxTapMovement;
      const mouseReliableChannel = channelsRef.current["mouse_reliable"];

      if (isTap) {
        lastTouchTapTimeRef.current = performance.now(); // Record tap completion time
        if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
          sendMouseClickAction(1, true);
          if (clickUpTimerRef.current) {
            clearTimeout(clickUpTimerRef.current);
          }
          clickUpTimerRef.current = setTimeout(() => {
            sendMouseClickAction(1, false);
            clickUpTimerRef.current = null;
          }, 35);
        }
      }
    } else {
      // Direct Touchscreen Mode:
      if (isDirectClickPendingRef.current) {
        // Tap: send complete click (down + up)
        sendTouchAbsolutePos(touch.clientX, touch.clientY);
        sendMouseClickAction(1, true);
        setTimeout(() => sendMouseClickAction(1, false), 35);
        isDirectClickPendingRef.current = false;
      } else {
        // Drag end: release left click
        sendTouchAbsolutePos(touch.clientX, touch.clientY);
        sendMouseClickAction(1, false);
      }
    }
  };

  // Mobile Virtual Keyboard Handlers
  const handleVirtualKeyboardInput = (e: React.ChangeEvent<HTMLInputElement>) => {
    const text = e.target.value;
    if (text.length > 0) {
      const keyboardChannel = channelsRef.current["keyboard"];
      if (keyboardChannel && keyboardChannel.readyState === "open") {
        const utf8Encoder = new TextEncoder();
        const encodedText = utf8Encoder.encode(text);
        const buffer = new ArrayBuffer(3 + encodedText.length);
        const view = new DataView(buffer);
        view.setUint8(0, 1); // Type 1: Text Event
        view.setUint16(1, encodedText.length, false);
        new Uint8Array(buffer, 3).set(encodedText);
        keyboardChannel.send(buffer);
      }
      e.target.value = "";
    }
  };

  const handleVirtualKeyboardKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    const key = e.key;
    if (key === "Backspace" || key === "Enter" || key === "Tab" || key === "Escape") {
      sendSpecialKeyRemote(key);
      e.preventDefault();
    }
  };

  const sendSpecialKeyRemote = (key: string) => {
    let vk = 0;
    if (key === "Backspace") vk = 8;
    else if (key === "Tab") vk = 9;
    else if (key === "Enter") vk = 13;
    else if (key === "Escape") vk = 27;
    else if (key === "Delete") vk = 46;

    if (vk > 0) {
      let modifiers = 0;
      if (modifierKeys.shift) modifiers |= 1;
      if (modifierKeys.ctrl) modifiers |= 2;
      if (modifierKeys.alt) modifiers |= 4;
      if (modifierKeys.meta) modifiers |= 8;

      sendRawKeyEvent(vk, true, modifiers);
      setTimeout(() => sendRawKeyEvent(vk, false, modifiers), 30);
    }
  };

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
    // via pointerrawupdate → absolute mouse position. Skip React handler.
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

    hostCursorMouseDownRef.current = isDown ? true : e.buttons !== 0;
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
      containerRef.current.requestFullscreen().then(() => {
        setIsFullscreen(true);
      }).catch(err => {
        console.error("Error entering fullscreen:", err);
      });
    } else {
      document.exitFullscreen().then(() => {
        setIsFullscreen(false);
      }).catch(err => {
        console.error("Error exiting fullscreen:", err);
      });
    }
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
  const streamCursorStyle = isStreaming && (hideLocalCursor || agentHostOs === "windows") ? "none" : "default";

  if (selectedAppId === null) {
    return (
      <div className="stream-container" style={{ display: 'flex', flexDirection: 'column', minHeight: '100vh' }}>
        <div className="glow-orb bg-glow-blue"></div>
        <div className="glow-orb bg-glow-purple"></div>

        {/* Header/Navbar */}
        <header className="navbar" style={{ position: 'static', background: 'transparent', borderBottom: '1px solid rgba(255, 255, 255, 0.05)' }}>
          <div className="nav-brand">
            <button onClick={onBack} className="btn-secondary stream-back-btn" title="Leave" style={{ marginRight: '0.5rem', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)' }}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
                <path d="M19 12H5M12 19l-7-7 7-7" />
              </svg>
            </button>
            <span className="brand-name" style={{ fontSize: '1.25rem' }}>{hostName}</span>
            <span className="badge-tech">APPS</span>
          </div>
          <div className="nav-user-panel">
            <button onClick={() => setShowSettingsModal(true)} className="btn-secondary" style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)', padding: '0.5rem 1rem', borderRadius: '8px', fontSize: '0.85rem' }}>
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
          <div className="stream-settings-overlay" style={{ zIndex: 200 }}>
            <div className="stream-settings-card">
              <h2>Stream Settings</h2>
              <p className="subtitle">Adjust quality settings for this session</p>
              
              <div className="settings-grid">
                <div className="settings-group">
                  <label htmlFor="resolution">Resolution</label>
                  <select 
                    id="resolution" 
                    value={draftResolution} 
                    onChange={(e) => setDraftResolution(e.target.value)}
                  >
                    <option value="1080p">1080p (1920x1080)</option>
                    <option value="720p">720p (1280x720)</option>
                    <option value="540p">540p (960x540)</option>
                  </select>
                </div>

                <div className="settings-group">
                  <label htmlFor="fps">Frame Rate</label>
                  <select 
                    id="fps" 
                    value={draftFps} 
                    onChange={(e) => setDraftFps(Number(e.target.value))}
                  >
                    <option value={240}>240 FPS</option>
                    <option value={144}>144 FPS</option>
                    <option value={120}>120 FPS</option>
                    <option value={90}>90 FPS</option>
                    <option value={60}>60 FPS</option>
                    <option value={30}>30 FPS</option>
                  </select>
                </div>

                <div className="settings-group">
                  <label htmlFor="codec">Video Codec</label>
                  <select 
                    id="codec" 
                    value={draftCodec} 
                    onChange={(e) => setDraftCodec(e.target.value)}
                  >
                    <option value="auto">
                      Auto (Best Available: {resolveAutoCodec().toUpperCase()})
                    </option>
                    <option value="h264" disabled={!supportedCodecs.h264}>
                      {getCodecLabel("H.264", browserCodecs.h264, hostH264Supported)}
                    </option>
                    <option value="h265" disabled={!supportedCodecs.h265}>
                      {getCodecLabel("H.265 (HEVC)", browserCodecs.h265, hostH265Supported)}
                    </option>
                    <option value="av1" disabled={!supportedCodecs.av1}>
                      {getCodecLabel("AV1", browserCodecs.av1, hostAv1Supported)}
                    </option>
                  </select>
                  {draftCodec === 'auto' && (
                    <p style={{ margin: '0.25rem 0 0', fontSize: '0.75rem', color: 'var(--text-muted, #888)' }}>
                      Browser: H264={browserCodecs.h264?'✓':'✗'} H265={browserCodecs.h265?'✓':'✗'} AV1={browserCodecs.av1?'✓':'✗'}
                    </p>
                  )}
                </div>

                <div className="settings-group full-width">
                  <label htmlFor="bitrate">Bitrate (Kbps)</label>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '1rem' }}>
                    <input 
                      type="range" 
                      id="bitrate" 
                      min={1000} 
                      max={150000} 
                      step={500}
                      value={draftBitrate} 
                      onChange={(e) => setDraftBitrate(Number(e.target.value))}
                      style={{ flex: 1 }}
                    />
                    <span style={{ minWidth: '70px', textAlign: 'right', fontWeight: 'bold', color: 'var(--accent-cyan)' }}>
                      {(draftBitrate / 1000).toFixed(1)} Mbps
                    </span>
                  </div>
                </div>
                <div className="settings-group full-width">
                  <label htmlFor="mouseQueueLimit">Mouse Queue Limit (Backpressure)</label>
                  <select 
                    id="mouseQueueLimit" 
                    value={draftMouseQueueLimit} 
                    onChange={(e) => setDraftMouseQueueLimit(Number(e.target.value))}
                  >
                    <option value={0}>0 B (Strict No Queue - High Lag Risk)</option>
                    <option value={64}>64 B (Ultra Low Buffer)</option>
                    <option value={256}>256 B (Recommended - Smooth & Responsive)</option>
                    <option value={1024}>1024 B (Moderate Buffer)</option>
                    <option value={4096}>4096 B (High Buffer)</option>
                    <option value={16384}>16384 B (Previous Default - High Latency Risk)</option>
                  </select>
                </div>
                <div className="settings-group full-width">
                  <label htmlFor="inputProtocol">Input Protocol (WebTransport reduces mouse/keyboard latency)</label>
                  <select 
                    id="inputProtocol" 
                    value={draftInputProtocol} 
                    onChange={(e) => setDraftInputProtocol(e.target.value)}
                  >
                    <option value="webrtc">WebRTC Data Channels (Standard)</option>
                    <option value="webtransport" disabled={typeof (window as any).WebTransport === 'undefined'}>
                      WebTransport QUIC Datagrams {typeof (window as any).WebTransport === 'undefined' ? "(Unsupported by browser)" : "(Experimental - Faster Mouse)"}
                    </option>
                  </select>
                </div>
                {!!(window as any).__TAURI__ && (
                  <div className="settings-checkbox-group">
                    <input 
                      type="checkbox" 
                      id="useNativeClient"
                      checked={typeof window.RTCPeerConnection === 'undefined' ? true : draftUseNativeClient} 
                      disabled={typeof window.RTCPeerConnection === 'undefined'}
                      onChange={(e) => setDraftUseNativeClient(e.target.checked)}
                      style={{ cursor: typeof window.RTCPeerConnection === 'undefined' ? 'not-allowed' : 'pointer' }}
                    />
                    <label htmlFor="useNativeClient" style={{ cursor: typeof window.RTCPeerConnection === 'undefined' ? 'not-allowed' : 'pointer' }}>
                      Use native client binary {typeof window.RTCPeerConnection === 'undefined' ? "(Forced: Webview WebRTC unsupported on Linux WebKitGTK)" : "(bypasses WebView-based WebRTC, recommended for Desktop)"}
                    </label>
                  </div>
                )}
                <div className="settings-checkbox-group">
                  <input 
                    type="checkbox" 
                    id="useCanvasRenderer"
                    checked={draftUseCanvasRenderer} 
                    disabled={typeof (window as any).MediaStreamTrackProcessor === 'undefined' || isIOSOrSafari}
                    onChange={(e) => setDraftUseCanvasRenderer(e.target.checked)}
                    style={{ cursor: (typeof (window as any).MediaStreamTrackProcessor === 'undefined' || isIOSOrSafari) ? 'not-allowed' : 'pointer' }}
                  />
                  <label htmlFor="useCanvasRenderer" style={{ cursor: (typeof (window as any).MediaStreamTrackProcessor === 'undefined' || isIOSOrSafari) ? 'not-allowed' : 'pointer' }}>
                    Use Canvas Renderer {typeof (window as any).MediaStreamTrackProcessor === 'undefined' ? "(Unsupported by browser)" : isIOSOrSafari ? "(Disabled on iOS/Safari due to WebKit limits)" : "(Highly recommended - zero latency & no stutter)"}
                  </label>
                </div>
                <div className="settings-checkbox-group">
                  <input
                    type="checkbox"
                    id="virtualDisplay"
                    checked={draftVirtualDisplay}
                    onChange={(e) => setDraftVirtualDisplay(e.target.checked)}
                  />
                  <label htmlFor="virtualDisplay">
                    Create Virtual Display (Linux: xrandr virtual output, Windows: IddSampleDriver required)
                  </label>
                </div>
                {availableDisplays.length > 0 && (
                  <div className="settings-group full-width">
                    <label htmlFor="display">Display</label>
                    <select
                      id="display"
                      value={draftDisplay}
                      onChange={(e) => setDraftDisplay(e.target.value)}
                    >
                      <option value="default">Default</option>
                      {availableDisplays.map(d => (
                        <option key={d.id} value={d.id}>
                          {d.name} ({d.width}x{d.height} @ {d.refresh_rate.toFixed(0)}Hz){d.is_primary ? ' ★' : ''}
                        </option>
                      ))}
                    </select>
                  </div>
                )}
              </div>

              <div className="settings-actions">
                <button
                  onClick={() => {
                    setDraftMouseQueueLimit(mouseQueueLimit);
                    setDraftUseNativeClient(useNativeClient);
                    setDraftInputProtocol(activeInputProtocol);
                    setDraftUseCanvasRenderer(useCanvasRenderer);
                    setDraftEncoder(activeEncoder);
                    setDraftDisplay(activeDisplay);
                    setDraftVirtualDisplay(activeVirtualDisplay);
                    setShowSettingsModal(false);
                  }}
                  className="btn-secondary"
                >
                  Cancel
                </button>
                <button 
                  onClick={() => {
                    setActiveResolution(draftResolution);
                    setActiveFps(draftFps);
                    setActiveBitrate(draftBitrate);
                    setActiveCodec(draftCodec);
                    setMouseQueueLimit(draftMouseQueueLimit);
                    setUseNativeClient(draftUseNativeClient);
                    setActiveInputProtocol(draftInputProtocol);
                    setUseCanvasRenderer(draftUseCanvasRenderer);
                    setActiveEncoder(draftEncoder);
                    setActiveDisplay(draftDisplay);
                    setActiveVirtualDisplay(draftVirtualDisplay);
                    
                    localStorage.setItem('lunaris_stream_res', draftResolution);
                    localStorage.setItem('lunaris_stream_fps', String(draftFps));
                    localStorage.setItem('lunaris_stream_bitrate', String(draftBitrate));
                    localStorage.setItem('lunaris_stream_codec', draftCodec);
                    localStorage.setItem('lunaris_mouse_queue_limit', String(draftMouseQueueLimit));
                    localStorage.setItem('lunaris_tauri_use_native', String(draftUseNativeClient));
                    localStorage.setItem('lunaris_input_protocol', draftInputProtocol);
                    localStorage.setItem('lunaris_canvas_renderer', String(draftUseCanvasRenderer));
                    localStorage.setItem('lunaris_stream_encoder', draftEncoder);
                    localStorage.setItem('lunaris_stream_display', draftDisplay);
                    localStorage.setItem('lunaris_stream_virtual_display', String(draftVirtualDisplay));
                    
                    setShowSettingsModal(false);
                    addLog(`Applied settings: res=${draftResolution}, fps=${draftFps}, bitrate=${draftBitrate}Kbps, codec=${draftCodec}, mouseQueueLimit=${draftMouseQueueLimit}B, useNative=${draftUseNativeClient}, inputProtocol=${draftInputProtocol}, useCanvasRenderer=${draftUseCanvasRenderer}, encoder=${draftEncoder}, display=${draftDisplay}, virtualDisplay=${draftVirtualDisplay}`);
                  }}
                  className="btn-primary"
                >
                  Save Settings
                </button>
              </div>
            </div>
          </div>
        )}

        {/* Content Area */}
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '2rem' }}>
          <div className="app-selection-card" style={{ maxWidth: '900px', width: '100%', zIndex: 10, background: 'var(--bg-glass)', border: '1px solid var(--border-color)', borderRadius: '24px', padding: '2.5rem', boxShadow: 'var(--shadow-card)', backdropFilter: 'blur(20px)' }}>
            
            <div style={{ textAlign: 'center', marginBottom: '2.5rem' }}>
              <h2 style={{ fontSize: '2.25rem', fontWeight: 800, background: 'linear-gradient(to right, #ffffff, #a5b4fc)', WebkitBackgroundClip: 'text', WebkitTextFillColor: 'transparent', letterSpacing: '-0.5px', marginBottom: '0.5rem' }}>
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
    <div className="stream-container" ref={containerRef}>
      {/* Settings Modal Overlay */}
      {showSettingsModal && (
        <div className="stream-settings-overlay" style={{ zIndex: 200 }}>
          <div className="stream-settings-card">
            <h2>Stream Settings</h2>
            <p className="subtitle">Adjust quality settings for this session</p>
            
            <div className="settings-grid">
              <div className="settings-group">
                <label htmlFor="resolution">Resolution</label>
                <select 
                  id="resolution" 
                  value={draftResolution} 
                  onChange={(e) => setDraftResolution(e.target.value)}
                >
                  <option value="1080p">1080p (1920x1080)</option>
                  <option value="720p">720p (1280x720)</option>
                  <option value="540p">540p (960x540)</option>
                </select>
              </div>

              <div className="settings-group">
                <label htmlFor="fps">Frame Rate</label>
                <select 
                  id="fps" 
                  value={draftFps} 
                  onChange={(e) => setDraftFps(Number(e.target.value))}
                >
                  <option value={240}>240 FPS</option>
                  <option value={144}>144 FPS</option>
                  <option value={120}>120 FPS</option>
                  <option value={90}>90 FPS</option>
                  <option value={60}>60 FPS</option>
                  <option value={30}>30 FPS</option>
                </select>
              </div>

              <div className="settings-group">
                <label htmlFor="codec">Video Codec</label>
                <select 
                  id="codec" 
                  value={draftCodec} 
                  onChange={(e) => setDraftCodec(e.target.value)}
                >
                  <option value="h264" disabled={!supportedCodecs.h264}>
                    {getCodecLabel("H.264", browserCodecs.h264, hostH264Supported)}
                  </option>
                  <option value="h265" disabled={!supportedCodecs.h265}>
                    {getCodecLabel("H.265 (HEVC)", browserCodecs.h265, hostH265Supported)}
                  </option>
                  <option value="av1" disabled={!supportedCodecs.av1}>
                    {getCodecLabel("AV1", browserCodecs.av1, hostAv1Supported)}
                  </option>
                </select>
              </div>

              <div className="settings-group">
                <label htmlFor="encoder">Encoder Backend</label>
                <select 
                  id="encoder" 
                  value={draftEncoder} 
                  onChange={(e) => setDraftEncoder(e.target.value)}
                >
                  <option value="auto">Auto (Recommended)</option>
                  <option value="native">Native GPU</option>
                  <option value="ffmpeg">FFmpeg GPU</option>
                  <option value="software">Software</option>
                  <option disabled>──────────</option>
                  {availableEncoders
                    .filter(enc => !['auto', 'native', 'ffmpeg', 'software'].includes(enc))
                    .map(enc => (
                      <option key={enc} value={enc}>{enc.replace(/_/g, ' ').toUpperCase()}</option>
                    ))}
                  {availableEncoders.length === 0 && (
                    <>
                      <option value="native_nvenc_d3d11">Native NVENC D3D11</option>
                      <option value="native_amf_d3d11">Native AMF D3D11</option>
                      <option value="ffmpeg_nvenc">FFmpeg NVENC</option>
                      <option value="ffmpeg_amf">FFmpeg AMF</option>
                      <option value="ffmpeg_qsv">FFmpeg QSV</option>
                    </>
                  )}
                </select>
                <small style={{ color: 'var(--text-muted)', marginTop: '0.35rem', display: 'block' }}>
                  Active: {activeEncoderStatus.encoder} ({activeEncoderStatus.hwType})
                </small>
              </div>

              {availableDisplays.length > 0 && (
                <div className="settings-group full-width">
                  <label htmlFor="display">Display</label>
                  <select 
                    id="display" 
                    value={draftDisplay} 
                    onChange={(e) => setDraftDisplay(e.target.value)}
                  >
                    <option value="default">Default</option>
                    {availableDisplays.map(d => (
                      <option key={d.id} value={d.id}>
                        {d.name} ({d.width}x{d.height} @ {d.refresh_rate.toFixed(0)}Hz){d.is_primary ? ' ★' : ''}
                      </option>
                    ))}
                  </select>
                </div>
              )}

              <div className="settings-group full-width">
                <label htmlFor="bitrate">Bitrate (Kbps)</label>
                <div style={{ display: 'flex', alignItems: 'center', gap: '1rem' }}>
                  <input 
                    type="range" 
                    id="bitrate" 
                    min={1000} 
                    max={150000} 
                    step={500}
                    value={draftBitrate} 
                    onChange={(e) => setDraftBitrate(Number(e.target.value))}
                    style={{ flex: 1 }}
                  />
                  <span style={{ minWidth: '70px', textAlign: 'right', fontWeight: 'bold', color: 'var(--accent-cyan)' }}>
                    {(draftBitrate / 1000).toFixed(1)} Mbps
                  </span>
                </div>
              </div>

              <div className="settings-group full-width">
                <label htmlFor="mouseQueueLimit">Mouse Queue Limit (Backpressure)</label>
                <select
                  id="mouseQueueLimit"
                  value={draftMouseQueueLimit}
                  onChange={(e) => setDraftMouseQueueLimit(Number(e.target.value))}
                >
                  <option value={0}>0 B (Strict No Queue - High Lag Risk)</option>
                  <option value={64}>64 B (Ultra Low Buffer)</option>
                  <option value={256}>256 B (Recommended - Smooth & Responsive)</option>
                  <option value={1024}>1024 B (Moderate Buffer)</option>
                  <option value={4096}>4096 B (High Buffer)</option>
                  <option value={16384}>16384 B (Previous Default - High Latency Risk)</option>
                </select>
              </div>
              <div className="settings-group full-width">
                <label htmlFor="inputProtocol">Input Protocol (WebTransport reduces mouse/keyboard latency)</label>
                <select 
                  id="inputProtocol" 
                  value={draftInputProtocol} 
                  onChange={(e) => setDraftInputProtocol(e.target.value)}
                >
                  <option value="webrtc">WebRTC Data Channels (Standard)</option>
                  <option value="webtransport" disabled={typeof (window as any).WebTransport === 'undefined'}>
                    WebTransport QUIC Datagrams {typeof (window as any).WebTransport === 'undefined' ? "(Unsupported by browser)" : "(Experimental - Faster Mouse)"}
                  </option>
                </select>
              </div>
              {!!(window as any).__TAURI__ && (
                <div className="settings-checkbox-group">
                  <input 
                    type="checkbox" 
                    id="useNativeClient"
                    checked={typeof window.RTCPeerConnection === 'undefined' ? true : draftUseNativeClient} 
                    disabled={typeof window.RTCPeerConnection === 'undefined'}
                    onChange={(e) => setDraftUseNativeClient(e.target.checked)}
                    style={{ cursor: typeof window.RTCPeerConnection === 'undefined' ? 'not-allowed' : 'pointer' }}
                  />
                  <label htmlFor="useNativeClient" style={{ cursor: typeof window.RTCPeerConnection === 'undefined' ? 'not-allowed' : 'pointer' }}>
                    Use native client binary {typeof window.RTCPeerConnection === 'undefined' ? "(Forced: Webview WebRTC unsupported on Linux WebKitGTK)" : "(bypasses WebView-based WebRTC, recommended for Desktop)"}
                  </label>
                </div>
              )}
              <div className="settings-checkbox-group">
                <input 
                  type="checkbox" 
                  id="useCanvasRenderer"
                  checked={draftUseCanvasRenderer} 
                  disabled={typeof (window as any).MediaStreamTrackProcessor === 'undefined' || isIOSOrSafari}
                  onChange={(e) => setDraftUseCanvasRenderer(e.target.checked)}
                  style={{ cursor: (typeof (window as any).MediaStreamTrackProcessor === 'undefined' || isIOSOrSafari) ? 'not-allowed' : 'pointer' }}
                />
                <label htmlFor="useCanvasRenderer" style={{ cursor: (typeof (window as any).MediaStreamTrackProcessor === 'undefined' || isIOSOrSafari) ? 'not-allowed' : 'pointer' }}>
                  Use Canvas Renderer {typeof (window as any).MediaStreamTrackProcessor === 'undefined' ? "(Unsupported by browser)" : isIOSOrSafari ? "(Disabled on iOS/Safari due to WebKit limits)" : "(Highly recommended - zero latency & no stutter)"}
                </label>
              </div>
              <div className="settings-checkbox-group">
                <input 
                  type="checkbox" 
                  id="virtualDisplay"
                  checked={draftVirtualDisplay} 
                  onChange={(e) => setDraftVirtualDisplay(e.target.checked)}
                />
                <label htmlFor="virtualDisplay">
                  Create Virtual Display (Linux: xrandr virtual output, Windows: IddSampleDriver required)
                </label>
              </div>
            </div>

            <div className="settings-actions">
              <button 
                onClick={() => {
                  setDraftResolution(activeResolution);
                  setDraftFps(activeFps);
                  setDraftBitrate(activeBitrate);
                  setDraftCodec(activeCodec);
                  setDraftMouseQueueLimit(mouseQueueLimit);
                  setDraftUseNativeClient(useNativeClient);
                  setDraftInputProtocol(activeInputProtocol);
                  setDraftUseCanvasRenderer(useCanvasRenderer);
                  setDraftEncoder(activeEncoder);
                  setDraftDisplay(activeDisplay);
                  setDraftVirtualDisplay(activeVirtualDisplay);
                  setShowSettingsModal(false);
                }} 
                className="btn-secondary"
              >
                Cancel
              </button>
              <button 
                onClick={() => {
                  // Save options to localStorage
                  localStorage.setItem('lunaris_stream_res', draftResolution);
                  localStorage.setItem('lunaris_stream_fps', String(draftFps));
                  localStorage.setItem('lunaris_stream_bitrate', String(draftBitrate));
                  localStorage.setItem('lunaris_stream_codec', draftCodec);
                  localStorage.setItem('lunaris_mouse_queue_limit', String(draftMouseQueueLimit));
                  localStorage.setItem('lunaris_tauri_use_native', String(draftUseNativeClient));
                  localStorage.setItem('lunaris_input_protocol', draftInputProtocol);
                  localStorage.setItem('lunaris_canvas_renderer', String(draftUseCanvasRenderer));
                  localStorage.setItem('lunaris_stream_encoder', draftEncoder);
                  localStorage.setItem('lunaris_stream_display', draftDisplay);
                  localStorage.setItem('lunaris_stream_virtual_display', String(draftVirtualDisplay));
                  
                  // Check if settings requiring reconnect changed
                  const reconnectNeeded = 
                    draftResolution !== activeResolution ||
                    draftCodec !== activeCodec ||
                    draftUseNativeClient !== useNativeClient ||
                    draftInputProtocol !== activeInputProtocol ||
                    draftUseCanvasRenderer !== useCanvasRenderer ||
                    draftEncoder !== activeEncoder ||
                    draftDisplay !== activeDisplay ||
                    draftVirtualDisplay !== activeVirtualDisplay;

                  setMouseQueueLimit(draftMouseQueueLimit);

                  // Always update FPS/bitrate (dynamic, no reconnect needed)
                  setActiveFps(draftFps);
                  setActiveBitrate(draftBitrate);

                  if (reconnectNeeded) {
                    // Update active values to trigger reconnect
                    setActiveResolution(draftResolution);
                    setActiveCodec(draftCodec);
                    setUseNativeClient(draftUseNativeClient);
                    setActiveInputProtocol(draftInputProtocol);
                    setUseCanvasRenderer(draftUseCanvasRenderer);
                    setActiveEncoder(draftEncoder);
                    setActiveDisplay(draftDisplay);
                    setActiveVirtualDisplay(draftVirtualDisplay);
                    
                    // Reset states for connection indicator
                    setStatus("Connecting...");
                  }
                  
                  setShowSettingsModal(false);
                }} 
                className="btn-primary"
              >
                Apply
              </button>
            </div>
          </div>
        </div>
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
        <button onClick={onBack} className="stream-action-btn" title="Leave Session" style={{ marginRight: '0.25rem' }}>
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
          <option value="1080p">1080p</option>
          <option value="720p">720p</option>
          <option value="540p">540p</option>
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
          <option value={2000}>2 Mbps</option>
          <option value={5000}>5 Mbps</option>
          <option value={10000}>10 Mbps</option>
          <option value={15000}>15 Mbps</option>
          <option value={20000}>20 Mbps</option>
          <option value={30000}>30 Mbps</option>
          <option value={40000}>40 Mbps</option>
          <option value={50000}>50 Mbps</option>
          <option value={75000}>75 Mbps</option>
          <option value={100000}>100 Mbps</option>
          <option value={150000}>150 Mbps</option>
          {![2000, 5000, 10000, 15000, 20000, 30000, 40000, 50000, 75000, 100000, 150000].includes(activeBitrate) && (
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
            WebTransport {typeof (window as any).WebTransport === 'undefined' ? "(Unsupported)" : ""}
          </option>
        </select>

        {/* Display Dropdown */}
        <select
          value={activeDisplay}
          onChange={(e) => updateSetting('display', e.target.value)}
          className="stream-select"
          title="Display"
        >
          <option value="default">Default Display</option>
          {availableDisplays.map(d => (
            <option key={d.id} value={d.id}>
              {d.name} ({d.width}x{d.height}){d.is_primary ? ' ★' : ''}
            </option>
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

        {/* Tauri Window Minimize Action */}
        {!!(window as any).__TAURI__ && (
          <button 
            onClick={handleMinimize}
            className="stream-action-btn"
            title="Minimize Window"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
          </button>
        )}

        {/* Mute/Unmute Action */}
        <button 
          onClick={() => {
            const newValue = !isMuted;
            setIsMuted(newValue);
            localStorage.setItem('lunaris_stream_muted', String(newValue));
          }}
          className={`stream-action-btn ${!isMuted ? 'active' : ''}`}
          title={isMuted ? "Unmute Audio" : "Mute Audio"}
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
        <button 
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
        className={`stream-viewport-wrapper ${isKeyboardActive ? 'keyboard-active' : ''}`}
      >
        {/* Hidden input for mobile virtual keyboard */}
        <input
          ref={keyboardInputRef}
          type="text"
          style={{
            position: 'absolute',
            top: '-100px',
            left: '-100px',
            width: '10px',
            height: '10px',
            opacity: 0,
            zIndex: -100,
            pointerEvents: 'none'
          }}
          value=""
          onChange={handleVirtualKeyboardInput}
          onKeyDown={handleVirtualKeyboardKeyDown}
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
                transformOrigin: '0 0'
              }}
            />
            <video
              ref={hiddenVideoRef}
              autoPlay
              playsInline
              muted={isMuted}
              onLoadedMetadata={handleVideoLoadedMetadata}
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
            style={{ 
              cursor: streamCursorStyle,
              transform: `translate(${zoomPan.x}px, ${zoomPan.y}px) scale(${zoomScale})`,
              transformOrigin: '0 0'
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
            contain: 'layout paint style',
          }}
        >
          <img
            ref={hostCursorImageRef}
            alt=""
            draggable={false}
            style={{ width: '32px', height: '32px', display: 'block', maxWidth: 'none' }}
          />
        </div>

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
              color: '#ccc',
              fontSize: '12px',
              fontFamily: 'system-ui, sans-serif',
              whiteSpace: 'nowrap',
            }}>Hold ESC to exit</span>
            <div style={{
              width: '80px',
              height: '4px',
              background: 'rgba(255,255,255,0.2)',
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
            contain: 'layout paint style',
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
            {/* Mobile Controls Drawer */}
            {showMobileMenu && (
              <div 
                className="mobile-controls-drawer"
                onTouchStart={(e) => e.stopPropagation()}
                onTouchMove={(e) => e.stopPropagation()}
                onTouchEnd={(e) => e.stopPropagation()}
                onMouseDown={(e) => e.stopPropagation()}
                onMouseUp={(e) => e.stopPropagation()}
                onClick={(e) => e.stopPropagation()}
              >
                <div className="drawer-header">
                  <h4>Touch & Keyboard Controls</h4>
                </div>

                <div className="drawer-section">
                  <span className="section-label">Mouse Mode</span>
                  <div className="mode-toggle-grid">
                    <button 
                      onClick={() => {
                        setTouchMode('direct');
                        localStorage.setItem('lunaris_mobile_touch_mode', 'direct');
                      }}
                      className={`btn-toggle-option ${touchMode === 'direct' ? 'active' : ''}`}
                    >
                      Touchscreen
                    </button>
                    <button 
                      onClick={() => {
                        setTouchMode('trackpad');
                        localStorage.setItem('lunaris_mobile_touch_mode', 'trackpad');
                      }}
                      className={`btn-toggle-option ${touchMode === 'trackpad' ? 'active' : ''}`}
                    >
                      Trackpad
                    </button>
                  </div>
                </div>

                {touchMode === 'direct' && (
                  <div className="drawer-section">
                    <span className="section-label">Finger/Cursor Offset</span>
                    <div className="mode-toggle-grid">
                      <button 
                        onClick={() => {
                          setUseTouchOffset(true);
                          localStorage.setItem('lunaris_mobile_touch_offset', 'true');
                        }}
                        className={`btn-toggle-option ${useTouchOffset ? 'active' : ''}`}
                      >
                        Enabled (-40px)
                      </button>
                      <button 
                        onClick={() => {
                          setUseTouchOffset(false);
                          localStorage.setItem('lunaris_mobile_touch_offset', 'false');
                        }}
                        className={`btn-toggle-option ${!useTouchOffset ? 'active' : ''}`}
                      >
                        Disabled
                      </button>
                    </div>
                  </div>
                )}

                <div className="drawer-section">
                  <span className="section-label">Actions</span>
                  <div className="action-buttons-grid">
                    <button 
                      onClick={() => {
                        if (keyboardInputRef.current) {
                          keyboardInputRef.current.focus();
                          setIsKeyboardActive(true);
                        }
                      }}
                      className={`btn-action-option ${isKeyboardActive ? 'active' : ''}`}
                    >
                      ⌨️ Keyboard
                    </button>
                    <button 
                      onClick={() => {
                        setZoomScale(1);
                        setZoomPan({ x: 0, y: 0 });
                      }}
                      className="btn-action-option"
                      disabled={zoomScale === 1}
                    >
                      🔄 Reset Zoom
                    </button>
                    <button
                      onClick={() => {
                        const newValue = !isMuted;
                        setIsMuted(newValue);
                        localStorage.setItem('lunaris_stream_muted', String(newValue));
                      }}
                      className={`btn-action-option ${isMuted ? '' : 'active'}`}
                    >
                      {isMuted ? '🔇 Unmute Audio' : '🔊 Mute Audio'}
                    </button>
                    <button
                      onClick={toggleFullscreen}
                      className={`btn-action-option ${isFullscreen ? 'active' : ''}`}
                    >
                      {isFullscreen ? '⊡ Exit Fullscreen' : '⛶ Fullscreen'}
                    </button>
                    <button 
                      onClick={handleStopActiveStream}
                      className="btn-action-option btn-danger-option"
                    >
                      🛑 Disconnect
                    </button>
                  </div>
                </div>

                <div className="drawer-section">
                  <span className="section-label">Stream Quality</span>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: '0.4rem' }}>
                    <select
                      value={activeCodec}
                      onChange={(e) => updateSetting('codec', e.target.value)}
                      className="stream-select"
                      style={{ width: '100%', fontSize: '0.8rem' }}
                    >
                      <option value="auto">🎬 Auto ({resolvedActiveCodec.toUpperCase()})</option>
                      <option value="h264" disabled={!supportedCodecs.h264}>H264</option>
                      <option value="h265" disabled={!supportedCodecs.h265}>H265</option>
                      <option value="av1" disabled={!supportedCodecs.av1}>AV1</option>
                    </select>
                    <select
                      value={activeResolution}
                      onChange={(e) => updateSetting('res', e.target.value)}
                      className="stream-select"
                      style={{ width: '100%', fontSize: '0.8rem' }}
                    >
                      <option value="1080p">📺 1080p</option>
                      <option value="720p">📺 720p</option>
                      <option value="540p">📺 540p</option>
                    </select>
                    <select
                      value={activeFps}
                      onChange={(e) => updateSetting('fps', e.target.value)}
                      className="stream-select"
                      style={{ width: '100%', fontSize: '0.8rem' }}
                    >
                      <option value={240}>🎯 240 FPS</option>
                      <option value={144}>🎯 144 FPS</option>
                      <option value={120}>🎯 120 FPS</option>
                      <option value={90}>🎯 90 FPS</option>
                      <option value={60}>🎯 60 FPS</option>
                      <option value={30}>🎯 30 FPS</option>
                    </select>
                  </div>
                </div>

                {isKeyboardActive && (
                  <div className="drawer-section modifier-keys-section">
                    <span className="section-label">Modifier Keys</span>
                    <div className="modifiers-grid">
                      <button 
                        onClick={() => {
                          setModifierKeys(prev => ({ ...prev, ctrl: !prev.ctrl }));
                          sendRawKeyEvent(17, !modifierKeys.ctrl, 0);
                        }}
                        className={`btn-modifier-key ${modifierKeys.ctrl ? 'active' : ''}`}
                      >
                        Ctrl
                      </button>
                      <button 
                        onClick={() => {
                          setModifierKeys(prev => ({ ...prev, alt: !prev.alt }));
                          sendRawKeyEvent(18, !modifierKeys.alt, modifierKeys.ctrl ? 2 : 0);
                        }}
                        className={`btn-modifier-key ${modifierKeys.alt ? 'active' : ''}`}
                      >
                        Alt
                      </button>
                      <button 
                        onClick={() => {
                          setModifierKeys(prev => ({ ...prev, shift: !prev.shift }));
                          sendRawKeyEvent(16, !modifierKeys.shift, (modifierKeys.ctrl ? 2 : 0) | (modifierKeys.alt ? 4 : 0));
                        }}
                        className={`btn-modifier-key ${modifierKeys.shift ? 'active' : ''}`}
                      >
                        Shift
                      </button>
                      <button 
                        onClick={() => {
                          setModifierKeys(prev => ({ ...prev, meta: !prev.meta }));
                          sendRawKeyEvent(91, !modifierKeys.meta, (modifierKeys.ctrl ? 2 : 0) | (modifierKeys.alt ? 4 : 0) | (modifierKeys.shift ? 1 : 0));
                        }}
                        className={`btn-modifier-key ${modifierKeys.meta ? 'active' : ''}`}
                      >
                        Win
                      </button>
                      <button 
                        onClick={() => sendSpecialKeyRemote("Escape")}
                        className="btn-modifier-key"
                      >
                        Esc
                      </button>
                      <button 
                        onClick={() => sendSpecialKeyRemote("Tab")}
                        className="btn-modifier-key"
                      >
                        Tab
                      </button>
                    </div>
                  </div>
                )}
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
                onClick={onBack}
                className="mobile-footer-btn"
                title="Disconnect"
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
                title="Display Settings"
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
                title="Keyboard"
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
                title={`Mouse Mode: ${touchMode === 'trackpad' ? 'Trackpad' : 'Touchscreen'}`}
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
                title={isMuted ? "Unmute Audio" : "Mute Audio"}
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

              {/* Fullscreen Button */}
              <button
                onClick={toggleFullscreen}
                className={`mobile-footer-btn ${isFullscreen ? 'active' : ''}`}
                title={isFullscreen ? 'Exit Fullscreen' : 'Fullscreen'}
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
              </button>

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
                title="Controls Menu"
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
            <div>Ping (RTT): <span className="stat-value">{stats.ping.toFixed(1)} ms</span></div>
            <div>Jitter: <span className="stat-value">{stats.jitter.toFixed(1)} ms</span></div>
            <div>Decode Latency: <span className="stat-value">{stats.decodeLatency.toFixed(1)} ms</span></div>
            <div>WebRTC FPS: <span className="stat-value">{stats.fps}</span></div>
            <div>Decoded FPS: <span className="stat-value">{stats.decodedFps}</span></div>
            <div>Render FPS: <span className="stat-value">{stats.renderFps}</span></div>
            <div>Bitrate: <span className="stat-value">{stats.bitrate} Kbps</span></div>
            <div>Codec: <span className="stat-value">{activeCodec === 'auto' ? `Auto (${resolvedActiveCodec.toUpperCase()})` : activeCodec.toUpperCase()}</span></div>
            <div>Encoder: <span className="stat-value">{activeEncoderStatus.encoder} ({activeEncoderStatus.hwType})</span></div>
            <div>GPU: <span className="stat-value">{activeEncoderStatus.gpuInfo || agentGpuInfo || 'Unknown'}</span></div>
            <div>Host OS: <span className="stat-value">{agentHostOs}</span></div>
            <div>Display: <span className="stat-value">{activeEncoderStatus.displayName || activeEncoderStatus.displayId || activeDisplay || 'Default'}</span></div>
            <div>Requested Encoder: <span className="stat-value">{activeEncoderStatus.requestedEncoder}</span></div>
            <div>Input Protocol: <span className="stat-value" style={{ color: isWebTransportConnected ? '#4ade80' : '#38bdf8', fontWeight: 'bold' }}>
              {isWebTransportConnected ? "WebTransport (QUIC)" : "WebRTC (SCTP)"}
            </span></div>
            <div>Network Path: <span className="stat-value" style={{ color: stats.connectionType === 'P2P (Direct)' ? '#4ade80' : '#fb923c', fontWeight: 'bold' }}>
              {stats.connectionType}
            </span></div>
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
                <button onClick={onBack} className="btn-primary">Return to Dashboard</button>
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
    </div>
  );
};
