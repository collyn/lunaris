import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Qt.labs.platform as Platform
import com.lunaris.agent 1.0

ApplicationWindow {
    id: window
    width: 640
    height: 540
    visible: false
    title: "Lunaris Host Agent"
    color: "#080c14"
    flags: Qt.Window | Qt.WindowCloseButtonHint | Qt.WindowMinimizeButtonHint

    // ── Bridge ──
    AgentBridge { id: bridge }

    // ── Internal state ──
    property bool agentActive: false
    property bool agentConnected: false
    property string agentId: "N/A"
    property string updateVersion: ""
    property string updateUrl: ""
    property bool closeToTray: false

    // ── System tray ──
    Platform.SystemTrayIcon {
        id: trayIcon
        visible: true
        icon.source: "qrc:/icon.png"
        tooltip: "Lunaris Host Agent"
        menu: Platform.Menu {
            Platform.MenuItem {
                text: window.visible ? "Hide Window" : "Show Window"
                onTriggered: toggleWindow()
            }
            Platform.MenuItem {
                text: agentActive ? "Stop Agent" : "Start Agent"
                onTriggered: agentActive ? bridge.stopAgent() : bridge.startAgent()
            }
            Platform.MenuSeparator {}
            Platform.MenuItem {
                text: "Quit"
                onTriggered: Qt.quit()
            }
        }
        onActivated: (reason) => {
            if (reason === Platform.SystemTrayIcon.Trigger) toggleWindow()
        }
    }

    function toggleWindow() {
        if (window.visible) { window.hide() } else { window.show(); window.raise() }
    }

    // ── Colors ──
    readonly property color clrBg:       "#080c14"
    readonly property color clrPanel:    "#0f1626"
    readonly property color clrField:    "#172033"
    readonly property color clrBorder:   Qt.rgba(1,1,1,0.07)
    readonly property color clrText:     "#f1f5f9"
    readonly property color clrMuted:    "#94a3b8"
    readonly property color clrCyan:     "#00f0ff"
    readonly property color clrPurple:   "#9d4edd"
    readonly property color clrOnline:   "#00ff94"
    readonly property color clrOffline:  "#ef4444"
    readonly property color clrInfo:     "#a7f3d0"
    readonly property color clrWarn:     "#fde047"
    readonly property color clrError:    "#fca5a5"
    readonly property color clrDebug:    "#94a3b8"

    // ── Timers ──
    Timer { interval: 100;  running: true; repeat: true; onTriggered: bridge.pollLogs() }
    Timer { interval: 2000; running: true; repeat: true; onTriggered: bridge.pollStatus() }
    Timer { interval: 30000; running: true; repeat: true; onTriggered: bridge.checkForUpdates() }

    // ── Main content ──
    Rectangle {
        anchors.fill: parent
        anchors.margins: 24

        ColumnLayout {
            anchors.fill: parent
            spacing: 12

            // Update banner
            Rectangle {
                id: updateBanner
                visible: window.updateVersion !== ""
                Layout.fillWidth: true
                height: 36
                color: Qt.rgba(0.94, 0.31, 0.65, 0.15) // purple-cyan blend
                border.color: Qt.rgba(0, 0.94, 1, 0.2)
                border.width: 1
                radius: 8
                RowLayout {
                    anchors.fill: parent
                    anchors.margins: 8
                    Text {
                        text: "Update available: v" + window.updateVersion
                        color: clrText
                        font.pixelSize: 12
                        Layout.fillWidth: true
                    }
                    Button {
                        text: "Update"
                        onClicked: bridge.openUrl(window.updateUrl)
                        contentItem: Text { text: "Update"; color: clrBg; font.bold: true; font.pixelSize: 11; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                        background: Rectangle { color: clrCyan; radius: 4 }
                    }
                }
            }

            // Header
            Text {
                text: "🌙  LUNARIS AGENT"
                font.pixelSize: 18
                font.bold: true
                color: clrText
            }

            // Dashboard grid (Status + Config)
            RowLayout {
                Layout.fillWidth: true
                spacing: 12

                // ── Status panel ──
                Rectangle {
                    Layout.preferredWidth: (parent ? parent.width : 590) / 2 - 6
                    height: 180
                    color: clrPanel
                    border.color: clrBorder
                    border.width: 1
                    radius: 10
                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 14
                        spacing: 8
                        Text { text: "SYSTEM STATUS"; color: clrMuted; font.pixelSize: 10; font.bold: true }

                        RowLayout {
                            spacing: 8
                            Rectangle {
                                width: 10; height: 10; radius: 5
                                color: window.agentConnected ? clrOnline : (window.agentActive ? "#ffb703" : clrOffline)
                            }
                            Text {
                                text: window.agentConnected ? "Connected" : (window.agentActive ? "Connecting" : "Inactive")
                                color: window.agentConnected ? clrOnline : (window.agentActive ? "#ffb703" : clrOffline)
                                font.pixelSize: 13; font.bold: true
                            }
                        }

                        Item { Layout.fillHeight: true }

                        Text {
                            text: "Agent ID: " + window.agentId
                            color: clrMuted
                            font.pixelSize: 10
                            font.family: "Courier New"
                        }
                    }
                }

                // ── Config panel ──
                Rectangle {
                    Layout.fillWidth: true
                    height: 180
                    color: clrPanel
                    border.color: clrBorder
                    border.width: 1
                    radius: 10
                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 14
                        spacing: 6
                        Text { text: "CONFIGURATION"; color: clrMuted; font.pixelSize: 10; font.bold: true }

                        RowLayout { Text { text: "Server URL"; color: clrMuted; font.pixelSize: 11; Layout.preferredWidth: 80 }
                            TextField { id: fieldServerUrl; Layout.fillWidth: true; color: clrText; font.pixelSize: 11; background: Rectangle { color: clrField; border.color: clrBorder; radius: 4 } } }
                        RowLayout { Text { text: "Token"; color: clrMuted; font.pixelSize: 11; Layout.preferredWidth: 80 }
                            TextField { id: fieldServerToken; Layout.fillWidth: true; echoMode: TextInput.Password; color: clrText; font.pixelSize: 11; background: Rectangle { color: clrField; border.color: clrBorder; radius: 4 } } }
                        RowLayout { Text { text: "Name"; color: clrMuted; font.pixelSize: 11; Layout.preferredWidth: 80 }
                            TextField { id: fieldAgentName; Layout.fillWidth: true; color: clrText; font.pixelSize: 11; background: Rectangle { color: clrField; border.color: clrBorder; radius: 4 } } }

                        CheckBox { id: cbAutostart; text: "Start on boot"; contentItem: Text { text: "Start on boot"; color: clrMuted; font.pixelSize: 11 } }
                        CheckBox { id: cbCloseToTray; text: "Close to tray"; contentItem: Text { text: "Close to tray"; color: clrMuted; font.pixelSize: 11 }
                            onCheckStateChanged: window.closeToTray = checked }
                    }
                }
            }

            // ── Action buttons ──
            RowLayout {
                Layout.fillWidth: true
                spacing: 10

                Button {
                    id: btnToggle
                    Layout.fillWidth: true
                    text: window.agentActive ? "Stop Host Agent" : "Start Host Agent"
                    onClicked: { if (agentActive) bridge.stopAgent(); else bridge.startAgent() }
                    contentItem: Text { text: btnToggle.text; color: clrBg; font.bold: true; font.pixelSize: 12; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                    background: Rectangle {
                        color: window.agentActive ? clrOffline : clrCyan
                        radius: 6
                        gradient: Gradient {
                            GradientStop { position: 0; color: window.agentActive ? clrOffline : clrCyan }
                            GradientStop { position: 1; color: window.agentActive ? "#dc2626" : clrPurple }
                        }
                    }
                }

                Button {
                    Layout.fillWidth: true
                    text: "Save Config"
                    onClicked: bridge.saveConfig(fieldServerUrl.text, fieldServerToken.text, cbAutostart.checked, cbCloseToTray.checked)
                    contentItem: Text { text: "Save Config"; color: clrText; font.bold: true; font.pixelSize: 12; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                    background: Rectangle { color: Qt.rgba(1,1,1,0.06); border.color: clrBorder; radius: 6 }
                }

                Button {
                    text: "Import"
                    onClicked: { bridge.importConfig(); bridge.loadConfig() }
                    contentItem: Text { text: "Import"; color: clrText; font.bold: true; font.pixelSize: 12; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                    background: Rectangle { color: Qt.rgba(1,1,1,0.06); border.color: clrBorder; radius: 6 }
                }
            }

            // ── Console ──
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                color: "#050710"
                border.color: clrBorder
                radius: 10

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 0

                    // Console header
                    Rectangle {
                        Layout.fillWidth: true; height: 28
                        color: Qt.rgba(0.059, 0.086, 0.149, 0.6)
                        radius: 10
                        Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: parent.height - 10; color: parent.color }
                        RowLayout {
                            anchors.fill: parent; anchors.margins: 8
                            Text { text: "CONSOLE OUTPUT"; color: clrMuted; font.pixelSize: 9; font.bold: true }
                            Item { Layout.fillWidth: true }
                            Button { text: "Clear"; onClicked: { consoleModel.clear(); bridge.clearLogs() }
                                contentItem: Text { text: "Clear"; color: clrMuted; font.pixelSize: 9 } }
                            Button { text: "Copy"; onClicked: {
                                var lines = []; for (var i=0; i<consoleModel.count; i++) lines.push(consoleModel.get(i).text);
                                clipboard.setText(lines.join("\n"))
                            }
                                contentItem: Text { text: "Copy"; color: clrMuted; font.pixelSize: 9 } }
                        }
                    }

                    // Console log list
                    ListView {
                        id: consoleView
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        model: ListModel { id: consoleModel }
                        delegate: Text {
                            text: model.text
                            color: model.color
                            font.family: "Courier New"
                            font.pixelSize: 10
                            width: consoleView.width - 24
                            x: 12
                            wrapMode: Text.WrapAnywhere
                        }
                        ScrollBar.vertical: ScrollBar {}
                        onCountChanged: { if (count > 0) positionViewAtEnd() }
                    }
                }
            }
        }
    }

    // Clipboard helper
    property var clipboard: null
    Component.onCompleted: {
        clipboard = Qt.createQmlObject("import QtQml; QtObject { function setText(t) {} }", window)
        // Load initial config
        bridge.loadConfig()
        window.show()
    }

    // ── Signal handlers ──
    Connections {
        target: bridge
        function onStatusChanged(active, connected) {
            window.agentActive = active; window.agentConnected = connected
        }
        function onLogMessage(msg) {
            var m = msg.toString ? msg.toString() : String(msg)
            var clr = clrDebug
            if (m.indexOf("INFO") >= 0 || m.indexOf("info") >= 0) clr = clrInfo
            else if (m.indexOf("WARN") >= 0 || m.indexOf("warn") >= 0) clr = clrWarn
            else if (m.indexOf("ERROR") >= 0 || m.indexOf("error") >= 0) clr = clrError
            consoleModel.append({text: m, color: clr})
            if (consoleModel.count > 500) consoleModel.remove(0)
        }
        function onConfigLoaded(serverUrl, serverToken, agentName, clientUniqueId, autostart, closeToTray) {
            fieldServerUrl.text = serverUrl; fieldServerToken.text = serverToken
            fieldAgentName.text = agentName; window.agentId = clientUniqueId
            cbAutostart.checked = autostart; cbCloseToTray.checked = closeToTray
            window.closeToTray = closeToTray
        }
        function onUpdateAvailable(version, url) { window.updateVersion = version; window.updateUrl = url }
        function onConfigSaved(success, errorMsg) { if (!success) consoleModel.append({text: "[ERROR] " + errorMsg, color: clrError}) }
        function onImportCompleted(success, errorMsg) { if (!success) consoleModel.append({text: "[ERROR] " + errorMsg, color: clrError}) }
    }

    onClosing: (close) => {
        if (window.closeToTray) { close.accepted = false; window.hide() }
    }
}
