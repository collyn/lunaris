import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtMultimedia
import com.lunaris.client 1.0
import com.lunaris.client.gpu 1.0

ApplicationWindow {
    id: window
    width: 1280
    height: 720
    visible: false
    flags: Qt.Window
    title: "Lunaris"
    color: window.isStreamMode ? "#000000" : Theme.bg

    property bool exitRequested: false

    // Custom properties to keep track of state
    property bool isPointerLocked: false
    property bool showStats: false
    property bool hideLocalCursor: true
    // Outside pointer lock the host cursor is shown as the real OS cursor
    // (HostCursor below): the compositor moves it at input rate, while a
    // QML Image only follows at the scene graph's frame rate and trails.
    readonly property bool useHostHardwareCursor: window.isStreamMode
        && window.hideLocalCursor && !window.isPointerLocked
    property bool ignoreMenuHover: false
    property real pingMs: 0.0
    property real decodeLatencyMs: 0.0
    property real fps: 0.0
    property real bitrateKbps: 0.0
    property string requestedCodec: "Unknown"
    property string activeCodec: "Unknown"
    property string activeDecodeBackend: "Unknown"
    property string activePresentBackend: "Unknown"
    property bool gpuDecodeEnabled: false
    property string decodeFallbackReason: "Waiting for video track"
    property int packetLossEvents: 0
    property int packetsLost: 0
    property int queueLagEvents: 0
    property int maxQueueLag: 0
    property string connectionType: "P2P (Direct)"
    property string activeEncoderName: "Unknown"
    property string activeDisplayId: ""
    property string activeDisplayName: ""
    property string activeEncoderHw: "Unknown"
    property string agentGpuInfo: "Unknown"
    property string requestedEncoder: "auto"
    property string activeInputProtocol: "webrtc"
    property bool useCuda: true
    property string renderBackend: "auto_gpu"
    property bool nativeVideoActive: gpuVideoItem.cudaActive || d3d11VideoItem.d3d11Active || vaapiDmabufVideoItem.dmabufActive
    // The updater's state (bridge.updateStatus(), updater/src/service.rs);
    // the banner and the Updates panel of the window without a link show it.
    property var updateStatus: ({ state: "idle", current: "", latest: "", autoCheck: true })
    property int hostCursorX: streamWidth / 2
    property int hostCursorY: streamHeight / 2
    property bool hostCursorVisible: false
    property string hostCursorKind: "arrow"
    property bool hostCursorMouseDown: false
    property bool hostCursorInWindowMoveSize: false
    // A host that cannot say when its pointer is drawn into its picture (a
    // computer of another type) only says it may be; the client then looks
    // for it in the decoded picture (pointer_in_picture.rs). "Maybe" alone
    // hides nothing.
    property bool pointerMaybeInPicture: false
    property bool pointerFoundInPicture: false
    // The host's pointer is part of its picture for the whole session (the
    // host's `pointer_in_picture` is "always": a computer shown through a
    // connector that always draws its pointer into what it sends, such as a
    // Sunshine host). No pointer is shown over the picture then, neither the
    // host-shaped one nor this computer's: the only pointer is the one in
    // the picture. The web client decides the same way (web/src/pointerShown.ts).
    property bool pointerAlwaysInPicture: false
    onIsStreamModeChanged: {
        if (isStreamMode) {
            systemKeysKind = bridge.systemKeysSupport();
        } else {
            pointerAlwaysInPicture = false;
            systemKeysPaused = false;
            systemKeysHintShown = false;
        }
    }
    property string agentHostOs: "unknown"
    property int localCursorX: streamWidth / 2
    property int localCursorY: streamHeight / 2
    property real localCursorVisualX: videoContainer.width / 2
    property real localCursorVisualY: videoContainer.height / 2
    property bool localCursorVisible: false
    property bool localCursorInitialized: false
    property bool hasNativeCursorImage: false
    property string nativeCursorKind: "unknown"
    property string nativeCursorSource: ""
    property int nativeCursorWidth: 32
    property int nativeCursorHeight: 32
    property int nativeCursorHotspotX: 0
    property int nativeCursorHotspotY: 0

    // ---- The stats overlay's "Encode" and "Decode" rows ----

    // A GPU's name as the host describes it, short: "RTX 2060" of
    // "NVIDIA GeForce RTX 2060 (vendor=0x10de, device=0x1f08)". "" when the
    // host named none ("Unknown ...").
    function shortGpuName(info) {
        var s = String(info || "").trim();
        if (s === "" || /^unknown/i.test(s)) return "";
        // "(vendor=0x.., device=0x..)", "(amdgpu, vendor=.., device=..)"
        s = s.replace(/\s*\([^()]*vendor=[^()]*\)\s*$/i, "");
        s = s.replace(/\((R|TM)\)/gi, "").replace(/\s+/g, " ").trim();
        var model = s.replace(/^(NVIDIA|AMD|ATI|Intel)( Corporation)?\s+/i, "")
                     .replace(/^(GeForce|Radeon)\s+/i, "");
        // "AMD GPU": nothing but the maker is known, so the maker stays.
        return (model === "" || /^GPU$/i.test(model)) ? s : model;
    }

    // What encodes the picture: "h264_nvenc · RTX 2060"; of a session that
    // shows another computer through a connector (the agent then says
    // encoder "remote", type "connector": the picture is encoded there, by
    // what nobody tells) "Remote PC".
    function encodeStatText(encoder, hwType, gpuInfo) {
        var name = String(encoder || "");
        var type = String(hwType || "");
        if (type.toLowerCase() === "connector" || name.toLowerCase() === "remote")
            return I18n.t("Remote PC");
        if (name === "" || name === "Unknown") return "-";
        // A software encoder runs on the processor, whatever GPU the host has.
        if (type.toLowerCase() === "software") return name + " · CPU";
        var gpu = window.shortGpuName(gpuInfo);
        return gpu !== "" ? name + " · " + gpu : name;
    }

    // The ping: tenths of a millisecond while it is under ten, whole
    // milliseconds from there ("0.4 ms", "9.6 ms", "12 ms").
    function pingStatText(ms) {
        var value = Number(ms);
        return (value < 9.95 ? value.toFixed(1) : value.toFixed(0)) + " ms";
    }

    // What decodes it and how long a picture takes: "VAAPI · 5.4 ms",
    // "Software (dav1d) · 6.0 ms". FFmpeg's CUDA device is NVIDIA's decoder
    // (NVDEC). Before the first picture only the time.
    function decodeStatText(backend, ms) {
        var time = Number(ms).toFixed(1) + " ms";
        var name = String(backend || "");
        if (name === "" || name === "Unknown") return time;
        if (name === "CUDA") name = "NVDEC";
        return name + " · " + time;
    }

    function normalizeCursorKind(kind) {
        var allowed = {
            "arrow": true,
            "ibeam": true,
            "hand": true,
            "cross": true,
            "move": true,
            "resize_ns": true,
            "resize_ew": true,
            "resize_nesw": true,
            "resize_nwse": true,
            "unavailable": true,
            "unknown": true
        };
        return allowed[kind] ? kind : "arrow";
    }

    // Bibata Modern Classic, rendered at 64 px from assets/cursors/*.svg.
    function cursorAssetName(kind) {
        var normalized = normalizeCursorKind(kind);
        if (normalized === "unknown") normalized = "arrow";
        return normalized.replace("_", "-") + ".png";
    }

    function cursorSourceForKind(kind) {
        return "qrc:/cursors/" + cursorAssetName(kind);
    }

    function hasMatchingNativeCursor(kind) {
        return window.hasNativeCursorImage && window.nativeCursorKind === window.normalizeCursorKind(kind);
    }

    function cursorHotspotX(kind) {
        if (window.hasMatchingNativeCursor(kind)) return window.nativeCursorHotspotX;
        var normalized = normalizeCursorKind(kind);
        if (normalized === "arrow" || normalized === "unknown") return 7;
        if (normalized === "hand") return 14;
        return 16;
    }

    function cursorHotspotY(kind) {
        if (window.hasMatchingNativeCursor(kind)) return window.nativeCursorHotspotY;
        var normalized = normalizeCursorKind(kind);
        if (normalized === "arrow" || normalized === "unknown") return 2;
        if (normalized === "hand") return 2;
        return 16;
    }

    function qtCursorShape(kind) {
        var normalized = normalizeCursorKind(kind);
        if (normalized === "ibeam") return Qt.IBeamCursor;
        if (normalized === "hand") return Qt.PointingHandCursor;
        if (normalized === "cross") return Qt.CrossCursor;
        if (normalized === "move") return Qt.SizeAllCursor;
        if (normalized === "resize_ns") return Qt.SizeVerCursor;
        if (normalized === "resize_ew") return Qt.SizeHorCursor;
        if (normalized === "resize_nesw") return Qt.SizeBDiagCursor;
        if (normalized === "resize_nwse") return Qt.SizeFDiagCursor;
        if (normalized === "unavailable") return Qt.ForbiddenCursor;
        return Qt.ArrowCursor;
    }

    function updateLocalCursorPrediction(localX, localY) {
        if (!window.isStreamMode || videoContainer.width <= 0 || videoContainer.height <= 0) return;
        var clampedX = Math.max(0, Math.min(videoContainer.width, localX));
        var clampedY = Math.max(0, Math.min(videoContainer.height, localY));
        window.localCursorVisualX = clampedX;
        window.localCursorVisualY = clampedY;
        window.localCursorX = Math.round((clampedX / videoContainer.width) * window.streamWidth);
        window.localCursorY = Math.round((clampedY / videoContainer.height) * window.streamHeight);
        window.localCursorVisible = true;
        window.localCursorInitialized = true;
    }

    function syncPointerLockCursor() {
        if (!window.isStreamMode || videoContainer.width <= 0 || videoContainer.height <= 0) return;
        // Sync to the last known host cursor position so the prediction
        // cursor starts where the remote cursor actually is, not the
        // center of the window.
        var targetX = window.hostCursorVisible ? window.hostCursorX : Math.round(window.streamWidth / 2);
        var targetY = window.hostCursorVisible ? window.hostCursorY : Math.round(window.streamHeight / 2);
        var visualX = (targetX / Math.max(1, window.streamWidth)) * videoContainer.width;
        var visualY = (targetY / Math.max(1, window.streamHeight)) * videoContainer.height;
        window.localCursorVisualX = visualX;
        window.localCursorVisualY = visualY;
        window.localCursorX = targetX;
        window.localCursorY = targetY;
        window.localCursorVisible = true;
        window.localCursorInitialized = true;
        bridge.sendMouseMove(targetX, targetY, videoContainer.width, videoContainer.height, 0, 0, false);
    }

    function updateLocalCursorDelta(dx, dy) {
        if (!window.isStreamMode || videoContainer.width <= 0 || videoContainer.height <= 0) return;
        if (!window.localCursorInitialized) {
            window.localCursorX = window.hostCursorVisible ? window.hostCursorX : Math.round(window.streamWidth / 2);
            window.localCursorY = window.hostCursorVisible ? window.hostCursorY : Math.round(window.streamHeight / 2);
            window.localCursorVisualX = (window.localCursorX / Math.max(1, window.streamWidth)) * videoContainer.width;
            window.localCursorVisualY = (window.localCursorY / Math.max(1, window.streamHeight)) * videoContainer.height;
            window.localCursorInitialized = true;
        }

        var scaledDx = (dx / Math.max(1, videoContainer.width)) * window.streamWidth;
        var scaledDy = (dy / Math.max(1, videoContainer.height)) * window.streamHeight;
        window.localCursorVisualX = Math.max(0, Math.min(videoContainer.width, window.localCursorVisualX + dx));
        window.localCursorVisualY = Math.max(0, Math.min(videoContainer.height, window.localCursorVisualY + dy));
        window.localCursorX = Math.round(Math.max(0, Math.min(window.streamWidth, window.localCursorX + scaledDx)));
        window.localCursorY = Math.round(Math.max(0, Math.min(window.streamHeight, window.localCursorY + scaledDy)));
        window.localCursorVisible = true;
    }

    // The host draws its own pointer into the picture: for the whole
    // session (pointerAlwaysInPicture), or while a button is held because
    // the host says so (a Windows host while a window is moved) or the
    // client found it there during this drag.
    function shouldHidePredictedCursor() {
        return window.pointerAlwaysInPicture
            || (window.hostCursorMouseDown
                && ((window.agentHostOs === "windows" && window.hostCursorInWindowMoveSize)
                    || window.pointerFoundInPicture));
    }

    // Hold ESC to unlock cursor properties
    property bool isEscHeld: false
    property bool wasUnlockedByHold: false
    property bool ignoreNextMouseMove: false
    property int lastMouseX: 0
    property int lastMouseY: 0
    property int warpX: -1
    property int warpY: -1
    property bool showLockBanner: false

    // Mouse send throttle — avoids saturating the QML→Rust bridge at high
    // polling rates (1000 Hz mice) which causes main-thread congestion and
    // visible cursor lag.
    property real lastMouseSendTime: 0
    property int pendingMouseX: -1
    property int pendingMouseY: -1
    property int pendingMouseW: 0
    property int pendingMouseH: 0
    property bool mouseSendPending: false

    Timer {
        id: mouseSendTimer
        interval: 4  // ~250 Hz flush rate
        repeat: true
        running: window.isStreamMode
        onTriggered: {
            if (window.mouseSendPending) {
                window.mouseSendPending = false;
                bridge.sendMouseMove(window.pendingMouseX, window.pendingMouseY,
                                     window.pendingMouseW, window.pendingMouseH,
                                     0, 0, window.isPointerLocked);
            }
        }
    }

    function throttledSendMouseMove(localX, localY, w, h) {
        var now = Date.now();
        var elapsed = now - window.lastMouseSendTime;
        if (elapsed >= 4) {
            window.lastMouseSendTime = now;
            window.mouseSendPending = false;
            bridge.sendMouseMove(localX, localY, w, h, 0, 0, false);
        } else {
            window.pendingMouseX = localX;
            window.pendingMouseY = localY;
            window.pendingMouseW = w;
            window.pendingMouseH = h;
            window.mouseSendPending = true;
        }
    }

    onIsPointerLockedChanged: {
        if (isPointerLocked) {
            window.syncPointerLockCursor();
            bridge.setPointerLocked(true);
            rootContainer.forceActiveFocus();
            window.showLockBanner = true;
            bannerTimer.restart();
            // Locking the pointer is asking for the keyboard again.
            window.systemKeysPaused = false;
        } else {
            bridge.setPointerLocked(false);
            window.showLockBanner = false;
            bannerTimer.stop();
        }
    }

    // ---- The keyboard of the stream ----
    // The stream view gets the keys while the window is the active one and
    // the view has the focus. When that ends (another window is activated,
    // a dialog takes the focus, the stream ends), the keys still held on
    // the remote computer are let go there: their releases will not come
    // here (moonlight-qt input.cpp notifyFocusLost 273-285: "certain keys
    // used in shortcuts that cause focus loss (such as Alt+Tab) may get
    // stuck down").
    property bool windowActive: window.active
    property bool streamViewFocused: rootContainer.activeFocus
    readonly property bool streamHasKeyboard: window.isStreamMode && window.windowActive && window.streamViewFocused
    onStreamHasKeyboardChanged: if (!streamHasKeyboard) bridge.releaseAllKeys()
    onWindowActiveChanged: if (!windowActive) systemKeysPaused = false

    // System shortcuts (Alt+Tab, the Super key, ...) go to the remote
    // computer when ViewPrefs.systemKeys says so: the window system is asked
    // for them (bridge.setKeyboardGrab). "fullscreen" and "off" are
    // moonlight-qt's modes (streamingpreferences.h CaptureSysKeysMode);
    // "always" is how RustDesk works. Ctrl+Alt+Esc gives them back to this
    // computer until the picture is clicked again.
    //
    // How depends on the window system (systemKeysKind, from the bridge):
    // - "grab" (X11) and "grab-maybe" (XWayland): a keyboard grab, held only
    //   while the stream view has the keyboard, and in "always" only while
    //   the pointer is over the picture - RustDesk takes the keyboard
    //   between enterView and leaveView the same way
    //   (flutter/lib/desktop/pages/remote_page.dart 850-906).
    // - "compositor" (Wayland): the request stays for as long as the stream
    //   is shown; the compositor applies it while the window has the
    //   keyboard and not otherwise (the protocol's active/inactive). It is
    //   not tied to the window being active or to the pointer here: GNOME
    //   asks the user in a dialog that takes both away from the window, and
    //   taking the request back at that moment would close the dialog.
    // - "window" (Windows, macOS): no system shortcut can be taken; Qt's
    //   own grab while the pointer is locked, as before.
    property string systemKeysKind: "no"
    property bool systemKeysPaused: false
    property bool pointerOverStream: streamMouseArea.containsMouse
    property bool systemKeysGrabbed: false
    property bool systemKeysHintShown: false
    function wantsSystemKeys(kind, mode, shown, hasKeyboard, paused, pointerLocked, fullScreen, pointerOver) {
        if (kind === "window") return hasKeyboard && pointerLocked;
        if (kind === "no" || paused || mode === "off") return false;
        if (kind === "compositor") return shown && (mode === "always" || fullScreen);
        if (!hasKeyboard) return false;
        if (mode === "fullscreen") return fullScreen;
        return pointerLocked || fullScreen || pointerOver;
    }
    readonly property bool systemKeysWanted: wantsSystemKeys(systemKeysKind, ViewPrefs.systemKeys,
        isStreamMode && visible, streamHasKeyboard, systemKeysPaused,
        isPointerLocked, window.visibility === Window.FullScreen, pointerOverStream)
    onSystemKeysWantedChanged: {
        if (systemKeysWanted) {
            // Shortly after: a mouse press still being handled holds the
            // grab, and a window just shown has no surface yet.
            keyboardGrabTimer.restart();
        } else {
            keyboardGrabTimer.stop();
            applySystemKeys();
        }
    }
    function applySystemKeys() {
        if (systemKeysGrabbed === systemKeysWanted) return;
        systemKeysGrabbed = systemKeysWanted;
        bridge.setKeyboardGrab(systemKeysGrabbed);
        if (systemKeysGrabbed && !systemKeysHintShown && systemKeysKind !== "window") {
            systemKeysHintShown = true;
            window.showToast(I18n.t("System shortcuts go to the remote computer. Ctrl+Alt+Esc gives them back to this computer."));
        }
    }
    function pauseSystemKeys() {
        if (!window.isStreamMode) return;
        var had = systemKeysGrabbed;
        systemKeysPaused = true;
        if (had && systemKeysKind !== "window") window.showToast(I18n.t("System shortcuts are with this computer again. Click the picture to send them to the remote computer."));
    }
    // Presses a key combination on the remote computer: the keys go down in
    // the order given and up in the opposite one (Qt key codes).
    function sendKeyCombo(keys) {
        var i;
        for (i = 0; i < keys.length; i++) bridge.sendKeyEvent(keys[i], 0, true);
        for (i = keys.length - 1; i >= 0; i--) bridge.sendKeyEvent(keys[i], 0, false);
    }
    function showToast(text) {
        shareToast.text = text;
        shareToast.opacity = 1;
        shareToastTimer.restart();
    }

    Item {
        id: rootContainer
        anchors.fill: parent
        focus: true

        // Capture keyboard events globally within root Item
        Keys.onPressed: (event) => {
            if (!window.isStreamMode) {
                event.accepted = false;
                return;
            }
            // Escape pointer lock by holding ESC
            if (event.key === Qt.Key_Escape && window.isPointerLocked) {
                if (!event.isAutoRepeat) {
                    window.isEscHeld = true;
                    escHoldTimer.start();
                }
                event.accepted = true;
                return;
            }

            bridge.sendKeyEventAt(event.key, event.modifiers, event.nativeScanCode, true, event.isAutoRepeat);
            event.accepted = true;
        }

        Keys.onReleased: (event) => {
            if (!window.isStreamMode) {
                event.accepted = false;
                return;
            }
            if (event.key === Qt.Key_Escape) {
                if (window.wasUnlockedByHold) {
                    window.wasUnlockedByHold = false;
                    event.accepted = true;
                    return;
                }
                if (window.isPointerLocked && window.isEscHeld) {
                    if (!event.isAutoRepeat) {
                        window.isEscHeld = false;
                        escHoldTimer.stop();
                        // Send normal short press ESC to host
                        bridge.sendKeyEvent(Qt.Key_Escape, event.modifiers, true);
                        bridge.sendKeyEvent(Qt.Key_Escape, event.modifiers, false);
                    }
                    event.accepted = true;
                    return;
                }
            }

            bridge.sendKeyEventAt(event.key, event.modifiers, event.nativeScanCode, false, event.isAutoRepeat);
            event.accepted = true;
        }

        // Instantiate our Rust cxx-qt bridge QObject
        StreamBridge {
        id: bridge

        // Handle diagnostic stats sent from Rust
        onStatsUpdated: (ping, decode, frames, bit, requestedCodec, negotiatedCodec, decodeBackend, presentBackend, gpuDecodeEnabled, fallbackReason, packetLossEvents, packetsLost, queueLagEvents, maxQueueLag, connType) => {
            window.pingMs = ping
            window.decodeLatencyMs = decode
            window.fps = frames
            window.bitrateKbps = bit
            window.requestedCodec = requestedCodec || "Unknown"
            window.activeCodec = negotiatedCodec || "Unknown"
            window.activeDecodeBackend = decodeBackend || "Unknown"
            window.activePresentBackend = presentBackend || "Unknown"
            window.gpuDecodeEnabled = gpuDecodeEnabled
            window.decodeFallbackReason = fallbackReason || "None"
            window.packetLossEvents = packetLossEvents
            window.packetsLost = packetsLost
            window.queueLagEvents = queueLagEvents
            window.maxQueueLag = maxQueueLag
            window.connectionType = connType
        }

        onHostCursorUpdated: (x, y, visible, kind, inWindowMoveSize) => {
            window.hostCursorX = x
            window.hostCursorY = y
            window.hostCursorVisible = visible
            window.hostCursorKind = window.normalizeCursorKind(kind)
            window.hostCursorInWindowMoveSize = inWindowMoveSize
            if (!visible) {
                window.localCursorVisible = false
            } else if (!window.localCursorInitialized) {
                window.localCursorX = x
                window.localCursorY = y
                window.localCursorVisualX = (x / Math.max(1, window.streamWidth)) * videoContainer.width
                window.localCursorVisualY = (y / Math.max(1, window.streamHeight)) * videoContainer.height
                window.localCursorVisible = true
                window.localCursorInitialized = true
            }
        }

        onPointerAlwaysInPictureUpdated: (always) => {
            window.pointerAlwaysInPicture = always
        }

        onPointerInPictureUpdated: (maybe, found) => {
            window.pointerMaybeInPicture = maybe
            window.pointerFoundInPicture = found
        }

        onHostCursorImageUpdated: (kind, source, width, height, hotspotX, hotspotY) => {
            window.nativeCursorKind = window.normalizeCursorKind(kind)
            window.nativeCursorSource = source
            window.nativeCursorWidth = Math.max(1, width)
            window.nativeCursorHeight = Math.max(1, height)
            window.nativeCursorHotspotX = hotspotX
            window.nativeCursorHotspotY = hotspotY
            window.hasNativeCursorImage = source !== ""
        }

        onHostOsUpdated: (hostOs) => {
            window.agentHostOs = String(hostOs).toLowerCase()
        }

        onHostInfoUpdated: (gpuInfo, hostOs) => {
            if (gpuInfo !== "") window.agentGpuInfo = gpuInfo
            if (hostOs !== "") window.agentHostOs = String(hostOs).toLowerCase()
        }

        onEncoderStatusUpdated: (encoder, hwType, gpuInfo, requestedEncoder, hostOs, displayId, displayName) => {
            window.activeEncoderName = encoder || "Unknown"
            window.activeEncoderHw = hwType || "Unknown"
            window.agentGpuInfo = gpuInfo || window.agentGpuInfo || "Unknown"
            window.requestedEncoder = requestedEncoder || "auto"
            if (hostOs !== "") window.agentHostOs = String(hostOs).toLowerCase()
            window.activeDisplayId = displayId || ""
            window.activeDisplayName = displayName || ""
        }

        onLocalCursorDelta: (rx, ry) => {
            window.updateLocalCursorDelta(rx, ry)
        }

        onSettingsLoaded: (res, fps, codec, bitrate, queueLimit, hostName, disableCuda, inputProtocol, renderBackend) => {
            menuBar.initializeSettings(res, fps, codec, bitrate, queueLimit, hostName, disableCuda, inputProtocol, renderBackend);
            window.renderBackend = String(renderBackend || (disableCuda ? "software" : "auto_gpu")).toLowerCase();
            window.useCuda = window.renderBackend !== "software";
            window.activeInputProtocol = String(inputProtocol).toLowerCase();
            var parts = res.split("x");
            if (parts.length === 2) {
                window.streamWidth = parseInt(parts[0]);
                window.streamHeight = parseInt(parts[1]);
            }
        }

        onDeeplinkReceived: (url) => {
            console.log("Deep link activation received. URL: " + url)
            window.show();
            window.raise();
            window.requestActivate();

            // A link to a server the user is not signed in to asks first.
            if (linkDialog.openIfPending()) return;

            if (url !== "") {
                launcherView.showMessage("");
                window.isStreamMode = true;
                bridge.setVideoSink(videoOutput.videoSink);
                bridge.startStream();
                bridge.requestSettings();
                rootContainer.forceActiveFocus();
            }
        }

        // A link's one-time ticket was refused (or the server not reached).
        onLinkFailed: (message) => {
            console.log("Link could not be used: " + message);
            window.show();
            window.raise();
            if (window.isStreamMode) window.showToast(I18n.t(message));
            else launcherView.showMessage(I18n.t(message));
        }

        onShareStatus: (message) => {
            shareToast.text = I18n.t(message);
            shareToast.opacity = 1;
            shareToastTimer.restart();
        }

        onStreamFailed: (message) => {
            console.log("Stream failed: " + message);
            bridge.stopStream();
            if (bridge.hasConnectionArgs()) {
                // Launched for one stream (deep link): nothing to go back to.
                window.exitRequested = true;
                Qt.quit();
                return;
            }
            window.isStreamMode = false;
            launcherView.showMessage(I18n.tError(message));
        }

        // The source being shown could not be connected, or closed: the
        // stream is over, with the host's reason when it gave one.
        onSourceStatus: (status, reason) => {
            console.log("Source " + status + ": " + reason);
            var failed = status === "failed";
            var message = reason !== ""
                ? I18n.t(failed ? "Could not connect to the other computer: %1" : "The other computer closed the session: %1").arg(I18n.tError(reason))
                : I18n.t(failed ? "Could not connect to the other computer." : "The other computer closed the session.");
            bridge.stopStream();
            if (bridge.hasConnectionArgs()) {
                // Launched for one stream (deep link): nothing to go back to.
                window.exitRequested = true;
                Qt.quit();
                return;
            }
            window.isStreamMode = false;
            launcherView.showMessage(message);
        }

    }

    // Periodic timer to poll statistics from Rust backend
    Timer {
        interval: 1000
        running: window.isStreamMode
        repeat: true
        onTriggered: {
            bridge.pollStats();
        }
    }

    Timer {
        interval: 16
        running: window.isStreamMode
        repeat: true
        onTriggered: {
            bridge.pollCursor();
        }
    }

    // Periodic timer to poll events (links arriving, links that failed) from the Rust backend
    Timer {
        interval: 100
        running: true
        repeat: true
        onTriggered: {
            bridge.pollEvents();
        }
    }

    // Timer to track holding ESC key to unlock cursor
    Timer {
        id: escHoldTimer
        interval: 1200
        running: false
        repeat: false
        onTriggered: {
            window.pauseSystemKeys();
            window.isPointerLocked = false;
            window.wasUnlockedByHold = true;
            window.isEscHeld = false;
            menuBar.open();
        }
    }

    // Timer to auto-hide pointer lock banner notification after 3 seconds
    Timer {
        id: bannerTimer
        interval: 3000
        running: false
        repeat: false
        onTriggered: {
            window.showLockBanner = false;
        }
    }

    // Timer to cooldown hover trigger on collapse click
    Timer {
        id: hoverCooldownTimer
        interval: 1500
        running: false
        repeat: false
        onTriggered: {
            window.ignoreMenuHover = false;
        }
    }

    // Timer to defer keyboard grab to bypass active mouse press grab state
    Timer {
        id: keyboardGrabTimer
        interval: 150
        running: false
        repeat: false
        onTriggered: window.applySystemKeys()
    }

    // Streaming Player View Container
    Item {
        id: streamView
        anchors.fill: parent
        clip: true
        visible: window.isStreamMode
        focus: window.isStreamMode

        // The picture's box, sized by the display mode (Display settings):
        // fit keeps the whole picture, fill covers the window (edges cropped),
        // stretch ignores the aspect ratio, native is one stream pixel per
        // device pixel. Input is mapped through this box, so it stays right.
        Item {
            id: videoContainer
            anchors.centerIn: parent
            readonly property real aspect: window.streamWidth / Math.max(1, window.streamHeight)
            readonly property real parentAspect: parent.width / Math.max(1, parent.height)
            width: {
                switch (ViewPrefs.displayMode) {
                case "stretch": return parent.width;
                case "native": return window.streamWidth / Screen.devicePixelRatio;
                case "fill": return parentAspect > aspect ? parent.width : parent.height * aspect;
                default: return parentAspect > aspect ? parent.height * aspect : parent.width;
                }
            }
            height: {
                switch (ViewPrefs.displayMode) {
                case "stretch": return parent.height;
                case "native": return window.streamHeight / Screen.devicePixelRatio;
                case "fill": return parentAspect > aspect ? parent.width / aspect : parent.height;
                default: return parentAspect > aspect ? parent.height : parent.width / aspect;
                }
            }

            // Video Output Area
            VideoOutput {
                id: videoOutput
                anchors.fill: parent
                fillMode: VideoOutput.Stretch
                // Always shown, under the native video items: they cover it
                // while they present. Hiding it and showing it again (when a
                // native path fails mid-stream and frames fall back to the
                // video sink) left it black.
                visible: true
            }

            GpuVideoItem {
                id: gpuVideoItem
                anchors.fill: parent
                visible: gpuVideoItem.cudaSupported && window.useCuda && gpuVideoItem.cudaActive
            }

            D3D11VideoItem {
                id: d3d11VideoItem
                anchors.fill: parent
                visible: d3d11VideoItem.d3d11Supported && window.useCuda && d3d11VideoItem.d3d11Active
            }

            VaapiDmabufVideoItem {
                id: vaapiDmabufVideoItem
                anchors.fill: parent
                visible: vaapiDmabufVideoItem.dmabufSupported && window.useCuda && vaapiDmabufVideoItem.dmabufActive
            }

        }

        // Full-window mouse input overlay — must cover the ENTIRE window,
        // not just videoContainer, so clicks in letterbox areas (e.g., taskbar)
        // are captured and correctly mapped to the remote desktop.
        MouseArea {
            id: streamMouseArea
            objectName: "streamMouseArea"
            anchors.fill: parent
            hoverEnabled: true
            acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
            // While HostCursor drives this item's cursor, keep the binding
            // constant so it never overwrites the pixmap cursor.
            // (HostCursor sets a blank cursor while `hidden`; with "show
            // local cursor" the binding itself is blank while the host's
            // pointer is in the picture.)
            cursorShape: window.useHostHardwareCursor ? Qt.ArrowCursor
                : (window.isPointerLocked || window.hideLocalCursor || window.shouldHidePredictedCursor()) ? Qt.BlankCursor : window.qtCursorShape(window.hostCursorKind)

            onPositionChanged: (mouse) => {
                if (mouse.y >= 50 && window.ignoreMenuHover) {
                    window.ignoreMenuHover = false;
                }
                
                if (!window.isPointerLocked) {
                    // Map window coordinates to videoContainer-local coordinates
                    var mapped = streamView.mapToItem(videoContainer, mouse.x, mouse.y);
                    window.updateLocalCursorPrediction(mapped.x, mapped.y);
                    window.throttledSendMouseMove(mapped.x, mapped.y, videoContainer.width, videoContainer.height);
                }
            }

            onPressed: (mouse) => {
                rootContainer.forceActiveFocus();
                // A click in the picture sends the system shortcuts again.
                window.systemKeysPaused = false;
                window.hostCursorMouseDown = true;
                if (!window.isPointerLocked) {
                    var mapped = streamView.mapToItem(videoContainer, mouse.x, mouse.y);
                    window.updateLocalCursorPrediction(mapped.x, mapped.y);
                    bridge.sendMouseClick(getButtonCode(mouse.button), true);
                }
            }

            onReleased: (mouse) => {
                window.hostCursorMouseDown = mouse.buttons !== 0;
                // The pointer was found for the drag that ends here.
                if (!(mouse.buttons & Qt.LeftButton)) window.pointerFoundInPicture = false;
                if (!window.isPointerLocked) {
                    var mapped = streamView.mapToItem(videoContainer, mouse.x, mouse.y);
                    window.updateLocalCursorPrediction(mapped.x, mapped.y);
                    bridge.sendMouseClick(getButtonCode(mouse.button), false);
                }
            }

            onWheel: (wheel) => {
                if (!window.isPointerLocked) {
                    bridge.sendMouseWheel(wheel.angleDelta.x, wheel.angleDelta.y);
                }
            }

            function getButtonCode(btn) {
                if (btn === Qt.LeftButton) return 1;
                if (btn === Qt.MiddleButton) return 2;
                if (btn === Qt.RightButton) return 3;
                return 0;
            }
        }

        HostCursor {
            objectName: "hostCursor"
            target: streamMouseArea
            active: window.useHostHardwareCursor
            hidden: !window.localCursorVisible || window.shouldHidePredictedCursor()
            source: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorSource : window.cursorSourceForKind(window.hostCursorKind)
            cursorWidth: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorWidth : 32
            cursorHeight: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorHeight : 32
            hotspotX: window.cursorHotspotX(window.hostCursorKind)
            hotspotY: window.cursorHotspotY(window.hostCursorKind)
        }

        Image {
            id: hostCursorOverlay
            objectName: "hostCursorOverlay"
            source: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorSource : window.cursorSourceForKind(window.hostCursorKind)
            width: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorWidth : 32
            height: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorHeight : 32
            fillMode: Image.PreserveAspectFit
            smooth: true
            mipmap: false
            cache: false
            asynchronous: false
            visible: window.isStreamMode
                && window.hideLocalCursor
                && window.isPointerLocked
                && window.localCursorVisible
                && !window.shouldHidePredictedCursor()
            x: videoContainer.x + Math.max(0, Math.min(videoContainer.width, window.localCursorVisualX)) - window.cursorHotspotX(window.hostCursorKind)
            y: videoContainer.y + Math.max(0, Math.min(videoContainer.height, window.localCursorVisualY)) - window.cursorHotspotY(window.hostCursorKind)
            z: 200
        }

    // Global Shortcut to escape pointer lock (always active regardless of focus)
    Shortcut {
        sequences: ["Ctrl+Alt+Escape", "Ctrl+Alt+Esc"]
        onActivated: {
            window.pauseSystemKeys();
            window.isPointerLocked = false;
            menuBar.open();
        }
    }



    // Stats overlay: what tells how the session is doing, and a line for
    // each thing that is wrong (shown only then).
    Rectangle {
        id: statsOverlay
        objectName: "statsOverlay"
        visible: window.showStats
        anchors.right: parent.right
        anchors.top: parent.top

        // Push down smoothly when menu is active to avoid overlap
        anchors.topMargin: menuBar.active ? 72 : 16
        Behavior on anchors.topMargin {
            NumberAnimation { duration: 250; easing.type: Easing.OutCubic }
        }

        anchors.rightMargin: 16
        // Wide enough for "h264_nvenc · RTX 2060" beside its label.
        width: 260
        height: statsColumn.implicitHeight + 24
        radius: 12
        color: Theme.live.surfaceA(0.6)
        border.color: Theme.live.ink(0.08)
        border.width: 1

        readonly property color good: "#4ade80"
        readonly property color warn: "#fb923c"
        readonly property bool direct: window.connectionType === "P2P (Direct)"
        // Rows: [label, value, colour ("" = normal), shown]
        // Ping: to the agent and back, plus from the agent to the computer
        // it shows and back when it shows another (the bridge adds them).
        readonly property var rows: [
            [I18n.t("Ping"), window.pingStatText(window.pingMs), "", true],
            ["FPS", window.fps.toFixed(0), "", true],
            [I18n.t("Bitrate"), (window.bitrateKbps / 1000).toFixed(1) + " Mbps", "", true],
            ["Codec", window.activeCodec.toUpperCase(), "", true],
            [I18n.t("Encode"), window.encodeStatText(window.activeEncoderName, window.activeEncoderHw, window.agentGpuInfo), "", true],
            [I18n.t("Decode"), window.decodeStatText(window.activeDecodeBackend, window.decodeLatencyMs), "", true],
            [I18n.t("Network"), statsOverlay.direct ? I18n.t("Direct") : window.connectionType, statsOverlay.direct ? statsOverlay.good : statsOverlay.warn, true],
            [I18n.t("Packets lost"), String(window.packetsLost), statsOverlay.warn, window.packetsLost > 0],
            [I18n.t("Late frames"), String(window.queueLagEvents), statsOverlay.warn, window.queueLagEvents > 0],
            [I18n.t("Drawn by"), window.activePresentBackend, statsOverlay.warn, window.activePresentBackend.indexOf("CPU") !== -1],
            [I18n.t("Fallback"), window.decodeFallbackReason, statsOverlay.warn, window.decodeFallbackReason !== "None" && window.decodeFallbackReason !== ""]
        ]

        Column {
            id: statsColumn
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 12
            spacing: 6

            Repeater {
                model: statsOverlay.rows.length
                Item {
                    readonly property var row: statsOverlay.rows[index]
                    visible: row[3]
                    height: visible ? 15 : 0
                    anchors.left: parent ? parent.left : undefined
                    anchors.right: parent ? parent.right : undefined
                    Text {
                        id: statLabel
                        textFormat: Text.PlainText
                        text: row[0]
                        color: Theme.live.textMuted
                        font.pixelSize: 11
                        anchors.left: parent.left
                    }
                    Text {
                        textFormat: Text.PlainText
                        text: row[1]
                        color: row[2] !== "" ? row[2] : Theme.live.textStrong
                        font.pixelSize: 11
                        font.bold: true
                        anchors.right: parent.right
                        width: parent.width - statLabel.width - 10
                        horizontalAlignment: Text.AlignRight
                        // What is too long loses its middle: the
                        // encoder's name and the end of the GPU's stay.
                        elide: Text.ElideMiddle
                    }
                }
            }
        }
    }

    // Interactive Notch/Floating trigger button when menu is hidden
    Rectangle {
        id: menuTrigger
        anchors.top: parent.top
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 4
        width: 80
        height: 24
        radius: 12
        
        color: triggerMouseArea.containsMouse ? Theme.live.surfaceA(0.8) : Qt.rgba(0, 0, 0, 0.05)
        border.color: triggerMouseArea.containsMouse ? Theme.live.accentA(0.35) : Theme.live.ink(0.04)
        border.width: 1
        visible: !menuBar.active && !window.isPointerLocked
        
        opacity: triggerMouseArea.containsMouse ? 1.0 : 0.25
        
        Behavior on opacity {
            NumberAnimation { duration: 150 }
        }
        Behavior on color {
            ColorAnimation { duration: 150 }
        }
        Behavior on border.color {
            ColorAnimation { duration: 150 }
        }

        // Allow dragging the window by holding and dragging the notch
        DragHandler {
            target: null
            onActiveChanged: {
                if (active) {
                    window.startSystemMove();
                }
            }
        }

        Text {
            anchors.centerIn: parent
            text: "▼ MENU"
            font.pixelSize: 9
            font.bold: true
            color: triggerMouseArea.containsMouse ? Theme.live.accentFg : Theme.live.ink(0.25)
            
            Behavior on color {
                ColorAnimation { duration: 150 }
            }
        }

        MouseArea {
            id: triggerMouseArea
            anchors.fill: parent
            hoverEnabled: true
            onClicked: {
                menuBar.open();
            }
        }
    }

    // Overlay Menu Bar
    LunarisMenuBar {
        id: menuBar
        objectName: "menuBar"
        anchors.horizontalCenter: parent.horizontalCenter
        isPointerLocked: window.isPointerLocked
        showStats: window.showStats
        hideLocalCursor: window.hideLocalCursor
        hostOs: window.agentHostOs

        onSessionSettingsRequested: {
            if (sessionSettings.opened) sessionSettings.close();
            else sessionSettings.open();
        }

        onSendKeysRequested: (keys) => {
            window.sendKeyCombo(keys);
            rootContainer.forceActiveFocus();
        }

        onSendFilesRequested: {
            if (!ViewPrefs.fileTransfer) {
                shareToast.text = I18n.t("File transfer is turned off in the session settings");
                shareToast.opacity = 1;
                shareToastTimer.restart();
                return;
            }
            fileManager.openCurrent();
        }

        onFullscreenToggled: {
            if (window.visibility === Window.FullScreen) {
                window.visibility = Window.Windowed;
            } else {
                window.visibility = Window.FullScreen;
            }
        }

        onLockToggled: {
            window.isPointerLocked = !window.isPointerLocked;
        }

        onStatsToggled: {
            window.showStats = !window.showStats;
        }

        onCursorHideToggled: {
            window.hideLocalCursor = !window.hideLocalCursor;
        }

        onSettingsChanged: (res, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol) => {
            window.renderBackend = String(renderBackend || (disableCuda ? "software" : "auto_gpu")).toLowerCase();
            window.useCuda = window.renderBackend !== "software";
            window.activeInputProtocol = String(inputProtocol).toLowerCase();
            bridge.updateStreamConfigWithBackend(res, fps, codec, bitrate, queueLimit, disableCuda, window.renderBackend, inputProtocol);
            var parts = res.split("x");
            if (parts.length === 2) {
                window.streamWidth = parseInt(parts[0]);
                window.streamHeight = parseInt(parts[1]);
            }
        }

        onCollapsed: {
            window.ignoreMenuHover = true;
            hoverCooldownTimer.restart();
            rootContainer.forceActiveFocus();
        }

        onMinimizeTriggered: {
            window.visibility = Window.Minimized;
        }

        onWindowToggleTriggered: {
            if (window.visibility === Window.FullScreen) {
                window.visibility = Window.Windowed;
            } else {
                window.visibility = Window.FullScreen;
            }
        }

        onExitTriggered: {
            bridge.stopStream();
            if (bridge.hasConnectionArgs()) {
                window.exitRequested = true;
                Qt.quit();
            } else {
                window.isStreamMode = false;
                sessionSettings.close();
            }
        }
    }

    // Files dropped on the stream go to the remote computer.
    DropArea {
        anchors.fill: parent
        enabled: window.isStreamMode && !fileManager.visible
        keys: ["text/uri-list"]
        onEntered: (drag) => { dropHint.visible = ViewPrefs.fileTransfer; }
        onExited: dropHint.visible = false
        onDropped: (drop) => {
            dropHint.visible = false;
            if (!drop.hasUrls) return;
            if (!ViewPrefs.fileTransfer) {
                shareToast.text = I18n.t("File transfer is turned off in the session settings");
                shareToast.opacity = 1;
                shareToastTimer.restart();
                return;
            }
            // Through the file connection (opened for this computer).
            if (!fileManager.openCurrent()) return;
            bridge.filesUploadDropped(drop.urls.map(function (u) { return u.toString(); }).join("\n"));
            drop.accept();
        }
    }
    Rectangle {
        id: dropHint
        visible: false
        anchors.fill: parent
        anchors.margins: 16
        z: 900
        radius: 16
        color: Qt.rgba(37 / 255, 99 / 255, 235 / 255, 0.18)
        border.color: Theme.live.accent
        border.width: 2
        Text {
            anchors.centerIn: parent
            text: I18n.t("Drop to send to the remote computer (Downloads/Lunaris)")
            color: "#ffffff"
            font.pixelSize: 18
            font.weight: Font.DemiBold
        }
    }

    // Files of the remote computer (its own connection): from the session
    // menu, or files dropped on the stream.
    FileManager {
        id: fileManager
        anchors.fill: parent
        z: 960
    }

    // This device's settings for the session (the menu's gear).
    SessionSettings {
        id: sessionSettings
        showStats: window.showStats
        renderBackend: window.renderBackend
        systemKeysKind: window.systemKeysKind
        onStatsToggled: (on) => window.showStats = on
        onRenderBackendPicked: (backend) => menuBar.setRenderBackend(backend)
        onClosed: if (window.isStreamMode) rootContainer.forceActiveFocus()
    }

    // Clipboard/file sharing news (progress, saved, errors).
    Rectangle {
        id: shareToast
        property alias text: shareToastText.text
        visible: opacity > 0 && window.isStreamMode
        opacity: 0
        z: 950
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottomMargin: 72
        width: Math.min(parent.width - 48, shareToastText.implicitWidth + 36)
        height: 40
        radius: 20
        color: Qt.rgba(17 / 255, 17 / 255, 20 / 255, 0.92)
        border.color: Theme.live.borderStrong
        Behavior on opacity { NumberAnimation { duration: 200 } }
        Text {
            id: shareToastText
            textFormat: Text.PlainText
            anchors.centerIn: parent
            width: parent.width - 32
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideMiddle
            color: Theme.live.textStrong
            font.pixelSize: 13
        }
        Timer { id: shareToastTimer; interval: 4000; onTriggered: shareToast.opacity = 0 }
    }

    // Pointer Lock Banner Notification
    Rectangle {
        id: pointerLockBanner
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottomMargin: 32
        width: 320
        height: 36
        radius: 6
        color: Theme.live.surfaceA(0.9)
        border.color: Theme.live.accentFg
        border.width: 1
        visible: window.isPointerLocked && !window.isEscHeld && window.showLockBanner

        Text {
            anchors.centerIn: parent
            text: "Mouse Locked. Hold ESC to release."
            font.pixelSize: 11
            color: Theme.live.textStrong
        }
    }

    // ESC Hold Unlock Progress Bar (Glassmorphic)
    Rectangle {
        id: escUnlockProgress
        anchors.top: parent.top
        anchors.topMargin: 48
        anchors.horizontalCenter: parent.horizontalCenter
        width: 280
        height: 52
        radius: 10
        color: Theme.live.surfaceA(0.92)
        border.color: Theme.live.accentA(0.35)
        border.width: 1.2
        visible: window.isEscHeld && window.isPointerLocked
        z: 99999

        Column {
            anchors.centerIn: parent
            spacing: 6

            Text {
                text: "Holding ESC to unlock cursor..."
                font.pixelSize: 11
                font.bold: true
                color: Theme.live.textStrong
                anchors.horizontalCenter: parent.horizontalCenter
            }

            Rectangle {
                width: 240
                height: 4
                color: Theme.live.ink(0.12)
                radius: 2
                anchors.horizontalCenter: parent.horizontalCenter

                Rectangle {
                    id: progressBarInner
                    width: 0
                    height: parent.height
                    color: Theme.live.accent
                    radius: 2

                    Behavior on width {
                        NumberAnimation { duration: 1200 }
                    }
                }
            }
        }

        onVisibleChanged: {
            if (visible) {
                progressBarInner.width = 240;
            } else {
                progressBarInner.width = 0;
            }
        }
    }
    }

    // Update notice above the window without a link (UpdateBanner.qml),
    // from the updater's state.
    UpdateBanner {
        id: updateBanner
        status: window.updateStatus
        visible: shown && !window.isStreamMode
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        z: 9999
    }

    // Started without a link: where streams come from (the server's web
    // page), the trusted servers, language, theme, updates, version.
    Launcher {
        id: launcherView
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.top: updateBanner.visible ? updateBanner.bottom : parent.top
        visible: !window.isStreamMode
        updateStatus: window.updateStatus
    }
    }

    property bool isStreamMode: false
    property int streamWidth: 1920
    property int streamHeight: 1080

    // This device's display and sound settings (ViewPrefs): saved, and the
    // sound applied to the audio output whenever they change.
    Connections {
        target: ViewPrefs
        function onDisplayModeChanged() { bridge.saveStore("view_prefs", ViewPrefs.toJson()); }
        function onAudioEnabledChanged() { window.applyAudio(); }
        function onVolumeChanged() { window.applyAudio(); }
        function onRecordMicrophoneChanged() { window.applyAudio(); }
        function onClipboardSyncChanged() { window.applySharing(); }
        function onFileTransferChanged() { window.applySharing(); }
        function onGamepadChanged() { window.applySharing(); }
        function onSystemKeysChanged() { bridge.saveStore("view_prefs", ViewPrefs.toJson()); }
    }
    function applySharing() {
        bridge.setShareOptions(ViewPrefs.clipboardSync, ViewPrefs.fileTransfer);
        bridge.setGamepadEnabled(ViewPrefs.gamepad);
        bridge.saveStore("view_prefs", ViewPrefs.toJson());
    }
    function applyAudio() {
        bridge.setAudio(ViewPrefs.audioEnabled, ViewPrefs.volume);
        bridge.setRecordMicrophone(ViewPrefs.recordMicrophone);
        bridge.saveStore("view_prefs", ViewPrefs.toJson());
    }

    Component.onCompleted: {
        ViewPrefs.fromJson(bridge.loadStore("view_prefs"));
        bridge.setAudio(ViewPrefs.audioEnabled, ViewPrefs.volume);
        bridge.setRecordMicrophone(ViewPrefs.recordMicrophone);
        bridge.setShareOptions(ViewPrefs.clipboardSync, ViewPrefs.fileTransfer);
        bridge.setGamepadEnabled(ViewPrefs.gamepad);

        // Center window procedurally on startup to avoid permanent bindings
        window.x = (Screen.width - window.width) / 2;
        window.y = (Screen.height - window.height) / 2;

        window.visible = true;

        if (bridge.hasConnectionArgs()) {
            window.isStreamMode = true;
            bridge.setVideoSink(videoOutput.videoSink);
            bridge.startStream();
            bridge.requestSettings();
            rootContainer.forceActiveFocus();
        } else {
            window.isStreamMode = false;
            linkDialog.openIfPending();
        }
    }

    // The updater checks by itself (shortly after the start, then daily,
    // unless turned off); this shows its state. Once an update is
    // installed the app quits and starts again.
    function pollUpdate() {
        var s;
        try { s = JSON.parse(bridge.updateStatus()); } catch (e) { return; }
        window.updateStatus = s;
        if (s.state === "restart" && bridge.updateRestart()) {
            window.exitRequested = true;
            Qt.quit();
        }
    }
    Timer {
        interval: 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: window.pollUpdate()
    }


    // A lunaris:// link to a server that is not trusted: nothing connects
    // until the user says so.
    DialogFrame {
        id: linkDialog
        objectName: "linkDialog"
        property string server: ""
        property string computer: ""
        // http/ws: the session's sign-in and picture travel unencrypted.
        property bool encrypted: true
        // A trusted server's link for another account than the one trusted.
        property string account: ""
        property string knownAccount: ""
        readonly property bool otherAccount: account !== "" && knownAccount !== ""
        property bool answered: false
        width: 460
        function openIfPending() {
            trustBox.checked = false;
            var raw = bridge.pendingLinkInfo();
            if (raw === "") return false;
            var info = {};
            try { info = JSON.parse(raw); } catch (e) { bridge.answerPendingLink(false); return false; }
            server = info.server || "";
            computer = info.computer || "";
            // Older bridges do not say: shown as encrypted, as before.
            encrypted = info.encrypted !== false;
            account = info.account || "";
            knownAccount = info.knownAccount || "";
            answered = false;
            open();
            return true;
        }
        function answer(accept) {
            if (answered) return;
            answered = true;
            // Yes: the stream starts with deeplinkReceived once the app has
            // traded the link's one-time ticket with the server.
            bridge.answerPendingLink(accept, accept && trustBox.checked);
            close();
        }
        onOpened: linkCancelButton.forceActiveFocus()
        onClosed: answer(false)
        contentItem: ColumnLayout {
            spacing: 12
            Text {
                Layout.fillWidth: true
                text: linkDialog.otherAccount ? I18n.t("Connect with another account?") : I18n.t("Connect to another server?")
                color: Theme.textStrong
                font.pixelSize: 16
                font.weight: Font.DemiBold
                wrapMode: Text.WordWrap
            }
            Text {
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: linkDialog.otherAccount
                    ? I18n.t("A link wants to connect this app to %1 as the account %2, but you trust this server for the account %3. Only continue if you opened this link yourself.").arg(linkDialog.server).arg(linkDialog.account).arg(linkDialog.knownAccount)
                    : I18n.t("A link wants to connect this app to %1. Only continue if you opened this link yourself.").arg(linkDialog.server)
                color: Theme.text
                font.pixelSize: 13
                wrapMode: Text.WrapAnywhere
            }
            Text {
                objectName: "linkNotEncrypted"
                Layout.fillWidth: true
                visible: !linkDialog.encrypted
                text: I18n.t("Not encrypted: this server is reached over http, so others on the network can see and change the session.")
                color: Theme.danger
                font.pixelSize: 12
                wrapMode: Text.WordWrap
            }
            Text {
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: I18n.t("Computer: %1").arg(linkDialog.computer)
                color: Theme.textMuted
                font.pixelSize: 12
                elide: Text.ElideRight
            }
            CheckRow {
                id: trustBox
                objectName: "linkTrustBox"
                Layout.fillWidth: true
                text: linkDialog.otherAccount ? I18n.t("Always trust this server for the account %1").arg(linkDialog.account) : I18n.t("Always trust this server")
            }
            Text {
                Layout.fillWidth: true
                text: trustBox.checked
                    ? I18n.t("Links to this server will connect without asking, with clipboard and file transfer.")
                    : I18n.t("Clipboard and file transfer stay off in this session.")
                color: Theme.textMuted
                font.pixelSize: 12
                wrapMode: Text.WordWrap
            }
            Text {
                objectName: "linkTrustNotEncrypted"
                Layout.fillWidth: true
                visible: trustBox.checked && !linkDialog.encrypted
                text: I18n.t("This server is not encrypted: someone on the network could pose as it, and its links would then connect without asking, with clipboard and file transfer.")
                color: Theme.danger
                font.pixelSize: 12
                wrapMode: Text.WordWrap
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton {
                    id: linkCancelButton
                    text: I18n.t("Cancel")
                    focus: true
                    onClicked: linkDialog.answer(false)
                }
                SecondaryButton { text: I18n.t("Connect"); danger: true; onClicked: linkDialog.answer(true) }
            }
        }
    }
}
