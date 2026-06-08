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
    title: "Lunaris Player Client"
    color: window.isStreamMode ? "#000000" : "#0a0b10"

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
                text: dashboardView.localAgentRunning ? "Stop Local Agent" : "Start Local Agent"
                onTriggered: {
                    if (dashboardView.localAgentRunning) {
                        bridge.stopLocalAgent();
                    } else {
                        if (dashboardView.localAgentHostname && dashboardView.serverUrl && dashboardView.agentToken) {
                            bridge.startLocalAgent(dashboardView.serverUrl, dashboardView.agentToken, dashboardView.localAgentHostname);
                        } else {
                            window.show()
                            window.raise()
                            window.requestActivate()
                            dashboardView.showPairingPage = true
                        }
                    }
                }
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
    property bool ignoreMenuHover: false
    property bool autostartEnabled: false
    property bool closeToTray: true
    property real pingMs: 0.0
    property real decodeLatencyMs: 0.0
    property real fps: 0.0
    property real bitrateKbps: 0.0
    property string activeCodec: "H264"
    property string activeDecodeBackend: "Unknown"
    property string activePresentBackend: "Unknown"
    property string connectionType: "P2P (Direct)"
    property string activeEncoderName: "Unknown"
    property string activeEncoderHw: "Unknown"
    property string agentGpuInfo: "Unknown"
    property string requestedEncoder: "auto"
    property string activeInputProtocol: "webrtc"
    property bool useCuda: true
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

    function cursorAssetName(kind) {
        var normalized = normalizeCursorKind(kind);
        if (normalized === "unknown") normalized = "arrow";
        if (normalized === "resize_ns") return "windows-aero-resize-ns.png";
        if (normalized === "resize_ew") return "windows-aero-resize-ew.png";
        if (normalized === "resize_nesw") return "windows-aero-resize-nesw.png";
        if (normalized === "resize_nwse") return "windows-aero-resize-nwse.png";
        return "windows-aero-" + normalized + ".png";
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
        if (normalized === "arrow" || normalized === "unknown") return 0;
        if (normalized === "hand") return 6;
        return 16;
    }

    function cursorHotspotY(kind) {
        if (window.hasMatchingNativeCursor(kind)) return window.nativeCursorHotspotY;
        var normalized = normalizeCursorKind(kind);
        if (normalized === "arrow" || normalized === "unknown") return 0;
        if (normalized === "hand") return 1;
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

    onIsPointerLockedChanged: {
        bridge.setPointerLocked(isPointerLocked);
        if (isPointerLocked) {
            rootContainer.forceActiveFocus();
            window.showLockBanner = true;
            bannerTimer.restart();
            keyboardGrabTimer.restart();
        } else {
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
        onStatsUpdated: (ping, decode, frames, bit, codec, decodeBackend, presentBackend, connType) => {
            window.pingMs = ping
            window.decodeLatencyMs = decode
            window.fps = frames
            window.bitrateKbps = bit
            window.activeCodec = codec
            window.activeDecodeBackend = decodeBackend
            window.activePresentBackend = presentBackend
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

        onEncoderStatusUpdated: (encoder, hwType, gpuInfo, requestedEncoder, hostOs) => {
            window.activeEncoderName = encoder || "Unknown"
            window.activeEncoderHw = hwType || "Unknown"
            window.agentGpuInfo = gpuInfo || window.agentGpuInfo || "Unknown"
            window.requestedEncoder = requestedEncoder || "auto"
            if (hostOs !== "") window.agentHostOs = String(hostOs).toLowerCase()
        }

        onLocalCursorDelta: (rx, ry) => {
            window.updateLocalCursorDelta(rx, ry)
        }

        onNewVersionAvailable: (version, url) => {
            window.latestVersion = version
            window.releaseUrl = url
            window.showUpdateBanner = true
        }

        onSettingsLoaded: (res, fps, codec, bitrate, queueLimit, hostName, disableCuda, inputProtocol) => {
            menuBar.initializeSettings(res, fps, codec, bitrate, queueLimit, hostName, disableCuda, inputProtocol);
            window.useCuda = !disableCuda;
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
        visible: window.isStreamMode
        focus: window.isStreamMode

        // Container to preserve aspect ratio of the stream
        Item {
            id: videoContainer
            anchors.centerIn: parent
            width: {
                var aspect = window.streamWidth / window.streamHeight;
                var parentAspect = parent.width / parent.height;
                if (parentAspect > aspect) {
                    return parent.height * aspect;
                } else {
                    return parent.width;
                }
            }
            height: {
                var aspect = window.streamWidth / window.streamHeight;
                var parentAspect = parent.width / parent.height;
                if (parentAspect > aspect) {
                    return parent.height;
                } else {
                    return parent.width / aspect;
                }
            }

            // Video Output Area
            VideoOutput {
                id: videoOutput
                anchors.fill: parent
                fillMode: VideoOutput.Stretch
                visible: !gpuVideoItem.cudaSupported || !window.useCuda || !gpuVideoItem.cudaActive

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

        }

        // Full-window mouse input overlay — must cover the ENTIRE window,
        // not just videoContainer, so clicks in letterbox areas (e.g., taskbar)
        // are captured and correctly mapped to the remote desktop.
        MouseArea {
            id: streamMouseArea
            anchors.fill: parent
            hoverEnabled: true
            acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
            cursorShape: (window.isPointerLocked || window.hideLocalCursor || window.shouldHidePredictedCursor()) ? Qt.BlankCursor : window.qtCursorShape(window.hostCursorKind)

            onPositionChanged: (mouse) => {
                if (mouse.y >= 50 && window.ignoreMenuHover) {
                    window.ignoreMenuHover = false;
                }
                
                if (!window.isPointerLocked) {
                    // Map window coordinates to videoContainer-local coordinates
                    var mapped = streamView.mapToItem(videoContainer, mouse.x, mouse.y);
                    window.updateLocalCursorPrediction(mapped.x, mapped.y);
                    bridge.sendMouseMove(mapped.x, mapped.y, videoContainer.width, videoContainer.height, 0, 0, false);
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
                    bridge.sendMouseWheel(wheel.angleDelta.y);
                }
            }

            function getButtonCode(btn) {
                if (btn === Qt.LeftButton) return 1;
                if (btn === Qt.MiddleButton) return 2;
                if (btn === Qt.RightButton) return 3;
                return 0;
            }
        }

        Image {
            id: hostCursorOverlay
            source: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorSource : window.cursorSourceForKind(window.hostCursorKind)
            width: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorWidth : 32
            height: window.hasMatchingNativeCursor(window.hostCursorKind) ? window.nativeCursorHeight : 32
            fillMode: Image.PreserveAspectFit
            smooth: false
            mipmap: false
            cache: false
            asynchronous: false
            visible: window.isStreamMode
                && window.hideLocalCursor
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
        height: 320
        radius: 16
        color: Qt.rgba(20/255, 20/255, 20/255, 0.6)
        border.color: Qt.rgba(255/255, 255/255, 255/255, 0.08)
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
                color: "#94a3b8"
            }

            Rectangle {
                height: 1
                color: Qt.rgba(255/255, 255/255, 255/255, 0.08)
                anchors.left: parent.left
                anchors.right: parent.right
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Ping (RTT)"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.pingMs.toFixed(1) + " ms"; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Decode Latency"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.decodeLatencyMs.toFixed(1) + " ms"; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "FPS"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.fps.toFixed(0); color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Bitrate"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.bitrateKbps.toFixed(0) + " Kbps"; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Codec"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeCodec.toUpperCase(); color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Decode"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeDecodeBackend; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 165; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Present"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activePresentBackend; color: window.activePresentBackend.indexOf("CPU") === -1 ? "#4ade80" : "#fb923c"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 165; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Encoder"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeEncoderName + " (" + window.activeEncoderHw + ")"; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 150; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "GPU"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.agentGpuInfo; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right; elide: Text.ElideLeft; width: 170; horizontalAlignment: Text.AlignRight }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Host OS"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.agentHostOs; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Requested Encoder"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.requestedEncoder; color: "#ffffff"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Input Protocol"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
                Text { text: window.activeInputProtocol.toUpperCase(); color: window.activeInputProtocol === "webtransport" ? "#4ade80" : "#38bdf8"; font.pixelSize: 11; font.bold: true; anchors.right: parent.right }
            }

            Item {
                height: 16
                anchors.left: parent.left
                anchors.right: parent.right
                Text { text: "Network Path"; color: "#94a3b8"; font.pixelSize: 11; font.bold: true; anchors.left: parent.left }
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
        
        color: triggerMouseArea.containsMouse ? Qt.rgba(15/255, 22/255, 38/255, 0.8) : Qt.rgba(0, 0, 0, 0.05)
        border.color: triggerMouseArea.containsMouse ? Qt.rgba(0, 240/255, 255/255, 0.35) : Qt.rgba(255/255, 255/255, 255/255, 0.04)
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
            color: triggerMouseArea.containsMouse ? "#00f0ff" : Qt.rgba(255/255, 255/255, 255/255, 0.25)
            
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

        onSettingsChanged: (res, fps, codec, bitrate, queueLimit, disableCuda, inputProtocol) => {
            window.useCuda = !disableCuda;
            window.activeInputProtocol = String(inputProtocol).toLowerCase();
            bridge.updateStreamConfig(res, fps, codec, bitrate, queueLimit, disableCuda, inputProtocol);
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
                bridge.fetchHosts();
                dashboardView.forceActiveFocus();
            }
        }
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
        color: Qt.rgba(15/255, 23/255, 42/255, 0.9)
        border.color: "#818cf8"
        border.width: 1
        visible: window.isPointerLocked && !window.isEscHeld && window.showLockBanner

        Text {
            anchors.centerIn: parent
            text: "Mouse Locked. Hold ESC to release."
            font.pixelSize: 11
            color: "#f1f5f9"
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
        color: Qt.rgba(15/255, 22/255, 38/255, 0.92)
        border.color: Qt.rgba(0, 240/255, 255/255, 0.35)
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
                color: "#f1f5f9"
                anchors.horizontalCenter: parent.horizontalCenter
            }

            Rectangle {
                width: 240
                height: 4
                color: Qt.rgba(1, 1, 1, 0.12)
                radius: 2
                anchors.horizontalCenter: parent.horizontalCenter

                Rectangle {
                    id: progressBarInner
                    width: 0
                    height: parent.height
                    color: "#00f0ff"
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
        color: Qt.rgba(20/255, 30/255, 55/255, 0.95)
        border.color: Qt.rgba(0/255, 240/255, 255/255, 0.3)
        border.width: 1
        z: 9999

        Row {
            anchors.centerIn: parent
            spacing: 12
            
            Text {
                text: "A new version of Lunaris Client (" + window.latestVersion + ") is available!"
                color: "#f1f5f9"
                font.pixelSize: 13
                font.bold: true
                anchors.verticalCenter: parent.verticalCenter
            }

            Rectangle {
                width: 90
                height: 26
                color: "#00f0ff"
                radius: 4
                anchors.verticalCenter: parent.verticalCenter
                
                Text {
                    text: "Update Now"
                    color: "#080c14"
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
                color: "#94a3b8"
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

    // Dashboard View
    Dashboard {
        id: dashboardView
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.top: updateBanner.visible ? updateBanner.bottom : parent.top
        visible: !window.isStreamMode
        focus: !window.isStreamMode

        onStartSessionRequested: (server, token, hostId, hostName, appId, res, fps, codec, bitrate, queueLimit, disableCuda, inputProtocol, encoder, displayId, virtualDisplay) => {
            window.useCuda = !disableCuda;
            window.isStreamMode = true;
            var parts = res.split("x");
            if (parts.length === 2) {
                window.streamWidth = parseInt(parts[0]);
                window.streamHeight = parseInt(parts[1]);
            }
            bridge.startGameSession(server, token, hostId, hostName, appId, res, fps, codec, bitrate, queueLimit, disableCuda, inputProtocol, encoder, displayId, virtualDisplay);
            bridge.setVideoSink(videoOutput.videoSink);
            bridge.requestSettings();
            rootContainer.forceActiveFocus();
        }
    }
    }

    property bool isStreamMode: false
    property int streamWidth: 1920
    property int streamHeight: 1080

    Component.onCompleted: {
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
