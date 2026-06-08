import QtQuick
import QtQuick.Controls

Item {
    id: menuContainer
    width: 1080
    height: 48

    property var fpsPresets: getSupportedFpsPresets()

    function getSupportedFpsPresets() {
        var maxRate = (Screen && Screen.refreshRate > 0) ? Math.ceil(Screen.refreshRate) : 60;
        var allPresets = [240, 144, 120, 90, 60, 30];
        var filtered = [];
        
        var insertedExact = false;
        for (var i = 0; i < allPresets.length; i++) {
            var preset = allPresets[i];
            if (preset <= maxRate) {
                if (!insertedExact && Math.abs(maxRate - preset) > 5 && maxRate > preset) {
                    filtered.push(maxRate);
                    insertedExact = true;
                }
                filtered.push(preset);
            }
        }
        
        if (!insertedExact && maxRate > 30 && filtered.indexOf(maxRate) === -1) {
            var inserted = false;
            for (var j = 0; j < filtered.length; j++) {
                if (maxRate > filtered[j]) {
                    filtered.splice(j, 0, maxRate);
                    inserted = true;
                    break;
                }
            }
            if (!inserted) {
                filtered.push(maxRate);
            }
        }
        
        if (filtered.indexOf(60) === -1) filtered.push(60);
        if (filtered.indexOf(30) === -1) filtered.push(30);
        
        filtered = filtered.filter(function(item, pos) {
            return filtered.indexOf(item) === pos;
        });
        filtered.sort(function(a, b) { return b - a; });
        
        return filtered;
    }
    
    // Custom signals
    signal fullscreenToggled()
    signal lockToggled()
    signal statsToggled()
    signal cursorHideToggled()
    signal settingsChanged(string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string inputProtocol)
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
            color: "#ffffff"
            font.pixelSize: 11
            font.bold: true
        }
        background: Rectangle {
            color: Qt.rgba(24/255, 24/255, 27/255, 0.95)
            border.color: Qt.rgba(255/255, 255/255, 255/255, 0.15)
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

    function initializeSettings(res, fps, codec, bitrate, queueLimit, host, disableCuda, inputProtocol) {
        isInitializing = true;
        
        hostName = host;

        // Set resolution combobox
        if (res.indexOf("1920") !== -1) {
            resComboBox.currentIndex = 0;
        } else if (res.indexOf("1280") !== -1) {
            resComboBox.currentIndex = 1;
        } else if (res.indexOf("960") !== -1) {
            resComboBox.currentIndex = 2;
        }

        // Set FPS combobox
        var fpsPresets = menuContainer.fpsPresets;
        var fpsIdx = fpsPresets.indexOf(fps);
        var fallbackIdx = fpsPresets.indexOf(60);
        if (fallbackIdx === -1) fallbackIdx = 0;
        fpsComboBox.currentIndex = fpsIdx !== -1 ? fpsIdx : fallbackIdx;

        // Set Bitrate combobox (bitrate is in kbps)
        var bitratePresets = [2000, 5000, 8000, 10000, 15000, 20000, 30000, 50000, 75000, 100000, 150000];
        var brIdx = -1;
        var closestDiff = 999999;
        for (var i = 0; i < bitratePresets.length; i++) {
            var diff = Math.abs(bitratePresets[i] - bitrate);
            if (diff < closestDiff) {
                closestDiff = diff;
                brIdx = i;
            }
        }
        bitrateComboBox.currentIndex = brIdx !== -1 ? brIdx : 2; // Default to 8 Mbps

        // Set Codec combobox
        var c = codec.toLowerCase();
        if (c === "h264") {
            codecComboBox.currentIndex = 0;
        } else if (c === "h265" || c === "hevc") {
            codecComboBox.currentIndex = 1;
        } else if (c === "av1") {
            codecComboBox.currentIndex = 2;
        }

        // Set Queue Limit combobox
        if (queueLimit <= 128) {
            queueComboBox.currentIndex = 0;
        } else if (queueLimit <= 256) {
            queueComboBox.currentIndex = 1;
        } else if (queueLimit <= 512) {
            queueComboBox.currentIndex = 2;
        } else {
            queueComboBox.currentIndex = 3;
        }

        decoderComboBox.currentIndex = (disableCuda === true) ? 2 : 0;

        if (inputProtocol === "webtransport") {
            protocolComboBox.currentIndex = 1;
        } else {
            protocolComboBox.currentIndex = 0;
        }

        isInitializing = false;
    }

    function applyCurrentSettings() {
        if (isInitializing) return;

        var resMap = ["1920x1080", "1280x720", "960x540"];
        var selectedRes = resMap[resComboBox.currentIndex];

        var selectedFps = menuContainer.fpsPresets[fpsComboBox.currentIndex];

        var bitrateMap = [2000, 5000, 8000, 10000, 15000, 20000, 30000, 50000, 75000, 100000, 150000];
        var selectedBitrate = bitrateMap[bitrateComboBox.currentIndex];

        var codecMap = ["h264", "h265", "av1"];
        var selectedCodec = codecMap[codecComboBox.currentIndex];

        var queueMap = [128, 256, 512, 1024];
        var selectedQueue = queueMap[queueComboBox.currentIndex];

        var selectedDisableCuda = (decoderComboBox.currentIndex === 2);

        var protocolMap = ["webrtc", "webtransport"];
        var selectedProtocol = protocolMap[protocolComboBox.currentIndex];

        console.log("Applying menu settings: " + selectedRes + ", " + selectedFps + ", " + selectedCodec + ", " + selectedBitrate + ", " + selectedQueue + ", disableCuda=" + selectedDisableCuda + ", protocol=" + selectedProtocol);
        menuContainer.settingsChanged(selectedRes, selectedFps, selectedCodec, selectedBitrate, selectedQueue, selectedDisableCuda, selectedProtocol);
    }

    // Pill background
    Rectangle {
        anchors.fill: parent
        radius: 24
        color: Qt.rgba(20/255, 20/255, 20/255, 0.9)
        border.color: Qt.rgba(255/255, 255/255, 255/255, 0.08)
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
                    color: backButton.hovered ? Qt.rgba(1, 1, 1, 0.08) : "transparent"
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
                        ctx.strokeStyle = "#f1f5f9";
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
                customWidth: 85
                model: ["1080p", "720p", "540p"]
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
                model: ["2 Mbps", "5 Mbps", "8 Mbps", "10 Mbps", "15 Mbps", "20 Mbps", "30 Mbps", "50 Mbps", "75 Mbps", "100 Mbps", "150 Mbps"]
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Stream Bitrate Limit" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Codec Dropdown
            LunarisComboBox {
                id: codecComboBox
                customWidth: 80
                model: ["H264", "H265", "AV1"]
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Video Decoder Codec" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Decoder Dropdown
            LunarisComboBox {
                id: decoderComboBox
                customWidth: 110
                model: ["Auto GPU", "Native GPU", "Software"]
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Decode and presentation backend" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Mouse Queue Dropdown
            LunarisComboBox {
                id: queueComboBox
                customWidth: 80
                model: ["128 B", "256 B", "512 B", "1024 B"]
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Mouse Queue Limit" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Input Protocol Dropdown
            LunarisComboBox {
                id: protocolComboBox
                customWidth: 125
                model: ["WebRTC (SCTP)", "WebTransport"]
                anchors.verticalCenter: parent.verticalCenter
                LunarisToolTip { text: "Input Protocol" }
                onActivated: menuContainer.applyCurrentSettings()
            }

            // Separator
            Rectangle {
                width: 1
                height: 18
                color: Qt.rgba(1, 1, 1, 0.12)
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
                    color: lockButton.hovered ? Qt.rgba(1, 1, 1, 0.12) : (menuContainer.isPointerLocked ? Qt.rgba(1, 1, 1, 0.08) : "transparent")
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
                        ctx.strokeStyle = "#f1f5f9";
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
                    color: eyeButton.hovered ? Qt.rgba(1, 1, 1, 0.12) : (menuContainer.hideLocalCursor ? Qt.rgba(1, 1, 1, 0.08) : "transparent")
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
                        ctx.strokeStyle = "#f1f5f9";
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
                        ctx.fillStyle = "#f1f5f9";
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
                    color: statsButton.hovered ? Qt.rgba(1, 1, 1, 0.12) : (menuContainer.showStats ? Qt.rgba(1, 1, 1, 0.08) : "transparent")
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
                        ctx.fillStyle = "#f1f5f9";
                        
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
                    color: pinButton.hovered ? Qt.rgba(1, 1, 1, 0.12) : (menuContainer.isPinned ? Qt.rgba(1, 1, 1, 0.08) : "transparent")
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
                        ctx.strokeStyle = "#f1f5f9";
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
                    color: minimizeButton.hovered ? Qt.rgba(1, 1, 1, 0.08) : "transparent"
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
                        ctx.strokeStyle = "#f1f5f9";
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
                    color: restoreButton.hovered ? Qt.rgba(1, 1, 1, 0.08) : "transparent"
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
                        ctx.strokeStyle = "#f1f5f9";
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
                    color: collapseButton.hovered ? Qt.rgba(1, 1, 1, 0.08) : "transparent"
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
                        ctx.strokeStyle = "#f1f5f9";
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
