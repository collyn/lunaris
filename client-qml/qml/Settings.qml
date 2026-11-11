import QtQuick
import QtQuick.Controls

// Stream settings for one computer, shown as a page over the computer list
// (the fields scroll, in two columns when wide; Cancel / Save stay at the
// bottom).
Rectangle {
    id: settingsRoot
    color: Theme.bg
    visible: false
    // Name of the computer being configured (page title).
    property string hostName: ""

    // Esc goes back like the Back button.
    Keys.onEscapePressed: settingsRoot.close()
    onVisibleChanged: if (visible) forceActiveFocus()

    // Same choices as the web client (StreamOptions).
    property var fpsPresets: StreamOptions.fps
    property var availableEncoders: ["Auto", "Native", "FFmpeg", "NVENC", "AMF", "QSV", "VAAPI", "Software"]
    property var availableDisplayIds: []
    property var availableDisplayLabels: []

    // Signals
    signal applySettings(string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol, string encoder, string displayId, bool virtualDisplay)
    // Enable/disable the host's forced high-refresh "virtual screen" (e.g. DP-2).
    signal configureVirtualDisplay(bool enable, int refreshHz)

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

    function updateCapabilities(displaysJson, encodersJson) {
        try {
            var displays = JSON.parse(displaysJson);
            var encoders = JSON.parse(encodersJson);

            // Update encoder list: merge agent-discovered with defaults
            var defaultEncoders = ["auto", "native", "ffmpeg", "software"];
            var merged = defaultEncoders.slice();
            for (var i = 0; i < encoders.length; i++) {
                var enc = encoders[i].toLowerCase();
                if (merged.indexOf(enc) === -1) {
                    merged.push(enc);
                }
            }
            availableEncoders = merged.map(function(e) {
                return e.charAt(0).toUpperCase() + e.slice(1);
            });

            // Update display list
            var ids = ["default"];
            var labels = ["Default"];
            for (var j = 0; j < displays.length; j++) {
                var d = displays[j];
                ids.push(d.id);
                var label = d.name + " (" + d.width + "x" + d.height + " @ " + Math.round(d.refresh_rate) + "Hz)";
                if (d.is_primary) label += " ★";
                labels.push(label);
            }
            availableDisplayIds = ids;
            availableDisplayLabels = labels;
        } catch (e) {
            console.log("updateCapabilities: failed to parse JSON:", e);
        }
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

        codecCombo.currentIndex = StreamOptions.indexOfValue(StreamOptions.codecs, StreamOptions.codecPart(codec));
        chromaCombo.currentIndex = StreamOptions.indexOfValue(StreamOptions.chromas, StreamOptions.chromaPart(codec));

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
        var encoderOptions = settingsRoot.availableEncoders.map(function(e) { return e.toLowerCase(); });
        var encoderIdx = encoderOptions.indexOf(encoderLower);
        encoderCombo.currentIndex = encoderIdx >= 0 ? encoderIdx : 0;

        // Set display combo from displayId
        var displayIdStr = displayId || "default";
        var displayIds = settingsRoot.availableDisplayIds.length > 0 ? settingsRoot.availableDisplayIds : ["default"];
        var displayIdx = displayIds.indexOf(displayIdStr);
        displayCombo.currentIndex = displayIdx >= 0 ? displayIdx : 0;

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
        width: parent.width
        height: 72
        anchors.top: parent.top
        color: Theme.surface

        // Back to the computer list
        Button {
            id: backButton
            anchors.left: parent.left
            anchors.leftMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            hoverEnabled: true
            onClicked: settingsRoot.close()
            Accessible.name: "Back"
            contentItem: Text {
                text: "←  Back"
                color: backButton.hovered ? Theme.textStrong : Theme.text
                font.pixelSize: 13
                font.weight: Font.Medium
                verticalAlignment: Text.AlignVCenter
                horizontalAlignment: Text.AlignHCenter
            }
            background: Rectangle {
                implicitWidth: 84
                implicitHeight: 34
                radius: Theme.radiusSm
                color: backButton.hovered ? Theme.surface3 : "transparent"
                border.color: Theme.border
            }
        }
        
        // A thin separator line under the header
        Rectangle {
            width: parent.width
            height: 1
            anchors.bottom: parent.bottom
            color: Theme.ink(0.06)
        }
    }

    // Header
    Text {
        id: headerTitle
        text: settingsRoot.hostName !== "" ? "Stream settings — " + settingsRoot.hostName : "Stream settings"
        font.pixelSize: 17
        font.weight: Font.DemiBold
        color: Theme.textStrong
        anchors.top: parent.top
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 24
    }

    // Subtitle
    Text {
        id: headerSubtitle
        text: "Adjust settings to optimize latency and video quality"
        font.pixelSize: 11
        color: Theme.textMuted
        anchors.top: headerTitle.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 6
    }

    // Fields, scrolling when the window is short
    Flickable {
        id: fieldsFlick
        anchors.top: headerHighlight.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: footer.top
        contentWidth: width
        contentHeight: fieldsGrid.height + 56
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

    Grid {
        id: fieldsGrid
        // Two label/field pairs per row when there is room.
        readonly property bool wide: fieldsFlick.width >= 940
        y: 28
        x: Math.max(24, (fieldsFlick.width - width) / 2)
        width: wide ? 880 : 420
        columns: wide ? 4 : 2
        rowSpacing: 12
        columnSpacing: wide ? 36 : 20
        verticalItemAlignment: Grid.AlignVCenter

        // Resolution
        Text { text: "Resolution:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        ComboBox {
            id: resCombo
            width: 260
            model: ["1080p (1920x1080)", "720p (1280x720)", "540p (960x540)"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: resCombo.width
                contentItem: Text {
                    text: modelData
                    color: resCombo.highlightedIndex === index ? Theme.textStrong : Theme.text
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: resCombo.highlightedIndex === index ? Theme.accent : "transparent"
                }
                highlighted: resCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: resCombo.displayText
                font.pixelSize: 13
                color: Theme.textStrong
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: Theme.surface
                border.color: resCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08)
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
                    color: Theme.surface
                    border.color: Theme.ink(0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // FPS — Editable ComboBox with presets + custom input
        Text { text: "Target FPS:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
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
                        color: fpsCombo.highlightedIndex === index ? Theme.textStrong : Theme.text
                        font.pixelSize: 13
                        verticalAlignment: Text.AlignVCenter
                    }
                    background: Rectangle {
                        color: fpsCombo.highlightedIndex === index ? Theme.accent : "transparent"
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
                    color: Theme.textStrong
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
                    color: Theme.surface
                    border.color: fpsInput.activeFocus || fpsCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08)
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
                        color: Theme.surface
                        border.color: Theme.ink(0.15)
                        border.width: 1
                        radius: 8
                    }
                }
            }
        }

        // Codec
        Text { text: "Video Codec:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        ComboBox {
            id: codecCombo
            width: 260
            // Auto: H.265 when the other computer encodes it in hardware, else H.264.
            model: StreamOptions.codecs.map(function(c) { return c.label; })
            currentIndex: 0

            delegate: ItemDelegate {
                width: codecCombo.width
                contentItem: Text {
                    text: modelData
                    color: codecCombo.highlightedIndex === index ? Theme.textStrong : Theme.text
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: codecCombo.highlightedIndex === index ? Theme.accent : "transparent"
                }
                highlighted: codecCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: codecCombo.displayText
                font.pixelSize: 13
                color: Theme.textStrong
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: Theme.surface
                border.color: codecCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08)
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
                    color: Theme.surface
                    border.color: Theme.ink(0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Chroma
        Text { text: "Chroma:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        ComboBox {
            id: chromaCombo
            width: 260
            // 4:4:4: sharp coloured text; H.264 only (host NVENC or CPU
            // encoder; decoded on the CPU here).
            model: StreamOptions.chromas.map(function(c) { return c.label; })
            currentIndex: 0

            delegate: ItemDelegate {
                width: chromaCombo.width
                contentItem: Text {
                    text: modelData
                    color: chromaCombo.highlightedIndex === index ? Theme.textStrong : Theme.text
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: chromaCombo.highlightedIndex === index ? Theme.accent : "transparent"
                }
                highlighted: chromaCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: chromaCombo.displayText
                font.pixelSize: 13
                color: Theme.textStrong
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: Theme.surface
                border.color: chromaCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: chromaCombo.height + 4
                width: chromaCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: chromaCombo.popup.visible ? chromaCombo.delegateModel : null
                    currentIndex: chromaCombo.highlightedIndex
                }

                background: Rectangle {
                    color: Theme.surface
                    border.color: Theme.ink(0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Encoder Backend
        Text { text: "Encoder Backend:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        ComboBox {
            id: encoderCombo
            width: 260
            model: settingsRoot.availableEncoders
            currentIndex: 0
            delegate: ItemDelegate {
                width: encoderCombo.width
                contentItem: Text { text: modelData; color: encoderCombo.highlightedIndex === index ? Theme.textStrong : Theme.text; font.pixelSize: 13; verticalAlignment: Text.AlignVCenter }
                background: Rectangle { color: encoderCombo.highlightedIndex === index ? Theme.accent : "transparent" }
                highlighted: encoderCombo.highlightedIndex === index
            }
            contentItem: Text { leftPadding: 12; text: encoderCombo.displayText; font.pixelSize: 13; color: Theme.textStrong; verticalAlignment: Text.AlignVCenter }
            background: Rectangle { implicitHeight: 38; color: Theme.surface; border.color: encoderCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08); border.width: 1; radius: 8 }
            popup: Popup {
                y: encoderCombo.height + 4; width: encoderCombo.width; implicitHeight: contentItem.implicitHeight; padding: 1
                contentItem: ListView { clip: true; implicitHeight: contentHeight; model: encoderCombo.popup.visible ? encoderCombo.delegateModel : null; currentIndex: encoderCombo.highlightedIndex }
                background: Rectangle { color: Theme.surface; border.color: Theme.ink(0.15); border.width: 1; radius: 8 }
            }
        }

        // Display ID
        Text { text: "Display:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        ComboBox {
            id: displayCombo
            width: 260
            model: settingsRoot.availableDisplayLabels.length > 0 ? settingsRoot.availableDisplayLabels : ["Default"]
            currentIndex: 0
            delegate: ItemDelegate {
                width: displayCombo.width
                contentItem: Text { text: modelData; color: displayCombo.highlightedIndex === index ? Theme.textStrong : Theme.text; font.pixelSize: 13; verticalAlignment: Text.AlignVCenter }
                background: Rectangle { color: displayCombo.highlightedIndex === index ? Theme.accent : "transparent" }
                highlighted: displayCombo.highlightedIndex === index
            }
            contentItem: Text { leftPadding: 12; text: displayCombo.displayText; font.pixelSize: 13; color: Theme.textStrong; verticalAlignment: Text.AlignVCenter }
            background: Rectangle { implicitHeight: 38; color: Theme.surface; border.color: displayCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08); border.width: 1; radius: 8 }
            popup: Popup {
                y: displayCombo.height + 4; width: displayCombo.width; implicitHeight: contentItem.implicitHeight; padding: 1
                contentItem: ListView { clip: true; implicitHeight: contentHeight; model: displayCombo.popup.visible ? displayCombo.delegateModel : null; currentIndex: displayCombo.highlightedIndex }
                background: Rectangle { color: Theme.surface; border.color: Theme.ink(0.15); border.width: 1; radius: 8 }
            }
        }

        // Virtual Display
        Text { text: "Virtual Display:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        CheckBox {
            id: virtualDisplayCheck
            width: 260
            text: "Create virtual display"
            checked: false
            contentItem: Text { text: virtualDisplayCheck.text; color: Theme.text; font.pixelSize: 13; verticalAlignment: Text.AlignVCenter; leftPadding: virtualDisplayCheck.indicator.width + 8 }
        }

        // Virtual Screen (host high-refresh output) — enable/disable + refresh
        Text { text: "Virtual Screen:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        Row {
            width: 260
            spacing: 8
            LunarisComboBox {
                id: vdRefreshCombo
                live: false
                customWidth: 84
                implicitHeight: 32
                model: ["240", "144", "120", "60"]
                currentIndex: 1
            }
            Repeater {
                model: [{ label: "Enable", enable: true }, { label: "Disable", enable: false }]
                Button {
                    id: vdBtn
                    required property var modelData
                    onClicked: settingsRoot.configureVirtualDisplay(modelData.enable, parseInt(vdRefreshCombo.currentText))
                    background: Rectangle {
                        implicitWidth: 76; implicitHeight: 32; radius: Theme.radiusSm
                        color: vdBtn.hovered ? Theme.surface2 : "transparent"
                        border.color: Theme.borderStrong; border.width: 1
                    }
                    contentItem: Text {
                        text: vdBtn.modelData.label
                        color: Theme.text
                        font.pixelSize: 12
                        font.weight: Font.Medium
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }
                }
            }
        }

        // Decoder Type
        Text { text: "Render Backend:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        ComboBox {
            id: decoderCombo
            width: 260
            model: ["Auto GPU", "Prefer Native GPU", "Software"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: decoderCombo.width
                contentItem: Text {
                    text: modelData
                    color: decoderCombo.highlightedIndex === index ? Theme.textStrong : Theme.text
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: decoderCombo.highlightedIndex === index ? Theme.accent : "transparent"
                }
                highlighted: decoderCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: decoderCombo.displayText
                font.pixelSize: 13
                color: Theme.textStrong
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: Theme.surface
                border.color: decoderCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08)
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
                    color: Theme.surface
                    border.color: Theme.ink(0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Bitrate — Slider
        Text { text: "Bitrate:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
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
                    color: Theme.ink(0.08)

                    Rectangle {
                        width: bitrateSlider.visualPosition * parent.width
                        height: parent.height
                        color: Theme.accent
                        radius: 2
                    }
                }

                handle: Rectangle {
                    x: bitrateSlider.leftPadding + bitrateSlider.visualPosition * (bitrateSlider.availableWidth - width)
                    y: bitrateSlider.topPadding + bitrateSlider.availableHeight / 2 - height / 2
                    implicitWidth: 16
                    implicitHeight: 16
                    radius: 8
                    color: bitrateSlider.pressed ? Theme.accent : Theme.accent
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
                    color: Theme.accentFg
                    font.pixelSize: 12
                    font.bold: true
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                }

                Text {
                    text: "(" + bitrateSlider.value + " Kbps)"
                    color: Theme.textSubtle
                    font.pixelSize: 11
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                }
            }
        }

        // Mouse Queue Limit
        Text { text: "Mouse Queue:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
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
                    color: queueCombo.highlightedIndex === index ? Theme.textStrong : Theme.text
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: queueCombo.highlightedIndex === index ? Theme.accent : "transparent"
                }
                highlighted: queueCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: queueCombo.displayText
                font.pixelSize: 13
                color: Theme.textStrong
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: Theme.surface
                border.color: queueCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08)
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
                    color: Theme.surface
                    border.color: Theme.ink(0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Input Protocol
        Text { text: "Input Protocol:"; color: Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; width: 120 }
        ComboBox {
            id: protocolCombo
            width: 260
            model: ["WebRTC Data Channel (SCTP)", "WebTransport (QUIC Datagrams)"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: protocolCombo.width
                contentItem: Text {
                    text: modelData
                    color: protocolCombo.highlightedIndex === index ? Theme.textStrong : Theme.text
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: protocolCombo.highlightedIndex === index ? Theme.accent : "transparent"
                }
                highlighted: protocolCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: protocolCombo.displayText
                font.pixelSize: 13
                color: Theme.textStrong
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: Theme.surface
                border.color: protocolCombo.activeFocus ? Theme.accentFg : Theme.ink(0.08)
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
                    color: Theme.surface
                    border.color: Theme.ink(0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }
    }

    }

    // Footer: Cancel / Save, always visible
    Rectangle {
        id: footer
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 72
        color: Theme.surface
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; height: 1; color: Theme.ink(0.06) }
    }

    Row {
        id: buttonRow
        anchors.verticalCenter: footer.verticalCenter
        anchors.right: parent.right
        anchors.rightMargin: Math.max(24, (settingsRoot.width - fieldsGrid.width) / 2 - 20)
        spacing: 12

        Button {
            id: cancelButton
            text: "Cancel"
            onClicked: {
                settingsRoot.close();
            }
            background: Rectangle {
                color: cancelButton.hovered ? Theme.surface2 : "transparent"
                border.color: Theme.borderStrong
                border.width: 1
                radius: Theme.radiusSm
                implicitWidth: 100
                implicitHeight: 36
            }
            contentItem: Text {
                text: cancelButton.text
                color: cancelButton.hovered ? Theme.textStrong : Theme.text
                font.weight: Font.DemiBold
                font.pixelSize: 13
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }

        Button {
            id: applyButton
            text: "Save & apply"
            onClicked: {
                var selectedRes = "1920x1080"
                if (resCombo.currentIndex === 1) selectedRes = "1280x720"
                else if (resCombo.currentIndex === 2) selectedRes = "960x540"

                var fps = parseInt(fpsInput.text) || 60
                if (fps < 1) fps = 1;
                if (fps > 999) fps = 999;

                var codec = StreamOptions.composeCodec(
                    (StreamOptions.codecs[codecCombo.currentIndex] || StreamOptions.codecs[0]).value,
                    (StreamOptions.chromas[chromaCombo.currentIndex] || StreamOptions.chromas[0]).value)
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
                var encoderOptions = settingsRoot.availableEncoders.map(function(e) { return e.toLowerCase(); })
                var encoder = encoderOptions[encoderCombo.currentIndex] || "auto"
                var displayIds = settingsRoot.availableDisplayIds.length > 0 ? settingsRoot.availableDisplayIds : ["default"]
                var displayId = displayIds[displayCombo.currentIndex] || "default"
                var virtualDisplay = virtualDisplayCheck.checked

                settingsRoot.applySettings(selectedRes, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol, encoder, displayId, virtualDisplay);
                settingsRoot.close();
            }
            background: Rectangle {
                color: applyButton.hovered ? Theme.accentHover : Theme.accent
                radius: Theme.radiusSm
                implicitWidth: 120
                implicitHeight: 36
            }
            contentItem: Text {
                text: applyButton.text
                color: Theme.textOnAccent
                font.weight: Font.DemiBold
                font.pixelSize: 13
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }
    }
}
