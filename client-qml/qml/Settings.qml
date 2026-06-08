import QtQuick
import QtQuick.Controls

Rectangle {
    id: settingsRoot
    width: 480
    height: 760
    radius: 16
    color: Qt.rgba(11/255, 17/255, 32/255, 0.96)
    border.color: Qt.rgba(0, 240/255, 255/255, 0.35)
    border.width: 1
    visible: false

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

    // Multi-layered glow border for a premium glow/shadow effect
    Rectangle {
        anchors.fill: parent
        anchors.margins: -4
        radius: parent.radius + 4
        color: "transparent"
        border.color: Qt.rgba(0, 240/255, 255/255, 0.1)
        border.width: 1.5
        z: -1
    }
    Rectangle {
        anchors.fill: parent
        anchors.margins: -8
        radius: parent.radius + 8
        color: "transparent"
        border.color: Qt.rgba(0, 240/255, 255/255, 0.04)
        border.width: 3
        z: -2
    }

    // Signals
    signal applySettings(string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol, string encoder, string displayId, bool virtualDisplay)

    function open() {
        settingsRoot.visible = true;
    }

    function close() {
        settingsRoot.visible = false;
    }

    function normalizeRenderBackend(renderBackend, disableCuda) {
        var value = String(renderBackend || "").toLowerCase().replace(/[- ]/g, "_");
        if (value === "software" || value === "cpu" || value === "ffmpeg" || value === "qvideosink") return "software";
        if (value === "native" || value === "native_gpu" || value === "native_render") return "native_gpu";
        if (value === "auto" || value === "auto_gpu" || value === "gpu" || value === "hardware") return "auto_gpu";
        return disableCuda ? "software" : "auto_gpu";
    }

    function backendIndex(renderBackend, disableCuda) {
        var backend = normalizeRenderBackend(renderBackend, disableCuda);
        if (backend === "native_gpu") return 1;
        if (backend === "software") return 2;
        return 0;
    }

    function currentRenderBackend() {
        if (decoderCombo.currentIndex === 1) return "native_gpu";
        if (decoderCombo.currentIndex === 2) return "software";
        return "auto_gpu";
    }

    function setCurrentSettings(res, fps, codec, bitrate, queueLimit, disableCuda, inputProtocol, encoder, displayId, virtualDisplay, renderBackend) {
        if (res.indexOf("1920") !== -1 || res.indexOf("1080") !== -1) {
            resCombo.currentIndex = 0;
        } else if (res.indexOf("1280") !== -1 || res.indexOf("720") !== -1) {
            resCombo.currentIndex = 1;
        } else if (res.indexOf("960") !== -1 || res.indexOf("540") !== -1) {
            resCombo.currentIndex = 2;
        }

        // Set FPS: check if value matches a preset, otherwise set custom
        var fpsVal = parseInt(fps);
        var presets = settingsRoot.fpsPresets;
        var foundIndex = presets.indexOf(fpsVal);
        if (foundIndex !== -1) {
            fpsCombo.currentIndex = foundIndex;
        } else {
            // Custom value: set text directly
            fpsCombo.currentIndex = -1;
        }
        fpsInput.text = String(fpsVal);

        var codecLower = codec.toLowerCase();
        if (codecLower.indexOf("264") !== -1) {
            codecCombo.currentIndex = 0;
        } else if (codecLower.indexOf("265") !== -1 || codecLower.indexOf("hevc") !== -1) {
            codecCombo.currentIndex = 1;
        } else if (codecLower.indexOf("av1") !== -1) {
            codecCombo.currentIndex = 2;
        }

        decoderCombo.currentIndex = backendIndex(renderBackend, disableCuda);

        bitrateSlider.value = bitrate;

        var qlVal = parseInt(queueLimit);
        if (qlVal === 0) queueCombo.currentIndex = 0;
        else if (qlVal === 64) queueCombo.currentIndex = 1;
        else if (qlVal === 256) queueCombo.currentIndex = 2;
        else if (qlVal === 1024) queueCombo.currentIndex = 3;
        else if (qlVal === 4096) queueCombo.currentIndex = 4;
        else if (qlVal === 16384) queueCombo.currentIndex = 5;
        else queueCombo.currentIndex = 2; // Default to 256

        if (inputProtocol === "webtransport") {
            protocolCombo.currentIndex = 1;
        } else {
            protocolCombo.currentIndex = 0;
        }

        var encoderLower = (encoder || "auto").toLowerCase();
        var encoderOptions = ["auto", "native", "ffmpeg", "nvenc", "amf", "qsv", "vaapi", "software"];
        var encoderIdx = encoderOptions.indexOf(encoderLower);
        encoderCombo.currentIndex = encoderIdx >= 0 ? encoderIdx : 0;
        displayInput.text = displayId || "default";
        virtualDisplayCheck.checked = virtualDisplay === true;
    }

    // Modal overlay blocker
    MouseArea {
        anchors.fill: parent
        propagateComposedEvents: false
    }

    // Subtle header background area highlight for premium look
    Rectangle {
        id: headerHighlight
        width: parent.width - 2
        height: 72
        anchors.top: parent.top
        anchors.topMargin: 1
        anchors.horizontalCenter: parent.horizontalCenter
        radius: 15
        color: Qt.rgba(255, 255, 255, 0.02)
        
        // Hide the rounded bottom corners of the header highlight to keep it top-only
        Rectangle {
            width: parent.width
            height: 15
            anchors.bottom: parent.bottom
            color: Qt.rgba(255, 255, 255, 0.02)
        }
        
        // A thin separator line under the header
        Rectangle {
            width: parent.width
            height: 1
            anchors.bottom: parent.bottom
            color: Qt.rgba(255, 255, 255, 0.06)
        }
    }

    // Header
    Text {
        id: headerTitle
        text: "STREAM CONFIGURATION"
        font.pixelSize: 15
        font.bold: true
        font.letterSpacing: 1.5
        color: "#00f0ff"
        anchors.top: parent.top
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 24
    }

    // Subtitle
    Text {
        id: headerSubtitle
        text: "Adjust settings to optimize latency and video quality"
        font.pixelSize: 11
        color: "#94a3b8"
        anchors.top: headerTitle.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 6
    }

    // Grid layout for fields
    Grid {
        anchors.top: headerSubtitle.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: buttonRow.top
        anchors.margins: 32
        columns: 2
        spacing: 20
        verticalItemAlignment: Grid.AlignVCenter

        // Resolution
        Text { text: "Resolution:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: resCombo
            width: 260
            model: ["1080p (1920x1080)", "720p (1280x720)", "540p (960x540)"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: resCombo.width
                contentItem: Text {
                    text: modelData
                    color: resCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: resCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: resCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: resCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: resCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: resCombo.height + 4
                width: resCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: resCombo.popup.visible ? resCombo.delegateModel : null
                    currentIndex: resCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // FPS — Editable ComboBox with presets + custom input
        Text { text: "Target FPS:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        Item {
            width: 260
            height: 38

            ComboBox {
                id: fpsCombo
                width: parent.width
                height: parent.height
                model: settingsRoot.fpsPresets.map(function(val) { return String(val); })
                currentIndex: settingsRoot.fpsPresets.indexOf(60) !== -1 ? settingsRoot.fpsPresets.indexOf(60) : 0
                editable: false

                onActivated: {
                    fpsInput.text = fpsCombo.currentText;
                }

                delegate: ItemDelegate {
                    width: fpsCombo.width
                    contentItem: Text {
                        text: modelData
                        color: fpsCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                        font.pixelSize: 13
                        verticalAlignment: Text.AlignVCenter
                    }
                    background: Rectangle {
                        color: fpsCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                    }
                    highlighted: fpsCombo.highlightedIndex === index
                }

                // Override the contentItem to show our custom editable input
                contentItem: TextInput {
                    id: fpsInput
                    leftPadding: 12
                    rightPadding: 36
                    text: "60"
                    font.pixelSize: 13
                    color: "#ffffff"
                    verticalAlignment: Text.AlignVCenter
                    selectByMouse: true
                    validator: IntValidator { bottom: 1; top: 999 }
                    inputMethodHints: Qt.ImhDigitsOnly

                    // Update combobox selection when user types a preset value
                    onTextChanged: {
                        var val = parseInt(text);
                        var presets = settingsRoot.fpsPresets;
                        var idx = presets.indexOf(val);
                        if (idx !== -1 && fpsCombo.currentIndex !== idx) {
                            fpsCombo.currentIndex = idx;
                        }
                    }
                }

                background: Rectangle {
                    implicitHeight: 38
                    color: "#0f1626"
                    border.color: fpsInput.activeFocus || fpsCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                    border.width: 1
                    radius: 8
                }

                popup: Popup {
                    y: fpsCombo.height + 4
                    width: fpsCombo.width
                    implicitHeight: contentItem.implicitHeight
                    padding: 1

                    contentItem: ListView {
                        clip: true
                        implicitHeight: contentHeight
                        model: fpsCombo.popup.visible ? fpsCombo.delegateModel : null
                        currentIndex: fpsCombo.highlightedIndex
                    }

                    background: Rectangle {
                        color: "#0f1626"
                        border.color: Qt.rgba(1, 1, 1, 0.15)
                        border.width: 1
                        radius: 8
                    }
                }
            }
        }

        // Codec
        Text { text: "Video Codec:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: codecCombo
            width: 260
            model: ["H264", "H265", "AV1"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: codecCombo.width
                contentItem: Text {
                    text: modelData
                    color: codecCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: codecCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: codecCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: codecCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: codecCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: codecCombo.height + 4
                width: codecCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: codecCombo.popup.visible ? codecCombo.delegateModel : null
                    currentIndex: codecCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Encoder Backend
        Text { text: "Encoder Backend:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: encoderCombo
            width: 260
            model: ["Auto", "Native", "FFmpeg", "NVENC", "AMF", "QSV", "VAAPI", "Software"]
            currentIndex: 0
            delegate: ItemDelegate {
                width: encoderCombo.width
                contentItem: Text { text: modelData; color: encoderCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"; font.pixelSize: 13; verticalAlignment: Text.AlignVCenter }
                background: Rectangle { color: encoderCombo.highlightedIndex === index ? "#4f46e5" : "transparent" }
                highlighted: encoderCombo.highlightedIndex === index
            }
            contentItem: Text { leftPadding: 12; text: encoderCombo.displayText; font.pixelSize: 13; color: "#ffffff"; verticalAlignment: Text.AlignVCenter }
            background: Rectangle { implicitHeight: 38; color: "#0f1626"; border.color: encoderCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08); border.width: 1; radius: 8 }
            popup: Popup {
                y: encoderCombo.height + 4; width: encoderCombo.width; implicitHeight: contentItem.implicitHeight; padding: 1
                contentItem: ListView { clip: true; implicitHeight: contentHeight; model: encoderCombo.popup.visible ? encoderCombo.delegateModel : null; currentIndex: encoderCombo.highlightedIndex }
                background: Rectangle { color: "#0f1626"; border.color: Qt.rgba(1, 1, 1, 0.15); border.width: 1; radius: 8 }
            }
        }

        // Display ID
        Text { text: "Display:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        TextField {
            id: displayInput
            width: 260
            height: 38
            text: "default"
            placeholderText: "default, 0, HDMI-1, DP-1..."
            color: "#ffffff"
            placeholderTextColor: "#64748b"
            font.pixelSize: 13
            leftPadding: 12
            background: Rectangle { color: "#0f1626"; border.color: displayInput.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08); border.width: 1; radius: 8 }
        }

        // Virtual Display
        Text { text: "Virtual Display:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        CheckBox {
            id: virtualDisplayCheck
            width: 260
            text: "Create virtual display"
            checked: false
            contentItem: Text { text: virtualDisplayCheck.text; color: "#cbd5e1"; font.pixelSize: 13; verticalAlignment: Text.AlignVCenter; leftPadding: virtualDisplayCheck.indicator.width + 8 }
        }

        // Decoder Type
        Text { text: "Render Backend:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: decoderCombo
            width: 260
            model: ["Auto GPU", "Native GPU", "Software"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: decoderCombo.width
                contentItem: Text {
                    text: modelData
                    color: decoderCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: decoderCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: decoderCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: decoderCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: decoderCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: decoderCombo.height + 4
                width: decoderCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: decoderCombo.popup.visible ? decoderCombo.delegateModel : null
                    currentIndex: decoderCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Bitrate — Slider
        Text { text: "Bitrate:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        Column {
            width: 260
            spacing: 6

            Slider {
                id: bitrateSlider
                width: parent.width
                from: 1000
                to: 150000
                stepSize: 500
                value: 8000

                background: Rectangle {
                    x: bitrateSlider.leftPadding
                    y: bitrateSlider.topPadding + bitrateSlider.availableHeight / 2 - height / 2
                    implicitWidth: 260
                    implicitHeight: 4
                    width: bitrateSlider.availableWidth
                    height: implicitHeight
                    radius: 2
                    color: Qt.rgba(1, 1, 1, 0.08)

                    Rectangle {
                        width: bitrateSlider.visualPosition * parent.width
                        height: parent.height
                        color: "#00f0ff"
                        radius: 2
                    }
                }

                handle: Rectangle {
                    x: bitrateSlider.leftPadding + bitrateSlider.visualPosition * (bitrateSlider.availableWidth - width)
                    y: bitrateSlider.topPadding + bitrateSlider.availableHeight / 2 - height / 2
                    implicitWidth: 16
                    implicitHeight: 16
                    radius: 8
                    color: bitrateSlider.pressed ? "#00d8e8" : "#00f0ff"
                    border.color: Qt.rgba(0, 0, 0, 0.3)
                    border.width: 1
                }
            }

            // Bitrate value label row
            Item {
                width: parent.width
                height: 16

                Text {
                    text: {
                        var val = bitrateSlider.value;
                        if (val >= 1000) {
                            var mbps = val / 1000;
                            return mbps % 1 === 0 ? mbps.toFixed(0) + " Mbps" : mbps.toFixed(1) + " Mbps";
                        }
                        return val + " Kbps";
                    }
                    color: "#00f0ff"
                    font.pixelSize: 12
                    font.bold: true
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                }

                Text {
                    text: "(" + bitrateSlider.value + " Kbps)"
                    color: "#64748b"
                    font.pixelSize: 11
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                }
            }
        }

        // Mouse Queue Limit
        Text { text: "Mouse Queue:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: queueCombo
            width: 260
            model: [
                "0 B (Strict No Queue)",
                "64 B (Ultra Low Buffer)",
                "256 B (Recommended)",
                "1024 B (Moderate Buffer)",
                "4096 B (High Buffer)",
                "16384 B (Previous Default)"
            ]
            currentIndex: 2

            delegate: ItemDelegate {
                width: queueCombo.width
                contentItem: Text {
                    text: modelData
                    color: queueCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: queueCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: queueCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: queueCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: queueCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: queueCombo.height + 4
                width: queueCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: queueCombo.popup.visible ? queueCombo.delegateModel : null
                    currentIndex: queueCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Input Protocol
        Text { text: "Input Protocol:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: protocolCombo
            width: 260
            model: ["WebRTC Data Channel (SCTP)", "WebTransport (QUIC Datagrams)"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: protocolCombo.width
                contentItem: Text {
                    text: modelData
                    color: protocolCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: protocolCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: protocolCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: protocolCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: protocolCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: protocolCombo.height + 4
                width: protocolCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: protocolCombo.popup.visible ? protocolCombo.delegateModel : null
                    currentIndex: protocolCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }
    }

    // Button Row
    Row {
        id: buttonRow
        anchors.bottom: parent.bottom
        anchors.right: parent.right
        anchors.margins: 28
        spacing: 12

        Button {
            id: cancelButton
            text: "Cancel"
            onClicked: {
                settingsRoot.close();
            }
            background: Rectangle {
                color: cancelButton.hovered ? Qt.rgba(1, 1, 1, 0.05) : "transparent"
                border.color: cancelButton.hovered ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.2)
                border.width: 1
                radius: 8
                implicitWidth: 100
                implicitHeight: 36
            }
            contentItem: Text {
                text: cancelButton.text
                color: cancelButton.hovered ? "#00f0ff" : "#cbd5e1"
                font.bold: true
                font.pixelSize: 13
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }

        Button {
            id: applyButton
            text: "Save & Apply"
            onClicked: {
                var selectedRes = "1920x1080"
                if (resCombo.currentIndex === 1) selectedRes = "1280x720"
                else if (resCombo.currentIndex === 2) selectedRes = "960x540"

                var fps = parseInt(fpsInput.text) || 60
                if (fps < 1) fps = 1;
                if (fps > 999) fps = 999;

                var codec = codecCombo.currentText.toLowerCase()
                var bitrate = Math.round(bitrateSlider.value)

                var queueLimit = 256
                if (queueCombo.currentIndex === 0) queueLimit = 0
                else if (queueCombo.currentIndex === 1) queueLimit = 64
                else if (queueCombo.currentIndex === 2) queueLimit = 256
                else if (queueCombo.currentIndex === 3) queueLimit = 1024
                else if (queueCombo.currentIndex === 4) queueLimit = 4096
                else if (queueCombo.currentIndex === 5) queueLimit = 16384

                var renderBackend = settingsRoot.currentRenderBackend();
                var disableCuda = (renderBackend === "software");
                var inputProtocol = (protocolCombo.currentIndex === 1) ? "webtransport" : "webrtc"
                var encoderOptions = ["auto", "native", "ffmpeg", "nvenc", "amf", "qsv", "vaapi", "software"]
                var encoder = encoderOptions[encoderCombo.currentIndex] || "auto"
                var displayId = displayInput.text.trim().length > 0 ? displayInput.text.trim() : "default"
                var virtualDisplay = virtualDisplayCheck.checked

                settingsRoot.applySettings(selectedRes, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol, encoder, displayId, virtualDisplay);
                settingsRoot.close();
            }
            background: Rectangle {
                color: applyButton.hovered ? "#00e0ff" : "#00f0ff"
                radius: 8
                implicitWidth: 120
                implicitHeight: 36
            }
            contentItem: Text {
                text: applyButton.text
                color: "#080c14"
                font.bold: true
                font.pixelSize: 13
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }
    }
}
