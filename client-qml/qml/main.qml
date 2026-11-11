import QtQuick
import QtQuick.Controls
import QtMultimedia
import Qt.labs.platform as Platform
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

    onClosing: (close) => {
        if (bridge.hasConnectionArgs()) {
            window.exitRequested = true;
        }
        if (!exitRequested && closeToTray) {
            close.accepted = false;
            window.hide();
        }
    }

    Platform.SystemTrayIcon {
        id: trayIcon
        visible: true
        icon.source: "qrc:/icon.png"
        tooltip: "Lunaris Client"

        menu: Platform.Menu {
            Platform.MenuItem {
                text: window.visible ? "Hide Client" : "Show Client"
                onTriggered: {
                    if (window.visible) {
                        window.hide()
                    } else {
                        window.show()
                        window.raise()
                        window.requestActivate()
                    }
                }
            }
            Platform.MenuItem {
                text: "Allow remote control"
                checkable: true
                checked: homeView.hostStatus.enabled
                onTriggered: homeView.setRemoteControl(!homeView.hostStatus.enabled)
            }
            Platform.MenuItem {
                text: "Autostart on Boot"
                checkable: true
                checked: window.autostartEnabled
                onTriggered: {
                    var nextState = !window.autostartEnabled;
                    bridge.setAutostartEnabled(nextState);
                    window.autostartEnabled = bridge.isAutostartEnabled();
                }
            }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: "Exit"
                onTriggered: {
                    window.exitRequested = true;
                    bridge.stopLocalAgent();
                    Qt.quit();
                }
            }
        }

        onActivated: (reason) => {
            if (reason === Platform.SystemTrayIcon.Trigger) {
                if (window.visible) {
                    window.hide();
                } else {
                    window.show();
                    window.raise();
                    window.requestActivate();
                }
            }
        }
    }

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
    property bool autostartEnabled: false
    property bool closeToTray: true
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
    property string latestVersion: ""
    property string releaseUrl: ""
    property bool showUpdateBanner: false
    property int hostCursorX: streamWidth / 2
    property int hostCursorY: streamHeight / 2
    property bool hostCursorVisible: false
    property string hostCursorKind: "arrow"
    property bool hostCursorMouseDown: false
    property bool hostCursorInWindowMoveSize: false
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

    function shouldHidePredictedCursor() {
        return window.agentHostOs === "windows"
            && window.hostCursorMouseDown
            && window.hostCursorInWindowMoveSize;
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
            keyboardGrabTimer.restart();
        } else {
            bridge.setPointerLocked(false);
            window.showLockBanner = false;
            bannerTimer.stop();
            keyboardGrabTimer.stop();
            bridge.setKeyboardGrab(false);
        }
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

            bridge.sendKeyEvent(event.key, event.modifiers, true);
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

            bridge.sendKeyEvent(event.key, event.modifiers, false);
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

        onNewVersionAvailable: (version, url) => {
            window.latestVersion = version
            window.releaseUrl = url
            window.showUpdateBanner = true
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

            if (url !== "") {
                window.isStreamMode = true;
                bridge.setVideoSink(videoOutput.videoSink);
                bridge.startStream();
                bridge.requestSettings();
                rootContainer.forceActiveFocus();
            }
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
            homeView.showError(message);
            homeView.forceActiveFocus();
        }

        onCapabilitiesUpdated: (displaysJson, encodersJson, gpuInfo, hostOs) => {
            dashboardView.settingsModal.updateCapabilities(displaysJson, encodersJson);
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

    // Periodic timer to poll events (REST / WS login / pairing results) from Rust backend
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
        onTriggered: {
            if (window.isPointerLocked) {
                bridge.setKeyboardGrab(true);
            }
        }
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
                visible: !window.useCuda || !window.nativeVideoActive

                onVisibleChanged: {
                    if (visible && videoOutput.videoSink) {
                        console.log("VideoOutput became visible, registering VideoSink: " + videoOutput.videoSink);
                        bridge.setVideoSink(videoOutput.videoSink);
                    }
                }
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
            anchors.fill: parent
            hoverEnabled: true
            acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
            // While HostCursor drives this item's cursor, keep the binding
            // constant so it never overwrites the pixmap cursor.
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
                window.hostCursorMouseDown = true;
                if (!window.isPointerLocked) {
                    var mapped = streamView.mapToItem(videoContainer, mouse.x, mouse.y);
                    window.updateLocalCursorPrediction(mapped.x, mapped.y);
                    bridge.sendMouseClick(getButtonCode(mouse.button), true);
                }
            }

            onReleased: (mouse) => {
                window.hostCursorMouseDown = mouse.buttons !== 0;
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
            window.isPointerLocked = false;
            menuBar.open();
        }
    }



    // Diagnostics Stats Overlay (Glassmorphism look)
    Rectangle {
        id: statsOverlay
        visible: window.showStats
        anchors.right: parent.right
        anchors.top: parent.top
        
        // Push down smoothly when menu is active to avoid overlap
        anchors.topMargin: menuBar.active ? 72 : 16
        Behavior on anchors.topMargin {
            NumberAnimation { duration: 250; easing.type: Easing.OutCubic }
        }
        
        anchors.rightMargin: 16
        width: 280
        height: 416
        radius: 16
        color: Theme.live.surfaceA(0.6)
        border.color: Theme.live.ink(0.08)
        border.width: 1

        Column {
            anchors.fill: parent
            anchors.margins: 14
            spacing: 8

            Text {
                text: "DIAGNOSTICS"
                font.pixelSize: 10
                font.bold: true
                font.letterSpacing: 1.5
                color: Theme.live.textMuted
            }

            Rectangle {
                height: 1
                color: Theme.live.ink(0.08)
                anchors.left: parent.left
                anchors.right: parent.right
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Ping (RTT)"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.pingMs.toFixed(1) + " ms"; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Decode Latency"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.decodeLatencyMs.toFixed(1) + " ms"; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "FPS"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.fps.toFixed(0); color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Bitrate"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.bitrateKbps.toFixed(0) + " Kbps"; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Requested Codec"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.requestedCodec.toUpperCase(); color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Negotiated Codec"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeCodec.toUpperCase(); color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Decode"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeDecodeBackend; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 165; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Present"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activePresentBackend; color: window.activePresentBackend.indexOf("CPU") === -1 ? "#4ade80" : "#fb923c"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 165; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "GPU Decode"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.gpuDecodeEnabled ? "Enabled" : "Disabled"; color: window.gpuDecodeEnabled ? "#4ade80" : "#fb923c"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Fallback"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.decodeFallbackReason; color: window.decodeFallbackReason === "None" ? "#4ade80" : "#fb923c"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 165; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Encoder"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeEncoderName + " (" + window.activeEncoderHw + ")"; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 150; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "GPU"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.agentGpuInfo; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 170; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Host OS"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.agentHostOs; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Requested Encoder"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.requestedEncoder; color: Theme.live.textStrong; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Input Protocol"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeInputProtocol.toUpperCase(); color: window.activeInputProtocol === "webtransport" ? "#4ade80" : Theme.live.accentFg; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Packet Loss"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.packetLossEvents + " events / " + window.packetsLost + " pkts"; color: window.packetLossEvents > 0 ? "#fb923c" : "#4ade80"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Queue Lag"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.queueLagEvents + " events / max " + window.maxQueueLag; color: window.queueLagEvents > 0 ? "#fb923c" : "#4ade80"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Network Path"; color: Theme.live.textMuted; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.connectionType; color: window.connectionType === "P2P (Direct)" ? "#4ade80" : "#fb923c"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
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
        anchors.horizontalCenter: parent.horizontalCenter
        isPointerLocked: window.isPointerLocked
        showStats: window.showStats
        hideLocalCursor: window.hideLocalCursor
        
        onSendFilesRequested: {
            if (!ViewPrefs.fileTransfer) {
                shareToast.text = I18n.t("File transfer is turned off in Settings");
                shareToast.opacity = 1;
                shareToastTimer.restart();
                return;
            }
            sendFilesDialog.open();
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
                if (homeView.dashboard.isLoggedIn) bridge.fetchHosts();
                homeView.setStatus("", false);
                homeView.forceActiveFocus();
            }
        }
    }

    // Files dropped on the stream go to the remote computer.
    DropArea {
        anchors.fill: parent
        enabled: window.isStreamMode
        keys: ["text/uri-list"]
        onEntered: (drag) => { dropHint.visible = ViewPrefs.fileTransfer; }
        onExited: dropHint.visible = false
        onDropped: (drop) => {
            dropHint.visible = false;
            if (!drop.hasUrls) return;
            if (!ViewPrefs.fileTransfer) {
                shareToast.text = I18n.t("File transfer is turned off in Settings");
                shareToast.opacity = 1;
                shareToastTimer.restart();
                return;
            }
            bridge.sendFiles(drop.urls.map(function (u) { return u.toString(); }).join("\n"));
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

    Platform.FileDialog {
        id: sendFilesDialog
        title: I18n.t("Send files to the remote computer")
        fileMode: Platform.FileDialog.OpenFiles
        onAccepted: bridge.sendFiles(files.map(function (u) { return u.toString(); }).join("\n"))
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

    // Update Banner
    Rectangle {
        id: updateBanner
        visible: window.showUpdateBanner && !window.isStreamMode
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: 40
        color: Theme.live.surfaceA(0.95)
        border.color: Theme.live.accentA(0.3)
        border.width: 1
        z: 9999

        Row {
            anchors.centerIn: parent
            spacing: 12
            
            Text {
                text: "A new version of Lunaris Client (" + window.latestVersion + ") is available!"
                color: Theme.live.textStrong
                font.pixelSize: 13
                font.bold: true
                anchors.verticalCenter: parent.verticalCenter
            }

            Rectangle {
                width: 90
                height: 26
                color: Theme.live.accent
                radius: 4
                anchors.verticalCenter: parent.verticalCenter
                
                Text {
                    text: "Update Now"
                    color: Theme.live.bg
                    font.pixelSize: 11
                    font.bold: true
                    anchors.centerIn: parent
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        Qt.openUrlExternally(window.releaseUrl)
                    }
                }
            }

            // Close button
            Text {
                text: "✕"
                color: Theme.live.textMuted
                font.pixelSize: 14
                anchors.verticalCenter: parent.verticalCenter
                
                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -4
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        window.showUpdateBanner = false
                    }
                }
            }
        }
    }

    // Home: this computer, remote computers, settings
    Home {
        id: homeView
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.top: updateBanner.visible ? updateBanner.bottom : parent.top
        visible: !window.isStreamMode
        focus: !window.isStreamMode

        onStartSessionRequested: (server, token, hostId, hostName, appId, res, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol, encoder, displayId, virtualDisplay) => {
            window.renderBackend = String(renderBackend || (disableCuda ? "software" : "auto_gpu")).toLowerCase();
            window.useCuda = window.renderBackend !== "software";
            window.isStreamMode = true;
            var parts = res.split("x");
            if (parts.length === 2) {
                window.streamWidth = parseInt(parts[0]);
                window.streamHeight = parseInt(parts[1]);
            }
            bridge.startGameSessionWithBackend(server, token, hostId, hostName, appId, res, fps, codec, bitrate, queueLimit, disableCuda, window.renderBackend, inputProtocol, encoder, displayId, virtualDisplay);
            bridge.setVideoSink(videoOutput.videoSink);
            bridge.requestSettings();
            rootContainer.forceActiveFocus();
        }
    }
    }

    property bool isStreamMode: false
    // The server sign-in and computer list (inside Home's Server tab).
    readonly property var dashboardView: homeView.dashboard
    property int streamWidth: 1920
    property int streamHeight: 1080

    // This device's display and sound settings (ViewPrefs): saved, and the
    // sound applied to the audio output whenever they change.
    Connections {
        target: ViewPrefs
        function onDisplayModeChanged() { bridge.saveStore("view_prefs", ViewPrefs.toJson()); }
        function onAudioEnabledChanged() { window.applyAudio(); }
        function onVolumeChanged() { window.applyAudio(); }
        function onClipboardSyncChanged() { window.applySharing(); }
        function onFileTransferChanged() { window.applySharing(); }
        function onGamepadChanged() { window.applySharing(); }
    }
    function applySharing() {
        bridge.setShareOptions(ViewPrefs.clipboardSync, ViewPrefs.fileTransfer);
        bridge.setGamepadEnabled(ViewPrefs.gamepad);
        bridge.saveStore("view_prefs", ViewPrefs.toJson());
    }
    function applyAudio() {
        bridge.setAudio(ViewPrefs.audioEnabled, ViewPrefs.volume);
        bridge.saveStore("view_prefs", ViewPrefs.toJson());
    }

    Component.onCompleted: {
        ViewPrefs.fromJson(bridge.loadStore("view_prefs"));
        bridge.setAudio(ViewPrefs.audioEnabled, ViewPrefs.volume);
        bridge.setShareOptions(ViewPrefs.clipboardSync, ViewPrefs.fileTransfer);
        bridge.setGamepadEnabled(ViewPrefs.gamepad);

        // Center window procedurally on startup to avoid permanent bindings
        window.x = (Screen.width - window.width) / 2;
        window.y = (Screen.height - window.height) / 2;

        window.autostartEnabled = bridge.isAutostartEnabled();

        if (bridge.shouldStartMinimized()) {
            window.visible = false;
        } else {
            window.visible = true;
        }

        if (bridge.hasConnectionArgs()) {
            window.isStreamMode = true;
            bridge.setVideoSink(videoOutput.videoSink);
            bridge.startStream();
            bridge.requestSettings();
            rootContainer.forceActiveFocus();
        } else {
            window.isStreamMode = false;
        }
        bridge.checkForUpdates();
    }


}
