import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The window shown when the app is started without a link. The app only
// shows streams: computers are chosen, added and managed on the web page of
// a Lunaris server, whose Connect button opens them here (lunaris://
// links). This lists the trusted servers (their links connect without
// asking) to open their web page, lets one be added, and has the app's
// language, theme, updates and version.
Rectangle {
    id: launcher
    objectName: "launcher"
    color: Theme.bg

    // The updater's state (main.qml).
    property var updateStatus: ({})
    // [{ server, web }] from the bridge.
    property var servers: []
    property string addError: ""
    // A message for the user (a link that could not be used, a stream that
    // ended badly); "" = none.
    property string message: ""
    // The app's settings store (app_settings): the language and the theme;
    // other keys of earlier versions are kept as they are.
    property var prefs: ({})
    // Opens a web page in the browser (the tests replace it).
    property var openUrl: function (url) { Qt.openUrlExternally(url); }

    function refresh() {
        try { launcher.servers = JSON.parse(bridge.trustedServers()); } catch (e) { launcher.servers = []; }
    }
    function loadPrefs() {
        var stored = bridge.loadStore("app_settings");
        var p = {};
        if (stored !== "") {
            try { p = JSON.parse(stored) || {}; } catch (e) { console.log("app_settings: " + e); p = {}; }
        }
        launcher.prefs = p;
        Theme.mode = ["light", "dark"].indexOf(p.theme) !== -1 ? p.theme : "system";
        I18n.lang = p.language === "vi" ? "vi" : "en";
    }
    function setPref(key, value) {
        var next = Object.assign({}, launcher.prefs);
        next[key] = value;
        launcher.prefs = next;
        bridge.saveStore("app_settings", JSON.stringify(next));
        if (key === "theme") Theme.mode = value;
        if (key === "language") I18n.lang = value;
    }
    function addServer(address) {
        var web = bridge.addTrustedServer(address);
        if (web === "") {
            launcher.addError = I18n.t("That is not the address of a web page.");
            return false;
        }
        launcher.addError = "";
        refresh();
        launcher.openUrl(web);
        return true;
    }
    function removeServer(server) {
        bridge.removeTrustedServer(server);
        refresh();
    }
    function showMessage(text) { launcher.message = text; }

    Component.onCompleted: { loadPrefs(); refresh(); }
    onVisibleChanged: if (visible) refresh()

    // ---------------- building blocks ----------------
    component Card: Rectangle {
        id: card
        default property alias content: cardBody.data
        property string title: ""
        Layout.fillWidth: true
        implicitHeight: cardBody.y + cardBody.implicitHeight + 18
        radius: Theme.radiusLg
        color: Theme.surface
        border.color: Theme.border
        Text {
            id: cardTitle
            visible: card.title !== ""
            x: 20; y: 16
            text: card.title
            color: Theme.textStrong
            font.pixelSize: 15
            font.weight: Font.DemiBold
        }
        ColumnLayout {
            id: cardBody
            x: 20
            y: card.title !== "" ? 46 : 18
            width: parent.width - 40
            spacing: 10
        }
    }

    component Segmented: Flow {
        id: seg
        property var options: []      // [{ value, label }]
        property var value
        signal picked(var value)
        spacing: 0
        Repeater {
            model: seg.options
            delegate: AbstractButton {
                id: segBtn
                readonly property bool active: String(seg.value) === String(modelData.value)
                hoverEnabled: true
                implicitHeight: 30
                implicitWidth: segText.implicitWidth + 22
                Accessible.name: I18n.t(modelData.label)
                onClicked: seg.picked(modelData.value)
                background: Rectangle {
                    color: segBtn.active ? Theme.accentA(0.18) : (segBtn.hovered ? Theme.surface2 : Theme.bg)
                    border.color: segBtn.active ? Theme.accent : Theme.borderStrong
                }
                contentItem: Text {
                    id: segText
                    text: I18n.t(modelData.label)
                    color: segBtn.active ? Theme.accentFg : Theme.text
                    font.pixelSize: 12
                    font.weight: segBtn.active ? Font.DemiBold : Font.Normal
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
        }
    }

    Flickable {
        id: flick
        anchors.fill: parent
        contentHeight: column.implicitHeight + 64
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        ColumnLayout {
            id: column
            width: Math.min(640, flick.width - 48)
            x: (flick.width - width) / 2
            y: 32
            spacing: 16

            RowLayout {
                spacing: 12
                Image { source: "qrc:/icon.png"; sourceSize.width: 40; sourceSize.height: 40; Layout.preferredWidth: 40; Layout.preferredHeight: 40 }
                Text { text: "Lunaris"; color: Theme.textStrong; font.pixelSize: 24; font.weight: Font.DemiBold }
            }

            Card {
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    Text {
                        Layout.fillWidth: true
                        text: I18n.t("Lunaris opens from the web page of your Lunaris server")
                        color: Theme.textStrong
                        font.pixelSize: 17
                        font.weight: Font.DemiBold
                        wrapMode: Text.WordWrap
                    }
                    Text {
                        Layout.fillWidth: true
                        text: I18n.t("Sign in there and click Connect on a computer: its picture opens in this app, with the lowest latency. Computers, users and this computer's remote control are managed on the web page and in the Lunaris agent.")
                        color: Theme.textMuted
                        font.pixelSize: 13
                        wrapMode: Text.WordWrap
                    }
                }
                Text {
                    objectName: "launcherMessage"
                    visible: launcher.message !== ""
                    Layout.fillWidth: true
                    textFormat: Text.PlainText
                    text: launcher.message
                    color: Theme.danger
                    font.pixelSize: 13
                    wrapMode: Text.WordWrap
                }
            }

            Card {
                title: I18n.t("Your servers")
                Text {
                    Layout.fillWidth: true
                    text: I18n.t("Links from these servers connect without asking.")
                    color: Theme.textMuted
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
                Text {
                    objectName: "noServers"
                    visible: launcher.servers.length === 0
                    Layout.fillWidth: true
                    text: I18n.t("No server yet. Add yours below, or choose “Always trust this server” when a link asks.")
                    color: Theme.textSubtle
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
                Repeater {
                    objectName: "serverList"
                    model: launcher.servers
                    delegate: RowLayout {
                        objectName: "serverRow"
                        Layout.fillWidth: true
                        spacing: 8
                        Icon { name: "globe"; size: 16; color: Theme.textMuted }
                        Text {
                            Layout.fillWidth: true
                            textFormat: Text.PlainText
                            text: modelData.web
                            color: Theme.text
                            font.pixelSize: 13
                            elide: Text.ElideMiddle
                        }
                        SecondaryButton {
                            objectName: "openServer"
                            text: I18n.t("Open in browser")
                            onClicked: launcher.openUrl(modelData.web)
                        }
                        IconButton {
                            objectName: "removeServer"
                            iconName: "trash"
                            tip: I18n.t("Remove from the list")
                            onClicked: launcher.removeServer(modelData.server)
                        }
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    InputField {
                        id: addField
                        objectName: "addServerField"
                        implicitHeight: 36
                        fontSize: 13
                        placeholder: I18n.t("Server address, e.g. https://lunaris.example.com")
                        onAccepted: addButton.clicked()
                    }
                    PrimaryButton {
                        id: addButton
                        objectName: "addServerButton"
                        text: I18n.t("Add and open")
                        enabled: addField.text.trim().length > 0
                        onClicked: if (launcher.addServer(addField.text)) addField.text = ""
                    }
                }
                Text {
                    visible: launcher.addError !== ""
                    Layout.fillWidth: true
                    text: launcher.addError
                    color: Theme.danger
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
            }

            Card {
                title: I18n.t("Appearance")
                RowLayout {
                    spacing: 16
                    Text { text: I18n.t("Language"); color: Theme.text; font.pixelSize: 13; Layout.preferredWidth: 90 }
                    Segmented {
                        objectName: "languageChoice"
                        options: I18n.languages
                        value: I18n.lang
                        onPicked: (v) => launcher.setPref("language", v)
                    }
                }
                RowLayout {
                    spacing: 16
                    Text { text: I18n.t("Theme"); color: Theme.text; font.pixelSize: 13; Layout.preferredWidth: 90 }
                    Segmented {
                        objectName: "themeChoice"
                        options: [{ value: "system", label: "Follow system" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]
                        value: Theme.mode
                        onPicked: (v) => launcher.setPref("theme", v)
                    }
                }
            }

            Card {
                title: I18n.t("Updates")
                UpdatesPanel {
                    Layout.fillWidth: true
                    status: launcher.updateStatus
                }
            }

            Card {
                title: "Lunaris " + bridge.appVersion()
                Text {
                    Layout.fillWidth: true
                    text: I18n.t("Low-latency remote desktop. Free software under the GNU GPL v3 or later.")
                    color: Theme.textMuted
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: 8
                    SecondaryButton { text: I18n.t("Website"); onClicked: launcher.openUrl("https://github.com/collyn/lunaris") }
                    SecondaryButton { text: I18n.t("Report a problem"); onClicked: launcher.openUrl("https://github.com/collyn/lunaris/issues") }
                    SecondaryButton { text: I18n.t("Releases"); onClicked: launcher.openUrl("https://github.com/collyn/lunaris/releases") }
                }
            }
        }
    }
}
