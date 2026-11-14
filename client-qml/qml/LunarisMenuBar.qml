import QtQuick
import QtQuick.Controls

Item {
    id: menuContainer
    // Wide enough for both sides, so the right-hand controls never cover
    // the back button (they grew with the chroma and resolution choices).
    width: leftSection.width + rightSection.width + 16 + 16 + 24
    height: 48

    // Same choices as the web client (StreamOptions), plus this screen's rate.
    property var fpsPresets: StreamOptions.fpsFor(bridge.screenRefreshRate())
    
    // Custom signals
    signal fullscreenToggled()
    signal lockToggled()
    signal statsToggled()
    // "Send file" button (main.qml opens the file dialog).
    signal sendFilesRequested()
    signal cursorHideToggled()
    signal settingsChanged(string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol)
    signal exitTriggered()
    signal collapsed()
    signal minimizeTriggered()
    signal windowToggleTriggered()

    // Status properties bound from parent
    property bool isPointerLocked: false
    property bool showStats: true
    property bool hideLocalCursor: true

    // Pin status property (defaults to false -> auto hides)
    property bool isPinned: false

    // Slide down transition properties
    property bool active: false
    y: active ? 12 : -menuContainer.height
    
    Behavior on y {
        NumberAnimation { duration: 250; easing.type: Easing.OutCubic }
    }

    // Reusable ToolTip Component
    component LunarisToolTip : ToolTip {
        id: controlToolTip
        visible: parent.hovered
        delay: 500
        contentItem: Text {
            text: controlToolTip.text
            color: Theme.live.textStrong
            font.pixelSize: 11
            font.bold: true
        }
        background: Rectangle {
            color: Theme.live.surfaceA(0.95)
            border.color: Theme.live.ink(0.15)
            border.width: 1
            radius: 6
        }
    }

    Timer {
        id: autoHideTimer
        interval: 4000
        repeat: false
        onTriggered: {
            if (!menuContainer.isPinned) {
                menuContainer.close();
            }
        }
    }

    function open() {
        menuContainer.active = true;
        if (!menuContainer.isPinned) {
            autoHideTimer.restart();
        }
    }

    function close() {
        menuContainer.collapsed();
        menuContainer.active = false;
        autoHideTimer.stop();
    }

    // Reset auto-hide timer on mouse activity inside menu
    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        propagateComposedEvents: true
        onPositionChanged: {
            if (!menuContainer.isPinned) {
                autoHideTimer.restart();
            }
        }
    }

    // Host name text property
    property string hostName: "Desktop • Host"

    // To prevent onActivated from firing when programmatically updating the values
    property bool isInitializing: false

    function normalizeRenderBackend(renderBackend, disableCuda) {
        var value = String(renderBackend || "").toLowerCase().replace(/[- ]/g, "_");
        if (value === "software" || value === "cpu" || value === "ffmpeg" || value === "qvideosink") return "software";
        if (value === "native" || value === "native_gpu" || value === "native_render") return "native_gpu";
        if (value === "auto" || value === "auto_gpu" || value === "gpu" || value === "hardware") return "auto_gpu";
        return disableCuda ? "software" : "auto_gpu";
    }

    function renderBackendIndex(renderBackend, disableCuda) {
        var backend = normalizeRenderBackend(renderBackend, disableCuda);
        if (backend === "native_gpu") return 1;
        if (backend === "software") return 2;
        return 0;
    }

    function currentRenderBackend() {
        if (decoderComboBox.currentIndex === 1) return "native_gpu";
        if (decoderComboBox.currentIndex === 2) return "software";
        return "auto_gpu";
    }

    function initializeSettings(res, fps, codec, bitrate, queueLimit, host, disableCuda, inputProtocol, renderBackend) {
        isInitializing = true;
        
        hostName = host;

        // Set resolution combobox
        resComboBox.currentIndex = StreamOptions.resolutionIndex(res);

        // Set FPS combobox
        var fpsPresets = menuContainer.fpsPresets;
        var fpsIdx = fpsPresets.indexOf(fps);
        var fallbackIdx = fpsPresets.indexOf(60);
        if (fallbackIdx === -1) fallbackIdx = 0;
        fpsComboBox.currentIndex = fpsIdx !== -1 ? fpsIdx : fallbackIdx;

        // Set Bitrate combobox (bitrate is in kbps)
        bitrateComboBox.currentIndex = StreamOptions.nearestIndex(StreamOptions.bitrates, bitrate);

        // Set Codec combobox
        codecComboBox.currentIndex = StreamOptions.indexOfValue(StreamOptions.codecs, StreamOptions.codecPart(codec));
        chromaComboBox.currentIndex = StreamOptions.indexOfValue(StreamOptions.chromas, StreamOptions.chromaPart(codec));

        // Set Queue Limit combobox
        queueComboBox.currentIndex = StreamOptions.nearestIndex(StreamOptions.mouseQueues, queueLimit);

        decoderComboBox.currentIndex = renderBackendIndex(renderBackend, disableCuda);

        if (inputProtocol === "webtransport") {
            protocolComboBox.currentIndex = 1;
        } else {
            protocolComboBox.currentIndex = 0;
        }

        isInitializing = false;
    }

    function applyCurrentSettings() {
        if (isInitializing) return;

        var selectedRes = StreamOptions.resolutions[resComboBox.currentIndex].value;

        var selectedFps = menuContainer.fpsPresets[fpsComboBox.currentIndex];

        var selectedBitrate = StreamOptions.bitrates[bitrateComboBox.currentIndex];

        var selectedCodec = StreamOptions.composeCodec(
            StreamOptions.codecs[codecComboBox.currentIndex].value,
            StreamOptions.chromas[chromaComboBox.currentIndex].value);

        var selectedQueue = StreamOptions.mouseQueues[queueComboBox.currentIndex];

        var selectedRenderBackend = currentRenderBackend();
        var selectedDisableCuda = (selectedRenderBackend === "software");

        var protocolMap = ["webrtc", "webtransport"];
        var selectedProtocol = protocolMap[protocolComboBox.currentIndex];

        console.log("Applying menu settings: " + selectedRes + ", " + selectedFps + ", " + selectedCodec + ", " + selectedBitrate + ", " + selectedQueue + ", renderBackend=" + selectedRenderBackend + ", disableCuda=" + selectedDisableCuda + ", protocol=" + selectedProtocol);
        menuContainer.settingsChanged(selectedRes, selectedFps, selectedCodec, selectedBitrate, selectedQueue, selectedDisableCuda, selectedRenderBackend, selectedProtocol);
    }

    // Pill background
    Rectangle {
        anchors.fill: parent
        radius: 24
        color: Theme.live.surfaceA(0.9)
        border.color: Theme.live.ink(0.08)
        border.width: 1

        // Drag handle for moving the window
        DragHandler {
            target: null
            onActiveChanged: {
                if (active) {
                    menuContainer.Window.window.startSystemMove();
                }
            }
        }

        // Left Section
        Row {
            id: leftSection
            anchors.left: parent.left
            anchors.leftMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            spacing: 12

            // Back Button
            Button {
                id: backButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Exit Stream" }
                background: Rectangle {
                    anchors.fill: parent
                    color: backButton.hovered ? Theme.live.ink(0.08) : "transparent"
                    radius: 14
                }
                contentItem: Canvas {
                    id: backIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    onPaint: {
                        var ctx = backIcon.getContext("2d");
                        ctx.reset();
                        ctx.strokeStyle = Theme.live.text;
                        ctx.lineWidth = 1.8;
                        ctx.lineCap = "round";
                        ctx.lineJoin = "round";
                        ctx.beginPath();
                        ctx.moveTo(16, 9);
                        ctx.lineTo(10, 14);
                        ctx.lineTo(16, 19);
                        ctx.stroke();
                    }
                }
                onClicked: {
                    menuContainer.exitTriggered();
                }
            }
        }

        // Right Section
        Row {
            id: rightSection
            anchors.right: parent.right
            anchors.rightMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10

            // Resolution Dropdown
            LunarisComboBox {
                id: resComboBox
                customWidth: 80
                // Short labels here ("1080p", "2K"); the settings page has the full ones.
                model: StreamOptions.resolutions.map(function(r) { return r.label.split(" ")[0]; })
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Stream Resolution" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // FPS Dropdown
            LunarisComboBox {
                id: fpsComboBox
                customWidth: 95
                model: menuContainer.fpsPresets.map(function(val) { return String(val) + " FPS"; })
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Stream Frame Rate (FPS)" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Bitrate Dropdown
            LunarisComboBox {
                id: bitrateComboBox
                customWidth: 95
                model: StreamOptions.bitrates.map(function(v) { return StreamOptions.bitrateLabel(v); })
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Stream Bitrate Limit" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Codec Dropdown
            LunarisComboBox {
                id: codecComboBox
                customWidth: 80
                model: StreamOptions.codecs.map(function(c) { return c.label; })
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Video codec (Auto: H.265 when the other computer encodes it in hardware, else H.264)" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Chroma Dropdown
            LunarisComboBox {
                id: chromaComboBox
                customWidth: 72
                model: StreamOptions.chromas.map(function(c) { return c.label; })
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Chroma (4:4:4: sharp coloured text, H.264 only, decoded on the CPU)" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Decoder Dropdown
            LunarisComboBox {
                id: decoderComboBox
                customWidth: 100
                model: ["Auto GPU", "Prefer Native GPU", "Software"]
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Decode and presentation backend. Prefer Native GPU may fall back until native presenter is available." }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Mouse Queue Dropdown
            LunarisComboBox {
                id: queueComboBox
                customWidth: 80
                model: StreamOptions.mouseQueues.map(function(v) { return v + " B"; })
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Mouse Queue Limit" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Input Protocol Dropdown
            LunarisComboBox {
                id: protocolComboBox
                customWidth: 124
                model: ["WebRTC (SCTP)", "WebTransport"]
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Input Protocol" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Separator
            Rectangle {
                width: 1
                height: 18
                color: Theme.live.ink(0.12)
                anchors.verticalCenter: parent.verticalCenter
            }

            // Lock Cursor Button
            Button {
                id: lockButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: menuContainer.isPointerLocked ? "Unlock Mouse Cursor" : "Lock Mouse Cursor to Window" }
                background: Rectangle {
                    anchors.fill: parent
                    color: lockButton.hovered ? Theme.live.ink(0.12) : (menuContainer.isPointerLocked ? Theme.live.ink(0.08) : "transparent")
                    radius: 14
                }
                contentItem: Canvas {
                    id: lockIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    
                    Connections {
                        target: menuContainer
                        ignoreUnknownSignals: true
                        function onIsPointerLockedChanged() {
                            lockIcon.requestPaint();
                        }
                    }
                    
                    onPaint: {
                        var ctx = lockIcon.getContext("2d");
                        ctx.reset();
                        ctx.strokeStyle = Theme.live.text;
                        ctx.lineWidth = 1.8;
                        ctx.lineCap = "round";
                        ctx.lineJoin = "round";
                        
                        // Draw lock body
                        ctx.beginPath();
                        ctx.rect(9, 13, 10, 8);
                        ctx.stroke();
                        
                        // Draw shackle
                        ctx.beginPath();
                        if (menuContainer.isPointerLocked) {
                            // Closed shackle
                            ctx.moveTo(12, 13);
                            ctx.lineTo(12, 9);
                            ctx.arc(14, 9, 2, Math.PI, 0, false);
                            ctx.lineTo(16, 13);
                        } else {
                            // Open shackle (curves up and away)
                            ctx.moveTo(12, 13);
                            ctx.lineTo(12, 9);
                            ctx.arc(14, 9, 2, Math.PI, 0, false);
                            ctx.lineTo(16, 10);
                        }
                        ctx.stroke();
                    }
                }
                onClicked: {
                    menuContainer.lockToggled();
                    if (!menuContainer.isPinned) autoHideTimer.restart();
                }
            }

            // Local Cursor (Eye) Button
            Button {
                id: eyeButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: menuContainer.hideLocalCursor ? "Show Local Cursor" : "Hide Local Cursor" }
                background: Rectangle {
                    anchors.fill: parent
                    color: eyeButton.hovered ? Theme.live.ink(0.12) : (menuContainer.hideLocalCursor ? Theme.live.ink(0.08) : "transparent")
                    radius: 14
                }
                contentItem: Canvas {
                    id: eyeIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    
                    Connections {
                        target: menuContainer
                        ignoreUnknownSignals: true
                        function onHideLocalCursorChanged() {
                            eyeIcon.requestPaint();
                        }
                    }
                    
                    onPaint: {
                        var ctx = eyeIcon.getContext("2d");
                        ctx.reset();
                        ctx.strokeStyle = Theme.live.text;
                        ctx.lineWidth = 1.8;
                        ctx.lineCap = "round";
                        ctx.lineJoin = "round";
                        
                        // Draw eye outer path
                        ctx.beginPath();
                        ctx.moveTo(6, 14);
                        ctx.quadraticCurveTo(14, 7, 22, 14);
                        ctx.quadraticCurveTo(14, 21, 6, 14);
                        ctx.stroke();
                        
                        // Draw pupil
                        ctx.beginPath();
                        ctx.arc(14, 14, 2.2, 0, 2 * Math.PI);
                        ctx.fillStyle = Theme.live.text;
                        ctx.fill();
                        
                        // Draw slash if hidden
                        if (menuContainer.hideLocalCursor) {
                            ctx.beginPath();
                            ctx.moveTo(6, 6);
                            ctx.lineTo(22, 22);
                            ctx.stroke();
                        }
                    }
                }
                onClicked: {
                    menuContainer.cursorHideToggled();
                    if (!menuContainer.isPinned) autoHideTimer.restart();
                }
            }

            // Show Stats Button
            Button {
                id: statsButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: menuContainer.showStats ? "Hide Performance Stats" : "Show Performance Stats" }
                background: Rectangle {
                    anchors.fill: parent
                    color: statsButton.hovered ? Theme.live.ink(0.12) : (menuContainer.showStats ? Theme.live.ink(0.08) : "transparent")
                    radius: 14
                }
                contentItem: Canvas {
                    id: statsIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    
                    Connections {
                        target: menuContainer
                        ignoreUnknownSignals: true
                        function onShowStatsChanged() {
                            statsIcon.requestPaint();
                        }
                    }
                    
                    onPaint: {
                        var ctx = statsIcon.getContext("2d");
                        ctx.reset();
                        ctx.fillStyle = Theme.live.text;
                        
                        // Draw 3 vertical bars
                        ctx.fillRect(8, 16, 2.5, 6);
                        ctx.fillRect(12.75, 10, 2.5, 12);
                        ctx.fillRect(17.5, 13, 2.5, 9);
                    }
                }
                onClicked: {
                    menuContainer.statsToggled();
                    if (!menuContainer.isPinned) autoHideTimer.restart();
                }
            }

            // Files of the remote computer (file manager, own connection)
            Button {
                id: sendFileButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: I18n.t("Files on the remote computer (or drop files on the stream)") }
                background: Rectangle {
                    anchors.fill: parent
                    color: sendFileButton.hovered ? Theme.live.ink(0.12) : "transparent"
                    radius: 14
                }
                contentItem: Item {
                    Icon { anchors.centerIn: parent; name: "folder"; size: 16; color: Theme.live.text }
                }
                onClicked: {
                    menuContainer.sendFilesRequested();
                    if (!menuContainer.isPinned) autoHideTimer.restart();
                }
            }

            // Pin Menu Button
            Button {
                id: pinButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: menuContainer.isPinned ? "Unpin Menu (Auto-hide)" : "Pin Menu (Always show)" }
                background: Rectangle {
                    anchors.fill: parent
                    color: pinButton.hovered ? Theme.live.ink(0.12) : (menuContainer.isPinned ? Theme.live.ink(0.08) : "transparent")
                    radius: 14
                }
                contentItem: Canvas {
                    id: pinIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    onPaint: {
                        var ctx = pinIcon.getContext("2d");
                        ctx.reset();
                        ctx.strokeStyle = Theme.live.text;
                        ctx.lineWidth = 1.8;
                        ctx.lineCap = "round";
                        ctx.lineJoin = "round";
                        
                        // Needle
                        ctx.beginPath();
                        ctx.moveTo(14, 16);
                        ctx.lineTo(14, 22);
                        ctx.stroke();
                        
                        // Guard & Cap
                        ctx.beginPath();
                        ctx.moveTo(10, 16);
                        ctx.lineTo(18, 16);
                        ctx.moveTo(9, 11);
                        ctx.lineTo(19, 11);
                        ctx.stroke();
                        
                        // Body
                        ctx.beginPath();
                        ctx.rect(11, 11, 6, 5);
                        ctx.stroke();
                    }
                }
                onClicked: {
                    menuContainer.isPinned = !menuContainer.isPinned;
                    if (menuContainer.isPinned) {
                        autoHideTimer.stop();
                    } else {
                        autoHideTimer.restart();
                    }
                }
            }

            // Minimize Button
            Button {
                id: minimizeButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Minimize Window" }
                background: Rectangle {
                    anchors.fill: parent
                    color: minimizeButton.hovered ? Theme.live.ink(0.08) : "transparent"
                    radius: 14
                }
                contentItem: Canvas {
                    id: minimizeIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    onPaint: {
                        var ctx = minimizeIcon.getContext("2d");
                        ctx.reset();
                        ctx.strokeStyle = Theme.live.text;
                        ctx.lineWidth = 1.8;
                        ctx.lineCap = "round";
                        ctx.beginPath();
                        ctx.moveTo(9, 14);
                        ctx.lineTo(19, 14);
                        ctx.stroke();
                    }
                }
                onClicked: {
                    menuContainer.minimizeTriggered();
                }
            }

            // Restore Down / Maximize Button
            Button {
                id: restoreButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: (menuContainer.Window.window && menuContainer.Window.window.visibility === Window.FullScreen) ? "Exit Fullscreen" : "Enter Fullscreen" }
                background: Rectangle {
                    anchors.fill: parent
                    color: restoreButton.hovered ? Theme.live.ink(0.08) : "transparent"
                    radius: 14
                }
                contentItem: Canvas {
                    id: restoreIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    
                    Connections {
                        target: menuContainer.Window.window
                        ignoreUnknownSignals: true
                        function onVisibilityChanged() {
                            restoreIcon.requestPaint();
                        }
                    }
                    
                    onPaint: {
                        var ctx = restoreIcon.getContext("2d");
                        ctx.reset();
                        ctx.strokeStyle = Theme.live.text;
                        ctx.lineWidth = 1.8;
                        ctx.lineJoin = "miter";
                        
                        ctx.beginPath();
                        var isFullscreen = (menuContainer.Window.window && menuContainer.Window.window.visibility === Window.FullScreen);
                        if (isFullscreen) {
                            // Front rect: x=8, y=12, w=8, h=8
                            ctx.rect(8, 12, 8, 8);
                            // Back rect visible edges
                            ctx.moveTo(12, 12);
                            ctx.lineTo(12, 8);
                            ctx.lineTo(20, 8);
                            ctx.lineTo(20, 16);
                            ctx.lineTo(16, 16);
                        } else {
                            // Single rect: x=9, y=9, w=10, h=10
                            ctx.rect(9, 9, 10, 10);
                        }
                        ctx.stroke();
                    }
                }
                onClicked: {
                    menuContainer.windowToggleTriggered();
                }
            }

            // Collapse Button
            Button {
                id: collapseButton
                width: 28
                height: 28
                padding: 0
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Collapse Menu" }
                background: Rectangle {
                    anchors.fill: parent
                    color: collapseButton.hovered ? Theme.live.ink(0.08) : "transparent"
                    radius: 14
                }
                contentItem: Canvas {
                    id: collapseIcon
                    width: 28
                    height: 28
                    contextType: "2d"
                    onPaint: {
                        var ctx = collapseIcon.getContext("2d");
                        ctx.reset();
                        ctx.strokeStyle = Theme.live.text;
                        ctx.lineWidth = 1.8;
                        ctx.lineCap = "round";
                        ctx.lineJoin = "round";
                        ctx.beginPath();
                        ctx.moveTo(9, 17);
                        ctx.lineTo(14, 12);
                        ctx.lineTo(19, 17);
                        ctx.stroke();
                    }
                }
                onClicked: {
                    menuContainer.close();
                }
            }
        }
    }
}
