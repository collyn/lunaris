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
    color: "#0b0e17"
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
        icon.source: "qrc:/icons/icon.png"
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
            Platform.MenuItem { text: "Quit"; onTriggered: Qt.quit() }
        }
        onActivated: (reason) => {
            if (reason === Platform.SystemTrayIcon.Trigger) toggleWindow()
        }
    }

    function toggleWindow() {
        if (window.visible) { window.hide() } else { window.show(); window.raise() }
    }

    // ── Color palette (strings, not color — stored in ListModel safely) ──
    readonly property string cBg:       "#0b0e17"
    readonly property string cPanel:    "#111827"
    readonly property string cField:    "#1a2236"
    readonly property string cBorder:   "#1e293b"
    readonly property string cAccent:   "#6366f1"
    readonly property string cAccent2:  "#818cf8"
    readonly property string cCyan:     "#22d3ee"
    readonly property string cText:     "#f1f5f9"
    readonly property string cMuted:    "#64748b"
    readonly property string cOnline:   "#22c55e"
    readonly property string cOffline:  "#ef4444"
    readonly property string cWarn:     "#f59e0b"
    readonly property string cLogInfo:  "#86efac"
    readonly property string cLogWarn:  "#fde047"
    readonly property string cLogError: "#fca5a5"
    readonly property string cLogDebug: "#64748b"

    // ── Timers ──
    Timer { interval: 100;  running: true; repeat: true; onTriggered: bridge.pollLogs() }
    Timer { interval: 2000; running: true; repeat: true; onTriggered: bridge.pollStatus() }
    Timer { interval: 30000; running: true; repeat: true; onTriggered: bridge.checkForUpdates() }

    // ── Component: panel card ──
    component PanelCard : Rectangle {
        color: cPanel
        border.color: cBorder
        border.width: 1
        radius: 12
    }

    // ── Component: action button ──
    component ActionButton : Button {
        id: btn
        property color bgColor: cAccent
        property color hoverColor: cAccent2
        font.pixelSize: 12
        font.bold: true
        contentItem: Text {
            text: btn.text
            color: "#ffffff"
            font: btn.font
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
        background: Rectangle {
            color: btn.hovered ? btn.hoverColor : btn.bgColor
            radius: 8
            Behavior on color { ColorAnimation { duration: 150 } }
        }
    }

    // ── Main content ──
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        spacing: 12

        // Update banner
        Rectangle {
            id: updateBanner
            visible: window.updateVersion !== ""
            Layout.fillWidth: true
            height: 36
            color: "#1e1345"
            border.color: "#6366f1"
            border.width: 1
            radius: 8
            RowLayout {
                anchors.fill: parent; anchors.margins: 8
                Text {
                    text: "⬆ Update v" + window.updateVersion + " available"
                    color: cText; font.pixelSize: 12
                    Layout.fillWidth: true
                }
                ActionButton {
                    text: "Download"; bgColor: "#6366f1"; hoverColor: "#818cf8"
                    font.pixelSize: 11
                    Layout.preferredWidth: 80; Layout.preferredHeight: 26
                    onClicked: bridge.openUrl(window.updateUrl)
                }
            }
        }

        // Header
        RowLayout {
            Layout.fillWidth: true
            Rectangle {
                width: 32; height: 32; radius: 8; color: "#6366f1"
                Text { anchors.centerIn: parent; text: "☾"; font.pixelSize: 18; color: "#fff" }
            }
            ColumnLayout {
                Layout.fillWidth: true; spacing: 0
                Text { text: "Lunaris Agent"; font.pixelSize: 16; font.bold: true; color: cText }
                Text { text: "v" + Qt.application.version; font.pixelSize: 10; color: cMuted }
            }
            Item { Layout.fillWidth: true }
        }

        // Dashboard grid
        RowLayout {
            Layout.fillWidth: true
            spacing: 12

            // ── Status card ──
            PanelCard {
                Layout.preferredWidth: (parent.width - 12) / 2
                Layout.preferredHeight: 150
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 8
                    Text { text: "STATUS"; font.pixelSize: 10; font.bold: true; color: cMuted; letterSpacing: 1 }

                    RowLayout {
                        Rectangle {
                            width: 12; height: 12; radius: 6
                            color: window.agentConnected ? cOnline : (window.agentActive ? cWarn : cOffline)
                        }
                        Text {
                            text: window.agentConnected ? "Connected" : (window.agentActive ? "Connecting..." : "Inactive")
                            font.pixelSize: 14; font.bold: true
                            color: window.agentConnected ? cOnline : (window.agentActive ? cWarn : cOffline)
                        }
                    }

                    Item { Layout.fillHeight: true }

                    Rectangle { Layout.fillWidth: true; height: 1; color: cBorder }
                    Text { text: window.agentId; font.pixelSize: 10; color: cMuted; font.family: "Courier New" }
                }
            }

            // ── Config card ──
            PanelCard {
                Layout.fillWidth: true
                Layout.preferredHeight: 150
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 6
                    Text { text: "CONFIGURATION"; font.pixelSize: 10; font.bold: true; color: cMuted; letterSpacing: 1 }

                    RowLayout { Text { text: "Server"; color: cMuted; font.pixelSize: 11; Layout.preferredWidth: 50 }
                        TextField { id: fieldServerUrl; Layout.fillWidth: true; font.pixelSize: 11; color: cText
                            background: Rectangle { color: cField; border.color: cBorder; radius: 6 } } }
                    RowLayout { Text { text: "Token"; color: cMuted; font.pixelSize: 11; Layout.preferredWidth: 50 }
                        TextField { id: fieldServerToken; Layout.fillWidth: true; echoMode: TextInput.Password; font.pixelSize: 11; color: cText
                            background: Rectangle { color: cField; border.color: cBorder; radius: 6 } } }
                    RowLayout { Text { text: "Name"; color: cMuted; font.pixelSize: 11; Layout.preferredWidth: 50 }
                        TextField { id: fieldAgentName; Layout.fillWidth: true; font.pixelSize: 11; color: cText
                            background: Rectangle { color: cField; border.color: cBorder; radius: 6 } } }

                    RowLayout {
                        CheckBox { id: cbAutostart; checked: false }
                        Text { text: "Start on boot"; color: cMuted; font.pixelSize: 11 }
                        Item { Layout.fillWidth: true }
                        CheckBox { id: cbCloseToTray; checked: false; onCheckStateChanged: window.closeToTray = checked }
                        Text { text: "Minimize to tray"; color: cMuted; font.pixelSize: 11 }
                    }
                }
            }
        }

        // ── Action buttons ──
        RowLayout {
            Layout.fillWidth: true
            spacing: 10

            ActionButton {
                id: btnToggle
                Layout.fillWidth: true; Layout.preferredHeight: 38
                text: window.agentActive ? "■ Stop Agent" : "▶ Start Agent"
                bgColor: window.agentActive ? "#ef4444" : "#22c55e"
                hoverColor: window.agentActive ? "#f87171" : "#4ade80"
                onClicked: { if (agentActive) bridge.stopAgent(); else bridge.startAgent() }
            }
            ActionButton {
                Layout.fillWidth: true; Layout.preferredHeight: 38
                text: "Save Config"
                bgColor: cField; hoverColor: "#1e293b"
                onClicked: bridge.saveConfig(fieldServerUrl.text, fieldServerToken.text, cbAutostart.checked, cbCloseToTray.checked)
            }
            ActionButton {
                Layout.preferredWidth: 90; Layout.preferredHeight: 38
                text: "Import"
                bgColor: cField; hoverColor: "#1e293b"
                onClicked: { bridge.importConfig(); bridge.loadConfig() }
            }
        }

        // ── Console ──
        PanelCard {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ColumnLayout {
                anchors.fill: parent
                spacing: 0

                // Console toolbar
                Rectangle {
                    Layout.fillWidth: true; height: 32
                    color: "transparent"
                    RowLayout {
                        anchors.fill: parent; anchors.leftMargin: 16; anchors.rightMargin: 8
                        Text { text: "CONSOLE LOG"; font.pixelSize: 10; font.bold: true; color: cMuted; letterSpacing: 1 }
                        Item { Layout.fillWidth: true }
                        Text { text: consoleModel.count + " lines"; font.pixelSize: 10; color: cMuted }
                        ActionButton {
                            text: "Clear"; bgColor: "transparent"; hoverColor: "#1e293b"
                            font.pixelSize: 10; Layout.preferredWidth: 50; Layout.preferredHeight: 24
                            onClicked: { consoleModel.clear(); bridge.clearLogs() }
                        }
                    }
                }
                Rectangle { Layout.fillWidth: true; height: 1; color: cBorder }

                // Log list
                ListView {
                    id: consoleView
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    cacheBuffer: 1000
                    model: ListModel { id: consoleModel }
                    delegate: Text {
                        text: model.text
                        // Convert hex color string to QColor explicitly
                        color: Qt.color(model.colorStr)
                        font.family: "Courier New"
                        font.pixelSize: 10
                        lineHeight: 1.4
                        leftPadding: 16
                        rightPadding: 16
                        width: consoleView.width
                        wrapMode: Text.WrapAnywhere
                    }
                    ScrollBar.vertical: ScrollBar {}
                    onCountChanged: { if (count > 0) positionViewAtEnd() }
                }
            }
        }
    }

    // ── Signal handlers ──
    Connections {
        target: bridge
        function onStatusChanged(active, connected) {
            window.agentActive = active; window.agentConnected = connected
        }
        function onLogMessage(msg) {
            var m = msg.toString ? msg.toString() : String(msg)
            var clr = cLogDebug
            if (m.indexOf("INFO") >= 0 || m.indexOf("info") >= 0) clr = cLogInfo
            else if (m.indexOf("WARN") >= 0 || m.indexOf("warn") >= 0) clr = cLogWarn
            else if (m.indexOf("ERROR") >= 0 || m.indexOf("error") >= 0) clr = cLogError
            consoleModel.append({text: m, colorStr: clr})
            if (consoleModel.count > 500) consoleModel.remove(0)
        }
        function onConfigLoaded(serverUrl, serverToken, agentName, clientUniqueId, autostart, closeToTray) {
            fieldServerUrl.text = serverUrl; fieldServerToken.text = serverToken
            fieldAgentName.text = agentName; window.agentId = clientUniqueId
            cbAutostart.checked = autostart; cbCloseToTray.checked = closeToTray
            window.closeToTray = closeToTray
        }
        function onUpdateAvailable(version, url) { window.updateVersion = version; window.updateUrl = url }
        function onConfigSaved(success, errorMsg) { if (!success) consoleModel.append({text: "[ERROR] " + errorMsg, colorStr: cLogError}) }
        function onImportCompleted(success, errorMsg) { if (!success) consoleModel.append({text: "[ERROR] " + errorMsg, colorStr: cLogError}) }
    }

    onClosing: (close) => {
        if (window.closeToTray) { close.accepted = false; window.hide() }
    }

    Component.onCompleted: {
        bridge.loadConfig()
        window.show()
    }
}
