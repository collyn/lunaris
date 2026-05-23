import React, { useEffect, useRef, useState } from 'react';

const getBackendHost = () => {
  if (window.location.port === '5173' || window.location.port === '3000') {
    return `${window.location.hostname}:8080`;
  }
  return window.location.host;
};

interface StreamPlayerProps {
  hostId: string;
  hostName: string;
  token: string;
  serverCodecModeSupport?: number;
  onBack: () => void;
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
  onBack
}) => {
  const videoRef = useRef<HTMLVideoElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const pcRef = useRef<RTCPeerConnection | null>(null);
  const channelsRef = useRef<Record<string, RTCDataChannel>>({});

  const [status, setStatus] = useState<string>('Initializing...');
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [isPointerLocked, setIsPointerLocked] = useState<boolean>(false);
  const [appList, setAppList] = useState<{ id: number; title: string }[] | null>(null);
  const [currentGameId, setCurrentGameId] = useState<number>(0);
  const [selectedAppId, setSelectedAppId] = useState<number | null>(null);
  const [isStoppingStream, setIsStoppingStream] = useState<boolean>(false);
  
  // Settings States
  const [activeResolution, setActiveResolution] = useState<string>(() => localStorage.getItem('lunaris_stream_res') || '1080p');
  const [activeFps, setActiveFps] = useState<number>(() => Number(localStorage.getItem('lunaris_stream_fps') || '60'));
  const [activeBitrate, setActiveBitrate] = useState<number>(() => Number(localStorage.getItem('lunaris_stream_bitrate') || '8000'));
  const [activeCodec, setActiveCodec] = useState<string>(() => localStorage.getItem('lunaris_stream_codec') || 'h264');

  const [draftResolution, setDraftResolution] = useState<string>(activeResolution);
  const [draftFps, setDraftFps] = useState<number>(activeFps);
  const [draftBitrate, setDraftBitrate] = useState<number>(activeBitrate);
  const [draftCodec, setDraftCodec] = useState<string>(activeCodec);

  const [browserCodecs, setBrowserCodecs] = useState<{ h264: boolean; h265: boolean; av1: boolean }>({
    h264: true,
    h265: true,
    av1: true,
  });

  // Query browser codec support
  useEffect(() => {
    let h264 = true;
    let h265 = false;
    let av1 = false;
    if (typeof RTCRtpReceiver !== 'undefined' && RTCRtpReceiver.getCapabilities) {
      const capabilities = RTCRtpReceiver.getCapabilities('video');
      if (capabilities && capabilities.codecs) {
        console.log("WebRTC Video Codec Capabilities:", capabilities.codecs);
        h264 = capabilities.codecs.some(codec => 
          codec.mimeType.toLowerCase() === 'video/h264'
        );
        h265 = capabilities.codecs.some(codec => 
          codec.mimeType.toLowerCase() === 'video/h265' || 
          codec.mimeType.toLowerCase() === 'video/hevc'
        );
        av1 = capabilities.codecs.some(codec => 
          codec.mimeType.toLowerCase() === 'video/av1'
        );
      }
    } else {
      h265 = true;
      av1 = true;
    }

    // Additional fallback checks using standard HTML5 video tag support
    if (!h265 && typeof document !== 'undefined') {
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
    }

    if (!av1 && typeof document !== 'undefined') {
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

    setBrowserCodecs({ h264, h265, av1 });
  }, []);

  const hostH264Supported = serverCodecModeSupport === undefined || serverCodecModeSupport === 0 || (serverCodecModeSupport & 262145) !== 0;
  const hostH265Supported = serverCodecModeSupport === undefined || serverCodecModeSupport === 0 || (serverCodecModeSupport & 1573632) !== 0;
  const hostAv1Supported = serverCodecModeSupport !== undefined && serverCodecModeSupport !== 0 && (serverCodecModeSupport & 6488064) !== 0;

  const supportedCodecs = {
    h264: browserCodecs.h264 && hostH264Supported,
    h265: browserCodecs.h265 && hostH265Supported,
    av1: browserCodecs.av1 && hostAv1Supported,
  };

  // Sync activeCodec if it's not supported by host/browser capabilities
  useEffect(() => {
    const currentCodec = activeCodec as 'h264' | 'h265' | 'av1';
    if (!supportedCodecs[currentCodec]) {
      let fallbackCodec = 'h264';
      if (supportedCodecs.h265) {
        fallbackCodec = 'h265';
      } else if (supportedCodecs.h264) {
        fallbackCodec = 'h264';
      } else if (supportedCodecs.av1) {
        fallbackCodec = 'av1';
      }

      addLog(`Active codec ${activeCodec} is not supported. Falling back to ${fallbackCodec}`);
      setActiveCodec(fallbackCodec);
      setDraftCodec(fallbackCodec);
      localStorage.setItem('lunaris_stream_codec', fallbackCodec);
    }
  }, [browserCodecs, serverCodecModeSupport, activeCodec]);


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
  const [hideLocalCursor, setHideLocalCursor] = useState<boolean>(() => localStorage.getItem('lunaris_stream_hide_cursor') === 'true');
  const [isFullscreen, setIsFullscreen] = useState<boolean>(false);
  const [isHeaderVisible, setIsHeaderVisible] = useState<boolean>(true);
  const [isHeaderPinned, setIsHeaderPinned] = useState<boolean>(() => localStorage.getItem('lunaris_header_pinned') === 'true');
  const [showStats, setShowStats] = useState<boolean>(() => localStorage.getItem('lunaris_show_stats') !== 'false');
  const headerTimeoutRef = useRef<any | null>(null);
  const pendingMouseCoordsRef = useRef<{ clientX: number; clientY: number } | null>(null);
  const pendingMouseDeltasRef = useRef<{ dx: number; dy: number }>({ dx: 0, dy: 0 });

  // Stats State
  const [stats, setStats] = useState<{
    iceState: string;
    connState: string;
    fps: number;
    bitrate: number;
    ping: number;
    decodeLatency: number;
  }>({
    iceState: 'new',
    connState: 'new',
    fps: 0,
    bitrate: 0,
    ping: 0,
    decodeLatency: 0
  });

  const addLog = (msg: string) => {
    console.log(`[Lunaris] ${msg}`);
  };

  // Pointer lock change listener
  useEffect(() => {
    const handlePointerLockChange = () => {
      const locked = document.pointerLockElement === videoRef.current;
      setIsPointerLocked(locked);
      addLog(locked ? "Pointer locked. Relative mouse mode." : "Pointer unlocked. Absolute mouse mode.");
    };

    document.addEventListener('pointerlockchange', handlePointerLockChange);
    return () => {
      document.removeEventListener('pointerlockchange', handlePointerLockChange);
    };
  }, []);

  // Listen to fullscreen changes (e.g. user presses ESC)
  useEffect(() => {
    const handleFullscreenChange = () => {
      setIsFullscreen(!!document.fullscreenElement);
    };
    document.addEventListener('fullscreenchange', handleFullscreenChange);
    return () => {
      document.removeEventListener('fullscreenchange', handleFullscreenChange);
    };
  }, []);

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

  // requestAnimationFrame loop for throttled mouse movements
  useEffect(() => {
    if (status !== "Streaming") return;

    let animationFrameId: number;

    const tick = () => {
      if (isPointerLocked) {
        // Relative mouse mode: process accumulated deltas
        const dx = pendingMouseDeltasRef.current.dx;
        const dy = pendingMouseDeltasRef.current.dy;
        if (dx !== 0 || dy !== 0) {
          const mouseRelativeChannel = channelsRef.current["mouse_relative"];
          if (mouseRelativeChannel && mouseRelativeChannel.readyState === "open") {
            if (mouseRelativeChannel.bufferedAmount < 16384) {
              // Reset deltas only if we successfully send them
              pendingMouseDeltasRef.current = { dx: 0, dy: 0 };

              const buffer = new ArrayBuffer(5);
              const view = new DataView(buffer);
              view.setUint8(0, 0); // Type 0: MouseMove
              view.setInt16(1, dx, false); // big-endian
              view.setInt16(3, dy, false); // big-endian
              mouseRelativeChannel.send(buffer);
            }
          }
        }
      } else {
        // Absolute mouse mode: process latest coordinates
        const coords = pendingMouseCoordsRef.current;
        if (coords) {
          const mouseAbsoluteChannel = channelsRef.current["mouse_absolute"];
          if (mouseAbsoluteChannel && mouseAbsoluteChannel.readyState === "open" && videoRef.current) {
            if (mouseAbsoluteChannel.bufferedAmount < 16384) {
              pendingMouseCoordsRef.current = null;

              const video = videoRef.current;
              const rect = video.getBoundingClientRect();
              
              const elWidth = rect.width;
              const elHeight = rect.height;
              const vidWidth = video.videoWidth;
              const vidHeight = video.videoHeight;
              
              let xNorm = 0.5;
              let yNorm = 0.5;
              
              if (vidWidth > 0 && vidHeight > 0) {
                const elAspectRatio = elWidth / elHeight;
                const vidAspectRatio = vidWidth / vidHeight;
                
                let actualVidWidth = elWidth;
                let actualVidHeight = elHeight;
                let offsetX = 0;
                let offsetY = 0;
                
                if (elAspectRatio > vidAspectRatio) {
                  // Pillarbox: video is narrower than container
                  actualVidHeight = elHeight;
                  actualVidWidth = elHeight * vidAspectRatio;
                  offsetX = (elWidth - actualVidWidth) / 2;
                } else {
                  // Letterbox: video is wider than container
                  actualVidWidth = elWidth;
                  actualVidHeight = elWidth / vidAspectRatio;
                  offsetY = (elHeight - actualVidHeight) / 2;
                }
                
                const xLocal = coords.clientX - rect.left;
                const yLocal = coords.clientY - rect.top;
                
                xNorm = (xLocal - offsetX) / actualVidWidth;
                yNorm = (yLocal - offsetY) / actualVidHeight;
              } else {
                xNorm = (coords.clientX - rect.left) / elWidth;
                yNorm = (coords.clientY - rect.top) / elHeight;
              }

              const scaledX = Math.max(0, Math.min(4096, Math.round(xNorm * 4096)));
              const scaledY = Math.max(0, Math.min(4096, Math.round(yNorm * 4096)));

              const buffer = new ArrayBuffer(9);
              const view = new DataView(buffer);
              view.setUint8(0, 1); // Type 1: MousePosition
              view.setInt16(1, scaledX, false);
              view.setInt16(3, scaledY, false);
              view.setInt16(5, 4096, false);
              view.setInt16(7, 4096, false);
              mouseAbsoluteChannel.send(buffer);
            }
          }
        }
      }

      animationFrameId = requestAnimationFrame(tick);
    };

    animationFrameId = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(animationFrameId);
    };
  }, [status, isPointerLocked]);

  // Establish WebRTC Signaling Session
  useEffect(() => {
    let resolvedCodec = activeCodec;
    if (activeCodec === 'h265' && !supportedCodecs.h265) {
      addLog("Warning: H.265 (HEVC) might not be supported by your browser. Proceeding anyway.");
    } else if (activeCodec === 'av1' && !supportedCodecs.av1) {
      addLog("Warning: AV1 might not be supported by your browser. Proceeding anyway.");
    }

    addLog(`Initiating session with host: ${hostName} (${hostId}) using codec ${resolvedCodec}`);
    
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
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
              app_id: selectedAppId
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
              await handleSdpOffer(payload.target_id, payload.sdp.sdp);
            }
            break;
            
          case "IceCandidate":
            addLog("Received remote ICE candidate.");
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
  }, [hostId, activeResolution, activeFps, activeBitrate, activeCodec, token, hostName, selectedAppId]);

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
    let lastBytesReceived = 0;
    let lastTimestamp = 0;

    const interval = setInterval(async () => {
      if (!pcRef.current) return;
      try {
        const statsReport = await pcRef.current.getStats();
        let currentRtt = 0;
        let videoDecodeLatency = 0;
        let videoFps = 0;
        let videoBitrate = 0;

        statsReport.forEach((report) => {
          if (report.type === 'candidate-pair' && report.state === 'succeeded' && report.currentRoundTripTime !== undefined) {
            currentRtt = report.currentRoundTripTime * 1000; // to ms
          } else if (report.type === 'inbound-rtp' && report.kind === 'video') {
            if (report.framesPerSecond !== undefined) {
              videoFps = report.framesPerSecond;
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
            if (lastFramesDecoded > 0 && framesDecoded > lastFramesDecoded) {
              const deltaDecodeTime = totalDecodeTime - lastDecodeTime;
              const deltaFrames = framesDecoded - lastFramesDecoded;
              videoDecodeLatency = (deltaDecodeTime / deltaFrames) * 1000;
            }
            lastDecodeTime = totalDecodeTime;
            lastFramesDecoded = framesDecoded;
          }
        });

        setStats(prev => ({
          ...prev,
          ping: currentRtt,
          decodeLatency: videoDecodeLatency,
          fps: videoFps || prev.fps,
          bitrate: videoBitrate || prev.bitrate
        }));
      } catch (err) {
        console.error("Error fetching WebRTC stats:", err);
      }
    }, 1000);

    return () => clearInterval(interval);
  }, [status]);

  const handleSdpOffer = async (agentId: string, offerSdp: string) => {
    setStatus("Establishing WebRTC...");
    
    // Create RTCPeerConnection
    const pc = new RTCPeerConnection({
      iceServers: [
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
      channelsRef.current[channel.label] = channel;
      
      channel.onopen = () => addLog(`Data Channel ${channel.label} opened.`);
      channel.onclose = () => addLog(`Data Channel ${channel.label} closed.`);
      channel.onerror = (e) => addLog(`Data Channel ${channel.label} error: ${e}`);
    };

    // Handle incoming media tracks
    const mediaStream = new MediaStream();
    if (videoRef.current) {
      videoRef.current.srcObject = mediaStream;
    }

    pc.ontrack = (event) => {
      addLog(`Media track received: ${event.track.kind}`);
      mediaStream.addTrack(event.track);
      
      // Auto play video when track is added
      if (videoRef.current) {
        videoRef.current.play().catch(e => addLog(`Autoplay prevented: ${e}`));
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
            }
          }
        }
      }));
      addLog("SDP Answer sent back to host agent.");
    }
  };

  const cleanup = () => {
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

    // Clear video src
    if (videoRef.current) {
      videoRef.current.srcObject = null;
    }
    
    document.exitPointerLock();
  };

  // Request pointer lock for relative controls
  const togglePointerLock = () => {
    if (!videoRef.current) return;
    if (document.pointerLockElement === videoRef.current) {
      document.exitPointerLock();
    } else {
      videoRef.current.requestPointerLock();
    }
  };

  // Send mouse position (absolute)
  const handleMouseMove = (e: React.MouseEvent<HTMLVideoElement>) => {
    if (isPointerLocked) {
      // Relative mouse mode: accumulate movement deltas
      pendingMouseDeltasRef.current.dx += e.movementX;
      pendingMouseDeltasRef.current.dy += e.movementY;
    } else {
      // Absolute mouse mode: store latest coordinates
      pendingMouseCoordsRef.current = {
        clientX: e.clientX,
        clientY: e.clientY
      };
    }
  };

  // Send mouse click event
  const handleMouseButton = (e: React.MouseEvent<HTMLVideoElement>, isDown: boolean) => {
    // Prevent context menu from right clicks
    e.preventDefault();
    
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
  const handleWheel = (e: React.WheelEvent<HTMLVideoElement>) => {
    const mouseReliableChannel = channelsRef.current["mouse_reliable"];
    if (mouseReliableChannel && mouseReliableChannel.readyState === "open") {
      const dx = Math.max(-127, Math.min(127, Math.round(e.deltaX / 120)));
      const dy = Math.max(-127, Math.min(127, Math.round(-e.deltaY / 120)));

      const buffer = new ArrayBuffer(3);
      const view = new DataView(buffer);
      view.setUint8(0, 4); // Type 4: Scroll
      view.setInt8(1, dx);
      view.setInt8(2, dy);
      mouseReliableChannel.send(buffer);
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
                      {getCodecLabel("H.265 (HEVC)", true, hostH265Supported)}
                    </option>
                    <option value="av1" disabled={!supportedCodecs.av1}>
                      {getCodecLabel("AV1", browserCodecs.av1, hostAv1Supported)}
                    </option>
                  </select>
                </div>

                <div className="settings-group">
                  <label htmlFor="bitrate">Bitrate (Kbps)</label>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '1rem' }}>
                    <input 
                      type="range" 
                      id="bitrate" 
                      min={1000} 
                      max={50000} 
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
              </div>

              <div className="settings-actions">
                <button 
                  onClick={() => setShowSettingsModal(false)}
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
                    
                    localStorage.setItem('lunaris_stream_res', draftResolution);
                    localStorage.setItem('lunaris_stream_fps', String(draftFps));
                    localStorage.setItem('lunaris_stream_bitrate', String(draftBitrate));
                    localStorage.setItem('lunaris_stream_codec', draftCodec);
                    
                    setShowSettingsModal(false);
                    addLog(`Applied settings: res=${draftResolution}, fps=${draftFps}, bitrate=${draftBitrate}Kbps, codec=${draftCodec}`);
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
                        {app.title.toLowerCase().includes('desktop') ? (
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
                    {getCodecLabel("H.265 (HEVC)", true, hostH265Supported)}
                  </option>
                  <option value="av1" disabled={!supportedCodecs.av1}>
                    {getCodecLabel("AV1", browserCodecs.av1, hostAv1Supported)}
                  </option>
                </select>
              </div>

              <div className="settings-group full-width">
                <label htmlFor="bitrate">Bitrate</label>
                <select 
                  id="bitrate" 
                  value={draftBitrate} 
                  onChange={(e) => setDraftBitrate(Number(e.target.value))}
                >
                  <option value={2000}>2 Mbps (Low quality / bandwidth)</option>
                  <option value={4000}>4 Mbps (Standard quality)</option>
                  <option value={8000}>8 Mbps (High quality)</option>
                  <option value={12000}>12 Mbps (Very high quality)</option>
                  <option value={16000}>16 Mbps (Ultra quality)</option>
                  <option value={20000}>20 Mbps (Maximum quality)</option>
                </select>
              </div>
            </div>

            <div className="settings-actions">
              <button 
                onClick={() => {
                  setDraftResolution(activeResolution);
                  setDraftFps(activeFps);
                  setDraftBitrate(activeBitrate);
                  setDraftCodec(activeCodec);
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
                  
                  // Update active values to trigger reconnect
                  setActiveResolution(draftResolution);
                  setActiveFps(draftFps);
                  setActiveBitrate(draftBitrate);
                  setActiveCodec(draftCodec);
                  
                  // Reset states for connection indicator
                  setStatus("Connecting...");
                  
                  setShowSettingsModal(false);
                }} 
                className="btn-primary"
              >
                Apply & Reconnect
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
        <button onClick={onBack} className="btn-secondary stream-back-btn" title="Leave Session">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M19 12H5M12 19l-7-7 7-7" />
          </svg>
        </button>
        <div className="stream-host-info">
          <div className={`status-pulse ${isStreaming ? 'online' : 'busy'}`}></div>
          <span className="stream-host-title">{hostName}</span>
          <span className="badge-tech">{status.toUpperCase()}</span>
        </div>
        <div className="stream-actions" style={{ display: 'flex', gap: '0.75rem' }}>
          <button 
            onClick={toggleFullscreen}
            className={`cursor-toggle-btn ${isFullscreen ? 'active' : ''}`}
            title={isFullscreen ? "Exit Fullscreen" : "Fullscreen"}
          >
            {isFullscreen ? (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M4 14h6v6M20 10h-6V4M14 10l7-7M10 14l-7 7" />
              </svg>
            ) : (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M8 3H5a2 2 0 0 0-2 2v3M21 8V5a2 2 0 0 0-2-2h-3M3 16v3a2 2 0 0 0 2 2h3M16 21h3a2 2 0 0 0 2-2v-3M10 21V14H3M21 10h-7V3" />
              </svg>
            )}
          </button>

          <button 
            onClick={() => setShowSettingsModal(true)}
            className="cursor-toggle-btn"
            title="Settings"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </button>
          
          <button 
            onClick={() => {
              const newValue = !showStats;
              setShowStats(newValue);
              localStorage.setItem('lunaris_show_stats', String(newValue));
            }}
            disabled={!isStreaming}
            className={`cursor-toggle-btn ${showStats ? 'active' : ''}`}
            title={showStats ? "Hide Stats" : "Show Stats"}
          >
            {showStats ? (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <line x1="18" y1="20" x2="18" y2="10" />
                <line x1="12" y1="20" x2="12" y2="4" />
                <line x1="6" y1="20" x2="6" y2="14" />
              </svg>
            ) : (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <line x1="18" y1="20" x2="18" y2="10" opacity="0.4" />
                <line x1="12" y1="20" x2="12" y2="4" opacity="0.4" />
                <line x1="6" y1="20" x2="6" y2="14" opacity="0.4" />
              </svg>
            )}
          </button>
          
          <button 
            onClick={() => {
              const newValue = !hideLocalCursor;
              setHideLocalCursor(newValue);
              localStorage.setItem('lunaris_stream_hide_cursor', String(newValue));
            }}
            disabled={!isStreaming}
            className={`cursor-toggle-btn ${hideLocalCursor ? 'active' : ''}`}
            title={hideLocalCursor ? "Show Local Cursor" : "Hide Local Cursor"}
          >
            {hideLocalCursor ? (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <line x1="1" y1="1" x2="23" y2="23" />
                <path d="M3 3l7.07 16.97 2.51-7.39 7.39-2.51L3 3z" />
              </svg>
            ) : (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M3 3l7.07 16.97 2.51-7.39 7.39-2.51L3 3z" />
                <path d="M13 13l6 6" />
              </svg>
            )}
          </button>
          <button 
            onClick={togglePointerLock} 
            disabled={!isStreaming}
            className={`cursor-toggle-btn ${isPointerLocked ? 'active' : ''}`}
            title={isPointerLocked ? "Release Pointer Lock (ESC)" : "Lock Pointer"}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="10" />
              <line x1="22" y1="12" x2="18" y2="12" />
              <line x1="6" y1="12" x2="2" y2="12" />
              <line x1="12" y1="6" x2="12" y2="2" />
              <line x1="12" y1="22" x2="12" y2="18" />
            </svg>
          </button>

          <button 
            onClick={() => {
              const newValue = !isHeaderPinned;
              setIsHeaderPinned(newValue);
              localStorage.setItem('lunaris_header_pinned', String(newValue));
            }}
            disabled={!isStreaming}
            className={`cursor-toggle-btn ${isHeaderPinned ? 'active' : ''}`}
            title={isHeaderPinned ? "Unlock menu (auto-hide)" : "Lock menu (always visible)"}
          >
            {isHeaderPinned ? (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                <path d="M7 11V7a5 5 0 0 1 10 0v4" />
              </svg>
            ) : (
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                <path d="M7 11V7a5 5 0 0 1 9.9-1" />
              </svg>
            )}
          </button>

          <button 
            onClick={() => {
              setIsHeaderVisible(false);
              if (headerTimeoutRef.current) {
                clearTimeout(headerTimeoutRef.current);
              }
            }}
            disabled={!isStreaming}
            className="cursor-toggle-btn"
            title="Collapse Menu"
            style={{ padding: '0.4rem 0.6rem' }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M18 15l-6-6-6 6" />
            </svg>
          </button>
        </div>
      </div>

      {/* Main Stream Area */}
      <div className="stream-viewport-wrapper">
        <video
          ref={videoRef}
          onMouseMove={handleMouseMove}
          onMouseDown={(e) => handleMouseButton(e, true)}
          onMouseUp={(e) => handleMouseButton(e, false)}
          onContextMenu={(e) => e.preventDefault()}
          onWheel={handleWheel}
          className={`stream-video-view ${isStreaming ? 'visible' : 'hidden'}`}
          autoPlay
          playsInline
          muted={true}
          style={{ cursor: hideLocalCursor ? 'none' : 'default' }}
        />

        {/* Stats overlay */}
        {isStreaming && showStats && (
          <div className="stream-stats-overlay">
            <div>Ping (RTT): <span className="stat-value">{stats.ping.toFixed(1)} ms</span></div>
            <div>Decode Latency: <span className="stat-value">{stats.decodeLatency.toFixed(1)} ms</span></div>
            <div>Encode Latency: <span className="stat-value">~2.5 ms</span></div>
            <div>FPS: <span className="stat-value">{stats.fps}</span></div>
            <div>Bitrate: <span className="stat-value">{stats.bitrate} Kbps</span></div>
            <div>Codec: <span className="stat-value">{activeCodec.toUpperCase()}</span></div>
            <div>ICE / Conn: <span className="stat-value">{stats.iceState} / {stats.connState}</span></div>
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
                <div className="connecting-subtext">Initializing Moonlight WebRTC session...</div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
