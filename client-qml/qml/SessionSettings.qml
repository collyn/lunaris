import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// This device's settings for the stream, opened from the session menu
// (LunarisMenuBar's gear): sound, clipboard and files, controllers (with
// the tester), system shortcuts, the picture (display mode, statistics,
// decoder). They apply right away and are kept in ViewPrefs (main.qml saves
// them); the decoder is the menu bar's, which restarts the stream.
Popup {
    id: panel
    objectName: "sessionSettings"

    // main.qml: whether the statistics overlay is shown.
    property bool showStats: false
    // "auto_gpu" | "native_gpu" | "software"
    property string renderBackend: "auto_gpu"
    // How this window system gives a window the system's shortcuts
    // (main.qml systemKeysKind): "window" cannot (Windows, macOS).
    property string systemKeysKind: "grab"
    // The controllers' live test is shown.
    property bool testerShown: false

    signal statsToggled(bool on)
    signal renderBackendPicked(string backend)

    width: Math.min(440, parent ? parent.width - 32 : 440)
    height: Math.min(content.implicitHeight + 32, parent ? parent.height - 96 : 640)
    x: parent ? parent.width - width - 16 : 0
    y: 72
    padding: 0
    modal: false
    dim: false
    focus: true
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
    onClosed: panel.testerShown = false

    background: Rectangle {
        radius: Theme.radiusLg
        color: Theme.surface
        border.color: Theme.borderStrong
        // The stream's cursor rules (a blank or host-shaped pointer) end here.
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.NoButton
            hoverEnabled: true
            cursorShape: Qt.ArrowCursor
        }
    }

    // ---------------- building blocks ----------------
    component SectionTitle: Text {
        Layout.topMargin: 6
        color: Theme.textMuted
        font.pixelSize: 12
        font.weight: Font.DemiBold
        font.capitalization: Font.AllUppercase
    }

    // "Name + explanation" on the left, the control on the right.
    component SettingRow: RowLayout {
        id: row
        property string title: ""
        property string hint: ""
        default property alias control: controlSlot.data
        Layout.fillWidth: true
        spacing: 12
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Text { textFormat: Text.PlainText; text: row.title; color: Theme.textStrong; font.pixelSize: 13; font.weight: Font.Medium; Layout.fillWidth: true; wrapMode: Text.WordWrap }
            Text { textFormat: Text.PlainText; visible: row.hint !== ""; text: row.hint; color: Theme.textMuted; font.pixelSize: 11; Layout.fillWidth: true; wrapMode: Text.WordWrap }
        }
        Item {
            id: controlSlot
            Layout.alignment: Qt.AlignVCenter
            implicitWidth: childrenRect.width
            implicitHeight: Math.max(childrenRect.height, 26)
        }
    }

    contentItem: Flickable {
        id: flick
        clip: true
        contentHeight: content.implicitHeight + 32
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        ColumnLayout {
            id: content
            x: 18
            y: 16
            width: flick.width - 36
            spacing: 12

            RowLayout {
                Layout.fillWidth: true
                Text {
                    Layout.fillWidth: true
                    text: I18n.t("Session settings")
                    color: Theme.textStrong
                    font.pixelSize: 16
                    font.weight: Font.DemiBold
                }
                IconButton {
                    iconName: "close"
                    tip: I18n.t("Close (Esc)")
                    onClicked: panel.close()
                }
            }

            // ===== Sound =====
            SectionTitle { text: I18n.t("Sound") }
            SettingRow {
                title: I18n.t("Play sound")
                hint: I18n.t("The remote computer's sound plays on this device.")
                Toggle {
                    objectName: "audioToggle"
                    checked: ViewPrefs.audioEnabled
                    onToggled: ViewPrefs.audioEnabled = checked
                    Accessible.name: I18n.t("Play sound")
                }
            }
            SettingRow {
                title: I18n.t("Volume")
                hint: ViewPrefs.volume + "%"
                Slider {
                    objectName: "volumeSlider"
                    width: 150
                    from: 0
                    to: 100
                    stepSize: 1
                    enabled: ViewPrefs.audioEnabled
                    value: ViewPrefs.volume
                    onMoved: ViewPrefs.volume = Math.round(value)
                    Accessible.name: I18n.t("Volume")
                }
            }
            SettingRow {
                title: I18n.t("Remote computer's microphone")
                hint: I18n.t("Also send what its microphone hears, mixed with its sound. Applies when you next connect.")
                Toggle {
                    objectName: "microphoneToggle"
                    checked: ViewPrefs.recordMicrophone
                    onToggled: ViewPrefs.recordMicrophone = checked
                    Accessible.name: I18n.t("Remote computer's microphone")
                }
            }

            // ===== Clipboard and files =====
            SectionTitle { text: I18n.t("Clipboard & files") }
            SettingRow {
                title: I18n.t("Sync clipboard")
                hint: I18n.t("Text you copy here can be pasted on the remote computer, and the other way round.")
                Toggle {
                    objectName: "clipboardToggle"
                    checked: ViewPrefs.clipboardSync
                    onToggled: ViewPrefs.clipboardSync = checked
                    Accessible.name: I18n.t("Sync clipboard")
                }
            }
            SettingRow {
                title: I18n.t("File transfer")
                hint: I18n.t("The Files button of this menu, and files dropped on the stream (they go to its Downloads/Lunaris).")
                Toggle {
                    objectName: "fileTransferToggle"
                    checked: ViewPrefs.fileTransfer
                    onToggled: ViewPrefs.fileTransfer = checked
                    Accessible.name: I18n.t("File transfer")
                }
            }

            // ===== Controllers =====
            SectionTitle { text: I18n.t("Game controllers") }
            SettingRow {
                title: I18n.t("Use controllers in sessions")
                hint: I18n.t("Controllers connected to this device act as Xbox controllers on the remote computer. Windows computers need the ViGEmBus driver.")
                Toggle {
                    objectName: "gamepadToggle"
                    checked: ViewPrefs.gamepad
                    onToggled: ViewPrefs.gamepad = checked
                    Accessible.name: I18n.t("Use controllers in sessions")
                }
            }
            SecondaryButton {
                objectName: "testControllersButton"
                text: panel.testerShown ? I18n.t("Hide the controller test") : I18n.t("Test controllers")
                onClicked: panel.testerShown = !panel.testerShown
            }
            Loader {
                Layout.fillWidth: true
                active: panel.testerShown && panel.visible
                visible: active
                sourceComponent: GamepadTester { objectName: "gamepadTester"; source: bridge }
            }

            // ===== Keyboard =====
            SectionTitle { visible: panel.systemKeysKind !== "window"; text: I18n.t("Keyboard") }
            SettingRow {
                visible: panel.systemKeysKind !== "window"
                title: I18n.t("System shortcuts")
                hint: I18n.t("Alt+Tab, the Windows or Super key and others go to the remote computer. Ctrl+Alt+Esc gives them back to this computer.")
                    + (panel.systemKeysKind === "no" ? " " + I18n.t("This desktop does not allow it.") : "")
                ThemedCombo {
                    objectName: "systemKeysCombo"
                    implicitWidth: 190
                    implicitHeight: 34
                    options: [
                        { value: "always", label: I18n.t("While the pointer is over the picture") },
                        { value: "fullscreen", label: I18n.t("In full screen only") },
                        { value: "off", label: I18n.t("Never") }
                    ]
                    value: ViewPrefs.systemKeys
                    onPicked: (v) => ViewPrefs.systemKeys = v
                }
            }

            // ===== Picture =====
            SectionTitle { text: I18n.t("Picture") }
            SettingRow {
                title: I18n.t("Display mode")
                hint: I18n.t("How the remote screen fits this window.")
                ThemedCombo {
                    objectName: "displayModeCombo"
                    implicitWidth: 190
                    implicitHeight: 34
                    options: ViewPrefs.displayModes
                    value: ViewPrefs.displayMode
                    onPicked: (v) => ViewPrefs.displayMode = v
                }
            }
            SettingRow {
                title: I18n.t("Statistics overlay")
                hint: I18n.t("Frame rate, bitrate, latency and the codec in a corner of the stream.")
                Toggle {
                    objectName: "statsToggle"
                    checked: panel.showStats
                    onToggled: panel.statsToggled(checked)
                    Accessible.name: I18n.t("Statistics overlay")
                }
            }
            SettingRow {
                title: I18n.t("Decoder")
                hint: I18n.t("GPU for the lowest latency (NVDEC, VAAPI, D3D11). Choose Software only if the picture stays black or breaks up. Restarts the stream.")
                ThemedCombo {
                    objectName: "decoderCombo"
                    implicitWidth: 190
                    implicitHeight: 34
                    options: [{ value: "auto_gpu", label: I18n.t("Auto (GPU)") }, { value: "native_gpu", label: I18n.t("Prefer native GPU") }, { value: "software", label: I18n.t("Software") }]
                    value: panel.renderBackend
                    onPicked: (v) => { if (v !== panel.renderBackend) panel.renderBackendPicked(v); }
                }
            }
        }
    }
}
