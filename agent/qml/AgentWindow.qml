import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Qt.labs.platform as Platform
import com.lunaris.agent 1.0

ApplicationWindow {
    id: window
    width: 480
    height: 280
    visible: false
    title: "Lunaris Host Agent"
    color: "#0d1117"
    flags: Qt.Window | Qt.WindowCloseButtonHint | Qt.WindowMinimizeButtonHint
    minimumWidth: 440; minimumHeight: 260

    // Expand window when log is shown
    onShowLogChanged: {
        if (showLog) { window.minimumHeight = 500; window.height = Math.max(window.height, 520) }
        else { window.minimumHeight = 260; window.height = 280 }
    }

    // ── Bridge ──
    AgentBridge { id: bridge }

    // ── State ──
    property bool agentActive: false
    property bool agentConnected: false
    property string agentId: ""
    property string updateVersion: ""
    property string updateUrl: ""
    property bool closeToTray: false
    property bool showLog: false

    // ── Tray ──
    Platform.SystemTrayIcon {
        id: trayIcon
        visible: true
        icon.source: "qrc:/icons/icon.png"
        tooltip: "Lunaris Host Agent"
        menu: Platform.Menu {
            Platform.MenuItem { text: window.visible ? "Hide" : "Show"; onTriggered: toggleWindow() }
            Platform.MenuItem { text: agentActive ? "Stop" : "Start"; onTriggered: agentActive ? bridge.stopAgent() : bridge.startAgent() }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: "Quit"; onTriggered: Qt.quit() }
        }
        onActivated: (reason) => { if (reason === Platform.SystemTrayIcon.Trigger) toggleWindow() }
    }
    function toggleWindow() { if (window.visible) { window.hide() } else { window.show(); window.raise() } }

    // ── Colors ──
    readonly property string cBg:       "#0d1117"
    readonly property string cCard:     "#161b22"
    readonly property string cField:    "#0d1117"
    readonly property string cBorder:   "#30363d"
    readonly property string cAccent:   "#58a6ff"
    readonly property string cText:     "#e6edf3"
    readonly property string cMuted:    "#8b949e"
    readonly property string cGreen:    "#3fb950"
    readonly property string cRed:      "#f85149"
    readonly property string cYellow:   "#d29922"
    readonly property string cLogInfo:  "#7ee787"
    readonly property string cLogWarn:  "#e3b341"
    readonly property string cLogError: "#f87171"
    readonly property string cLogDebug: "#8b949e"

    // ── Timers ──
    Timer { interval: 100;  running: true; repeat: true; onTriggered: bridge.pollLogs() }
    Timer { interval: 2000; running: true; repeat: true; onTriggered: bridge.pollStatus() }
    Timer { interval: 3600000; running: true; repeat: true; onTriggered: bridge.checkForUpdates() }

    // ── Layout ──
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 16
        spacing: 10

        // Update banner
        Rectangle {
            visible: window.updateVersion !== ""
            Layout.fillWidth: true; height: 30; radius: 6
            color: "#1a1a3e"; border.color: "#58a6ff"
            RowLayout {
                anchors.fill: parent; anchors.margins: 8
                Text { text: "Update available"; color: cText; font.pixelSize: 11; Layout.fillWidth: true }
                Rectangle {
                    width: updateLabel.contentWidth + 20; height: 22; radius: 4; color: "#58a6ff"
                    Text { id: updateLabel; anchors.centerIn: parent; text: "v" + window.updateVersion; font.pixelSize: 10; font.bold: true; color: "#000" }
                    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: bridge.openUrl(window.updateUrl) }
                }
            }
        }

        // Header row
        RowLayout {
            Layout.fillWidth: true; spacing: 10
            Image { source: "qrc:/icons/icon.png"; Layout.preferredWidth: 28; Layout.preferredHeight: 28 }
            Text { text: "Lunaris Agent"; font.pixelSize: 16; font.bold: true; color: cText; Layout.fillWidth: true }
            Rectangle {
                width: 10; height: 10; radius: 5
                color: window.agentConnected ? cGreen : (window.agentActive ? cYellow : cMuted)
            }
            Text {
                text: window.agentConnected ? "Connected" : (window.agentActive ? "Connecting" : "Idle")
                font.pixelSize: 12; color: window.agentConnected ? cGreen : (window.agentActive ? cYellow : cMuted)
            }
        }

        // Config + status card
        Rectangle {
            Layout.fillWidth: true; Layout.preferredHeight: 130
            color: cCard; border.color: cBorder; border.width: 1; radius: 8
            GridLayout {
                anchors.fill: parent; anchors.margins: 14
                columns: 2; columnSpacing: 10; rowSpacing: 6

                Text { text: "Server"; color: cMuted; font.pixelSize: 11 }
                TextField { id: fieldServerUrl; Layout.fillWidth: true; font.pixelSize: 11; color: cText
                    background: Rectangle { color: cField; border.color: cBorder; radius: 4 } }
                Text { text: "Token"; color: cMuted; font.pixelSize: 11 }
                TextField { id: fieldServerToken; Layout.fillWidth: true; echoMode: TextInput.Password; font.pixelSize: 11; color: cText
                    background: Rectangle { color: cField; border.color: cBorder; radius: 4 } }
                Text { text: "Name"; color: cMuted; font.pixelSize: 11 }
                TextField { id: fieldAgentName; Layout.fillWidth: true; font.pixelSize: 11; color: cText
                    background: Rectangle { color: cField; border.color: cBorder; radius: 4 } }
                RowLayout {
                    CheckBox { id: cbAutostart }
                    Text { text: "Start on boot"; color: cMuted; font.pixelSize: 11 }
                    Item { Layout.preferredWidth: 10 }
                    CheckBox { id: cbCloseToTray; onCheckStateChanged: window.closeToTray = checked }
                    Text { text: "Minimize to tray"; color: cMuted; font.pixelSize: 11 }
                }
            }
        }

        // Action row
        RowLayout {
            Layout.fillWidth: true; spacing: 8
            Rectangle {
                Layout.fillWidth: true; height: 34; radius: 6
                color: window.agentActive ? "#f85149" : "#238636"
                Text { anchors.centerIn: parent; text: window.agentActive ? "Stop" : "Start"; font.pixelSize: 12; font.bold: true; color: "#fff" }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                    onClicked: { if (agentActive) bridge.stopAgent(); else bridge.startAgent() } }
            }
            Rectangle {
                Layout.fillWidth: true; height: 34; radius: 6
                color: cCard; border.color: cBorder; border.width: 1
                Text { anchors.centerIn: parent; text: "Save"; font.pixelSize: 12; font.bold: true; color: cText }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                    onClicked: bridge.saveConfig(fieldServerUrl.text, fieldServerToken.text, cbAutostart.checked, cbCloseToTray.checked) }
            }
            Rectangle {
                width: 60; height: 34; radius: 6
                color: cCard; border.color: cBorder; border.width: 1
                Text { anchors.centerIn: parent; text: "Import"; font.pixelSize: 11; color: cText }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor
                    onClicked: { bridge.importConfig(); bridge.loadConfig() } }
            }
        }

        // Log toggle
        Rectangle {
            Layout.fillWidth: true; height: 28; radius: 6
            color: window.showLog ? cCard : "transparent"
            border.color: window.showLog ? cBorder : "transparent"
            border.width: 1
            RowLayout {
                anchors.fill: parent; anchors.leftMargin: 10; anchors.rightMargin: 10
                Text { text: window.showLog ? "Hide Log" : "Log (" + consoleModel.count + ")"; font.pixelSize: 11; color: cMuted; Layout.fillWidth: true }
                Text { text: consoleModel.count + " lines"; font.pixelSize: 10; color: cMuted; visible: !window.showLog }
                Rectangle {
                    visible: window.showLog; width: clearLabel.contentWidth + 16; height: 20; radius: 4
                    color: clearMa.containsMouse ? "#30363d" : "transparent"
                    Text { id: clearLabel; anchors.centerIn: parent; text: "Clear"; font.pixelSize: 10; color: cAccent }
                    MouseArea { id: clearMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor
                        onClicked: { consoleModel.clear(); bridge.clearLogs() } }
                }
            }
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: window.showLog = !window.showLog; z: -1 }
        }

        // Console (hidden by default)
        Rectangle {
            visible: window.showLog
            Layout.fillWidth: true; Layout.fillHeight: true; Layout.minimumHeight: 220
            color: "#010409"; border.color: cBorder; border.width: 1; radius: 6
            ListView {
                id: consoleView
                anchors.fill: parent; anchors.margins: 2
                clip: true; cacheBuffer: 500
                model: ListModel { id: consoleModel }
                delegate: Text {
                    text: model.text; color: Qt.color(model.colorStr)
                    font.family: "Courier New"; font.pixelSize: 10; lineHeight: 1.3
                    leftPadding: 10; width: consoleView.width - 20
                    wrapMode: Text.WrapAnywhere
                }
                ScrollBar.vertical: ScrollBar {}
                onCountChanged: { if (count > 0) positionViewAtEnd() }
            }
        }
    }

    // ── Signals ──
    Connections {
        target: bridge
        function onStatusChanged(active, connected) { window.agentActive = active; window.agentConnected = connected }
        function onLogMessage(msg) {
            var m = msg.toString ? msg.toString() : String(msg)
            var clr = cLogDebug
            var s = m.toLowerCase()
            if (s.indexOf("info") >= 0) clr = cLogInfo
            else if (s.indexOf("warn") >= 0) clr = cLogWarn
            else if (s.indexOf("error") >= 0 || s.indexOf("fail") >= 0) clr = cLogError
            consoleModel.append({text: m, colorStr: clr})
            if (consoleModel.count > 500) consoleModel.remove(0)
        }
        function onConfigLoaded(srv, tok, name, id, autostart, closetray) {
            fieldServerUrl.text = srv; fieldServerToken.text = tok
            fieldAgentName.text = name; window.agentId = id
            cbAutostart.checked = autostart; cbCloseToTray.checked = closetray
            window.closeToTray = closetray
        }
        function onUpdateAvailable(version, url) { window.updateVersion = version; window.updateUrl = url }
    }

    onClosing: (close) => { if (window.closeToTray) { close.accepted = false; window.hide() } }
    Component.onCompleted: { bridge.loadConfig(); window.show() }
}
