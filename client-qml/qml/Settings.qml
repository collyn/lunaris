import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Stream settings for one computer, shown over the computer list: a
// category list on the left, sections of "name + explanation | control"
// rows on the right (like the app's Settings), Cancel / Save at the bottom.
// The same choices as the web client (StreamOptions).
Rectangle {
    id: settingsRoot
    color: Theme.bg
    visible: false

    // Name of the computer being configured (header).
    property string hostName: ""
    // "video" | "host" | "decoding" | "input"
    property string category: "video"

    // Lists from the host (updateCapabilities).
    property var availableEncoders: ["Auto", "Native", "FFmpeg", "NVENC", "AMF", "QSV", "VAAPI", "Software"]
    property var availableDisplayIds: []
    property var availableDisplayLabels: []

    // The settings being edited.
    property string res: "1920x1080"
    property int fps: 60
    property string codec: "auto"
    property string chroma: "420"
    property int bitrate: 15000
    property int queueLimit: 256
    property string inputProtocol: "webrtc"
    property string encoder: "auto"
    property string displayId: "default"
    property bool virtualDisplay: false
    property string renderBackend: "auto_gpu"
    property int virtualScreenHz: 144

    signal applySettings(string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol, string encoder, string displayId, bool virtualDisplay)
    // Enable/disable the host's forced high-refresh "virtual screen" (e.g. DP-2).
    signal configureVirtualDisplay(bool enable, int refreshHz)

    readonly property var categories: [
        { key: "video", label: "Video", icon: "monitor" },
        { key: "display", label: "Display", icon: "grid" },
        { key: "audio", label: "Audio", icon: "volume" },
        { key: "sharing", label: "Clipboard & files", icon: "clipboard" },
        { key: "host", label: "Remote computer", icon: "server" },
        { key: "input", label: "Input", icon: "link" }
    ]

    Keys.onEscapePressed: settingsRoot.close()
    onVisibleChanged: if (visible) forceActiveFocus()

    function open() { settingsRoot.category = "video"; settingsRoot.visible = true; }
    function close() { settingsRoot.visible = false; }

    function normalizeRenderBackend(renderBackend, disableCuda) {
        var value = String(renderBackend || "").toLowerCase().replace(/[- ]/g, "_");
        if (value === "software" || value === "cpu" || value === "ffmpeg" || value === "qvideosink") return "software";
        if (value === "native" || value === "native_gpu" || value === "native_render") return "native_gpu";
        if (value === "auto" || value === "auto_gpu" || value === "gpu" || value === "hardware") return "auto_gpu";
        return disableCuda ? "software" : "auto_gpu";
    }

    function currentRenderBackend() { return settingsRoot.renderBackend; }

    function updateCapabilities(displaysJson, encodersJson) {
        try {
            var displays = JSON.parse(displaysJson);
            var encoders = JSON.parse(encodersJson);
            var merged = ["auto", "native", "ffmpeg", "software"];
            for (var i = 0; i < encoders.length; i++) {
                var enc = encoders[i].toLowerCase();
                if (merged.indexOf(enc) === -1) merged.push(enc);
            }
            availableEncoders = merged.map(function (e) { return e.charAt(0).toUpperCase() + e.slice(1); });

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
        settingsRoot.res = StreamOptions.resolutions[StreamOptions.resolutionIndex(res)].value;
        settingsRoot.fps = parseInt(fps) || 60;
        settingsRoot.codec = StreamOptions.codecPart(codec);
        settingsRoot.chroma = StreamOptions.chromaPart(codec);
        settingsRoot.bitrate = bitrate;
        settingsRoot.queueLimit = StreamOptions.mouseQueues[StreamOptions.nearestIndex(StreamOptions.mouseQueues, parseInt(queueLimit))];
        settingsRoot.inputProtocol = inputProtocol === "webtransport" ? "webtransport" : "webrtc";
        settingsRoot.encoder = (encoder || "auto").toLowerCase();
        settingsRoot.displayId = displayId || "default";
        settingsRoot.virtualDisplay = virtualDisplay === true;
        settingsRoot.renderBackend = normalizeRenderBackend(renderBackend, disableCuda);
    }

    function save() {
        settingsRoot.applySettings(
            settingsRoot.res, settingsRoot.fps,
            StreamOptions.composeCodec(settingsRoot.codec, settingsRoot.chroma),
            settingsRoot.bitrate, settingsRoot.queueLimit,
            settingsRoot.renderBackend === "software", settingsRoot.renderBackend,
            settingsRoot.inputProtocol, settingsRoot.encoder, settingsRoot.displayId,
            settingsRoot.virtualDisplay);
        settingsRoot.close();
    }

    // Swallow clicks so the list underneath doesn't get them.
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (wheel) => wheel.accepted = true }

    // ---------------- building blocks ----------------
    component SectionTitle: Text {
        Layout.topMargin: 6
        color: Theme.textStrong
        font.pixelSize: 16
        font.weight: Font.DemiBold
    }

    // A card of rows separated by thin lines.
    component Card: Rectangle {
        default property alias rows: rowsColumn.data
        Layout.fillWidth: true
        implicitHeight: rowsColumn.implicitHeight
        radius: Theme.radius
        color: Theme.surface
        border.color: Theme.border
        ColumnLayout {
            id: rowsColumn
            anchors.left: parent.left
            anchors.right: parent.right
            spacing: 0
        }
    }

    // "Name + explanation" on the left, the control on the right.
    component SettingRow: Item {
        id: settingRow
        property string title: ""
        property string hint: ""
        property bool divider: true
        default property alias control: controlSlot.data
        Layout.fillWidth: true
        implicitHeight: Math.max(rowText.implicitHeight, controlSlot.childrenRect.height) + 36
        Rectangle {
            visible: settingRow.divider
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.leftMargin: 20
            anchors.rightMargin: 20
            height: 1
            color: Theme.border
        }
        ColumnLayout {
            id: rowText
            anchors.left: parent.left
            anchors.right: controlSlot.left
            anchors.leftMargin: 20
            anchors.rightMargin: 24
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4
            Text { text: settingRow.title; color: Theme.textStrong; font.pixelSize: 14; font.weight: Font.Medium; Layout.fillWidth: true; wrapMode: Text.WordWrap }
            Text { visible: settingRow.hint !== ""; text: settingRow.hint; color: Theme.textMuted; font.pixelSize: 12; Layout.fillWidth: true; wrapMode: Text.WordWrap; lineHeight: 1.15 }
        }
        Item {
            id: controlSlot
            anchors.right: parent.right
            anchors.rightMargin: 20
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(260, settingRow.width * 0.42)
            height: childrenRect.height
        }
    }

    // ---------------- header ----------------
    Rectangle {
        id: header
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: 56
        color: Theme.surface
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 1; color: Theme.border }

        Row {
            anchors.left: parent.left
            anchors.leftMargin: 22
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10
            Text { text: I18n.t("Stream settings"); color: Theme.textMuted; font.pixelSize: 17; font.weight: Font.DemiBold; anchors.verticalCenter: parent.verticalCenter }
            Icon { name: "arrowRight"; size: 14; color: Theme.textSubtle; anchors.verticalCenter: parent.verticalCenter }
            Text {
                text: I18n.t(settingsRoot.categories.filter(function (c) { return c.key === settingsRoot.category; })[0].label)
                color: Theme.textStrong
                font.pixelSize: 17
                font.weight: Font.DemiBold
                anchors.verticalCenter: parent.verticalCenter
            }
            Rectangle {
                visible: settingsRoot.hostName !== ""
                anchors.verticalCenter: parent.verticalCenter
                implicitWidth: hostLabel.implicitWidth + 18
                implicitHeight: 24
                radius: 12
                color: Theme.surface2
                border.color: Theme.border
                Text { id: hostLabel; anchors.centerIn: parent; text: settingsRoot.hostName; color: Theme.textMuted; font.pixelSize: 12 }
            }
        }
        IconButton {
            anchors.right: parent.right
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            iconName: "close"
            tip: I18n.t("Close (Esc)")
            onClicked: settingsRoot.close()
        }
    }

    // ---------------- categories ----------------
    Rectangle {
        id: nav
        anchors.left: parent.left
        anchors.top: header.bottom
        anchors.bottom: footer.top
        width: 210
        color: Theme.surface
        Rectangle { anchors.right: parent.right; anchors.top: parent.top; anchors.bottom: parent.bottom; width: 1; color: Theme.border }

        Column {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 12
            spacing: 4
            Repeater {
                model: settingsRoot.categories
                delegate: AbstractButton {
                    id: navBtn
                    readonly property bool current: settingsRoot.category === modelData.key
                    width: parent.width
                    implicitHeight: 40
                    hoverEnabled: true
                    onClicked: { settingsRoot.category = modelData.key; contentFlick.contentY = 0; }
                    Accessible.name: I18n.t(modelData.label)
                    background: Rectangle {
                        radius: Theme.radiusSm
                        color: navBtn.current ? Theme.surface2 : (navBtn.hovered ? Theme.ink(0.04) : "transparent")
                        Rectangle { visible: navBtn.current; width: 3; height: 22; radius: 1.5; color: Theme.accent; anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter }
                    }
                    contentItem: Row {
                        leftPadding: 14
                        spacing: 12
                        Icon { name: modelData.icon; size: 17; color: navBtn.current ? Theme.textStrong : Theme.textMuted; anchors.verticalCenter: parent.verticalCenter }
                        Text { text: I18n.t(modelData.label); color: navBtn.current ? Theme.textStrong : Theme.text; font.pixelSize: 14; font.weight: navBtn.current ? Font.DemiBold : Font.Normal; anchors.verticalCenter: parent.verticalCenter }
                    }
                }
            }
        }
    }

    // ---------------- sections ----------------
    Flickable {
        id: contentFlick
        anchors.left: nav.right
        anchors.right: parent.right
        anchors.top: header.bottom
        anchors.bottom: footer.top
        contentHeight: sections.implicitHeight + 48
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        ColumnLayout {
            id: sections
            x: 28
            y: 24
            width: Math.min(760, contentFlick.width - 56)
            spacing: 12

            // ===== Video =====
            SectionTitle { visible: settingsRoot.category === "video"; text: I18n.t("Picture") }
            Card {
                visible: settingsRoot.category === "video"
                SettingRow {
                    divider: false
                    title: I18n.t("Resolution")
                    hint: I18n.t("Size of the picture the remote computer sends. Default 1080p (FHD).")
                    ThemedCombo {
                        width: parent.width
                        options: StreamOptions.resolutions.map(function (r) { return { value: r.value, label: r.label + "  (" + r.value.replace("x", " × ") + ")" }; })
                        value: settingsRoot.res
                        onPicked: (v) => settingsRoot.res = v
                    }
                }
                SettingRow {
                    title: I18n.t("Frame rate")
                    hint: I18n.t("Frames per second sent by the remote computer. Above its screen's refresh rate needs a virtual screen (Remote computer).")
                    ThemedCombo {
                        width: parent.width
                        options: StreamOptions.fps.map(function (f) { return { value: f, label: f + " FPS" }; })
                        value: settingsRoot.fps
                        onPicked: (v) => settingsRoot.fps = v
                    }
                }
                SettingRow {
                    title: I18n.t("Bitrate")
                    hint: I18n.t("Higher looks sharper but needs a faster network.")
                    ThemedCombo {
                        width: parent.width
                        options: StreamOptions.bitrates.map(function (b) { return { value: b, label: StreamOptions.bitrateLabel(b) }; })
                        value: StreamOptions.bitrates[StreamOptions.nearestIndex(StreamOptions.bitrates, settingsRoot.bitrate)]
                        onPicked: (v) => settingsRoot.bitrate = v
                    }
                }
            }

            SectionTitle { visible: settingsRoot.category === "video"; text: I18n.t("Compression") }
            Card {
                visible: settingsRoot.category === "video"
                SettingRow {
                    divider: false
                    title: I18n.t("Codec")
                    hint: I18n.t("Auto: H.265 when the remote computer encodes it in hardware (lighter and sharper at the same bitrate), otherwise H.264.")
                    ThemedCombo {
                        width: parent.width
                        options: StreamOptions.codecs
                        value: settingsRoot.codec
                        onPicked: (v) => settingsRoot.codec = v
                    }
                }
                SettingRow {
                    title: I18n.t("Chroma")
                    hint: I18n.t("4:4:4 keeps coloured text sharp. H.264 only, encoded on NVIDIA or the CPU and decoded on the CPU here.")
                    ThemedCombo {
                        width: parent.width
                        options: StreamOptions.chromas
                        value: settingsRoot.chroma
                        onPicked: (v) => settingsRoot.chroma = v
                    }
                }
            }

            // ===== Video: decoding =====
            SectionTitle { visible: settingsRoot.category === "video"; text: I18n.t("Decoding") }
            Card {
                visible: settingsRoot.category === "video"
                SettingRow {
                    divider: false
                    title: I18n.t("Decoder")
                    hint: I18n.t("GPU for the lowest latency (NVDEC, VAAPI, D3D11). Choose Software only if the picture stays black or breaks up.")
                    ThemedCombo {
                        width: parent.width
                        options: [{ value: "auto_gpu", label: I18n.t("Auto (GPU)") }, { value: "native_gpu", label: I18n.t("Prefer native GPU") }, { value: "software", label: I18n.t("Software") }]
                        value: settingsRoot.renderBackend
                        onPicked: (v) => settingsRoot.renderBackend = v
                    }
                }
            }

            // ===== Display (this device, applied right away) =====
            SectionTitle { visible: settingsRoot.category === "display"; text: I18n.t("Picture on screen") }
            Card {
                visible: settingsRoot.category === "display"
                SettingRow {
                    divider: false
                    title: I18n.t("Display mode")
                    hint: I18n.t("How the remote screen fits this window. Applies right away to every computer.")
                    ThemedCombo {
                        width: parent.width
                        options: ViewPrefs.displayModes
                        value: ViewPrefs.displayMode
                        onPicked: (v) => ViewPrefs.displayMode = v
                    }
                }
                SettingRow {
                    title: I18n.t("Statistics overlay")
                    hint: I18n.t("Frame rate, bitrate, latency and the codec in a corner of the stream.")
                    Toggle {
                        anchors.right: parent.right
                        checked: typeof window !== "undefined" && window.showStats
                        onToggled: if (typeof window !== "undefined") window.showStats = checked
                        Accessible.name: I18n.t("Statistics overlay")
                    }
                }
            }

            // ===== Audio (this device, applied right away) =====
            SectionTitle { visible: settingsRoot.category === "audio"; text: I18n.t("Sound") }
            Card {
                visible: settingsRoot.category === "audio"
                SettingRow {
                    divider: false
                    title: I18n.t("Play sound")
                    hint: I18n.t("The remote computer's sound plays on this device. Applies right away.")
                    Toggle {
                        anchors.right: parent.right
                        checked: ViewPrefs.audioEnabled
                        onToggled: ViewPrefs.audioEnabled = checked
                        Accessible.name: I18n.t("Play sound")
                    }
                }
                SettingRow {
                    title: I18n.t("Volume")
                    hint: ViewPrefs.volume + "%"
                    Slider {
                        width: parent.width
                        from: 0
                        to: 100
                        stepSize: 1
                        enabled: ViewPrefs.audioEnabled
                        value: ViewPrefs.volume
                        onMoved: ViewPrefs.volume = Math.round(value)
                        Accessible.name: I18n.t("Volume")
                    }
                }
            }

            // ===== Clipboard & files (this device, applied right away) =====
            SectionTitle { visible: settingsRoot.category === "sharing"; text: I18n.t("Clipboard") }
            Card {
                visible: settingsRoot.category === "sharing"
                SettingRow {
                    divider: false
                    title: I18n.t("Sync clipboard")
                    hint: I18n.t("Text you copy here can be pasted on the remote computer, and the other way round. The remote computer can turn this off for itself.")
                    Toggle {
                        anchors.right: parent.right
                        checked: ViewPrefs.clipboardSync
                        onToggled: ViewPrefs.clipboardSync = checked
                        Accessible.name: I18n.t("Sync clipboard")
                    }
                }
            }
            SectionTitle { visible: settingsRoot.category === "sharing"; text: I18n.t("Files") }
            Card {
                visible: settingsRoot.category === "sharing"
                SettingRow {
                    divider: false
                    title: I18n.t("Send files")
                    hint: I18n.t("Drop files on the stream or use the send button in the menu; they are saved in Downloads/Lunaris on the remote computer.")
                    Toggle {
                        anchors.right: parent.right
                        checked: ViewPrefs.fileTransfer
                        onToggled: ViewPrefs.fileTransfer = checked
                        Accessible.name: I18n.t("Send files")
                    }
                }
            }

            // ===== Remote computer =====
            SectionTitle { visible: settingsRoot.category === "host"; text: I18n.t("Encoding") }
            Card {
                visible: settingsRoot.category === "host"
                SettingRow {
                    divider: false
                    title: I18n.t("Encoder")
                    hint: I18n.t("Auto picks the remote computer's hardware encoder (NVENC, VAAPI, QSV, AMF) and falls back to the CPU.")
                    ThemedCombo {
                        width: parent.width
                        options: settingsRoot.availableEncoders.map(function (e) { return { value: e.toLowerCase(), label: e }; })
                        value: settingsRoot.encoder
                        onPicked: (v) => settingsRoot.encoder = v
                    }
                }
            }

            SectionTitle { visible: settingsRoot.category === "host"; text: I18n.t("Screen") }
            Card {
                visible: settingsRoot.category === "host"
                SettingRow {
                    divider: false
                    title: I18n.t("Display")
                    hint: I18n.t("Which screen of the remote computer to show.")
                    ThemedCombo {
                        width: parent.width
                        options: settingsRoot.availableDisplayIds.length > 0
                            ? settingsRoot.availableDisplayIds.map(function (id, i) { return { value: id, label: settingsRoot.availableDisplayLabels[i] }; })
                            : [{ value: "default", label: I18n.t("Default") }]
                        value: settingsRoot.displayId
                        onPicked: (v) => settingsRoot.displayId = v
                    }
                }
                SettingRow {
                    title: I18n.t("Virtual display")
                    hint: I18n.t("Create a separate screen on the remote computer for this session (Windows hosts with the virtual display driver).")
                    Toggle {
                        anchors.right: parent.right
                        checked: settingsRoot.virtualDisplay
                        onToggled: settingsRoot.virtualDisplay = checked
                    }
                }
                SettingRow {
                    title: I18n.t("High-refresh virtual screen")
                    hint: I18n.t("Linux hosts: turn on a forced high-refresh output so frame rates above the real screen are possible. Applies right away.")
                    RowLayout {
                        width: parent.width
                        spacing: 8
                        ThemedCombo {
                            Layout.preferredWidth: 96
                            implicitWidth: 96
                            options: [240, 144, 120, 60].map(function (h) { return { value: h, label: h + " Hz" }; })
                            value: settingsRoot.virtualScreenHz
                            onPicked: (v) => settingsRoot.virtualScreenHz = v
                        }
                        SecondaryButton { text: I18n.t("On"); implicitHeight: 40; onClicked: settingsRoot.configureVirtualDisplay(true, settingsRoot.virtualScreenHz) }
                        SecondaryButton { text: I18n.t("Off"); implicitHeight: 40; onClicked: settingsRoot.configureVirtualDisplay(false, settingsRoot.virtualScreenHz) }
                    }
                }
            }

            // ===== Input =====
            SectionTitle { visible: settingsRoot.category === "input"; text: I18n.t("Mouse and keyboard") }
            Card {
                visible: settingsRoot.category === "input"
                SettingRow {
                    divider: false
                    title: I18n.t("Input protocol")
                    hint: I18n.t("WebTransport sends mouse and keyboard over QUIC datagrams, which lowers latency on lossy networks; WebRTC is used when it can't connect.")
                    ThemedCombo {
                        width: parent.width
                        options: [{ value: "webrtc", label: I18n.t("WebRTC data channels") }, { value: "webtransport", label: I18n.t("WebTransport") }]
                        value: settingsRoot.inputProtocol
                        onPicked: (v) => settingsRoot.inputProtocol = v
                    }
                }
                SettingRow {
                    title: I18n.t("Mouse queue limit")
                    hint: I18n.t("How much mouse movement may wait to be sent. Smaller drops old moves for responsiveness; 256 B suits most networks.")
                    ThemedCombo {
                        width: parent.width
                        options: StreamOptions.mouseQueues.map(function (q) { return { value: q, label: q + " B" + (q === 256 ? " (recommended)" : "") }; })
                        value: settingsRoot.queueLimit
                        onPicked: (v) => settingsRoot.queueLimit = v
                    }
                }
            }
        }
    }

    // ---------------- footer ----------------
    Rectangle {
        id: footer
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 64
        color: Theme.surface
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; height: 1; color: Theme.border }
        Text {
            anchors.left: parent.left
            anchors.leftMargin: 22
            anchors.verticalCenter: parent.verticalCenter
            text: I18n.t("Used when you connect to %1.").arg(settingsRoot.hostName || I18n.t("this computer"))
            color: Theme.textSubtle
            font.pixelSize: 12
        }
        Row {
            anchors.right: parent.right
            anchors.rightMargin: 20
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10
            SecondaryButton { text: I18n.t("Cancel"); onClicked: settingsRoot.close() }
            PrimaryButton { text: I18n.t("Save"); onClicked: settingsRoot.save() }
        }
    }
}
