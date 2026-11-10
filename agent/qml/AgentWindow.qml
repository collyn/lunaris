import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Qt.labs.platform as Platform
import com.lunaris.agent 1.0

ApplicationWindow {
    id: window
    width: 460
    height: baseHeight
    minimumWidth: 420
    minimumHeight: baseHeight
    visible: false
    title: "Lunaris Agent"
    color: Theme.bg
    flags: Qt.Window

    readonly property int baseHeight: 560

    // Grow the window while the activity log is open.
    onShowLogChanged: {
        if (showLog) { window.minimumHeight = baseHeight + 200; window.height = Math.max(window.height, baseHeight + 240) }
        else { window.minimumHeight = baseHeight; window.height = baseHeight }
    }

    // ── Bridge ──
    AgentBridge { id: bridge }

    // Hidden helper for clipboard access
    TextField { id: clipHelper; visible: false }

    // ── State ──
    property bool agentActive: false
    property bool agentConnected: false
    property string agentId: ""
    property string agentName: ""
    property string updateVersion: ""
    property string updateUrl: ""
    property bool closeToTray: false
    property bool showLog: false
    property string notice: ""
    property bool noticeError: false

    readonly property string statusText: agentConnected ? "Online" : (agentActive ? "Connecting…" : "Offline")
    readonly property color statusColor: agentConnected ? Theme.success : (agentActive ? Theme.warning : Theme.textSubtle)

    // ── Tray ──
    Platform.SystemTrayIcon {
        id: trayIcon
        visible: true
        icon.source: "qrc:/icons/icon.png"
        tooltip: "Lunaris Agent — " + window.statusText
        menu: Platform.Menu {
            Platform.MenuItem { text: window.visible ? "Hide" : "Show"; onTriggered: toggleWindow() }
            Platform.MenuItem { text: agentActive ? "Stop sharing" : "Start sharing"; onTriggered: agentActive ? bridge.stopAgent() : saveAndStart() }
            Platform.MenuSeparator {}
            Platform.MenuItem { text: "Quit"; onTriggered: Qt.quit() }
        }
        onActivated: (reason) => { if (reason === Platform.SystemTrayIcon.Trigger) toggleWindow() }
    }
    function toggleWindow() { if (window.visible) { window.hide() } else { window.show(); window.raise() } }
    function saveForm() {
        bridge.saveConfig(fieldServerUrl.text, fieldServerToken.text, cbAutostart.checked, cbCloseToTray.checked)
    }
    // Start always uses what is in the form: persist it first, otherwise the
    // agent connects with the previously saved server URL / token.
    function saveAndStart() { saveForm(); bridge.startAgent() }
    function copyText(t) { clipHelper.text = t; clipHelper.selectAll(); clipHelper.copy(); clipHelper.text = "" }
    function flash(msg, isError) { window.notice = msg; window.noticeError = isError; noticeTimer.restart() }

    // ── Timers ──
    Timer { interval: 100;  running: true; repeat: true; onTriggered: bridge.pollLogs() }
    Timer { interval: 2000; running: true; repeat: true; onTriggered: bridge.pollStatus() }
    Timer { interval: 3600000; running: true; repeat: true; onTriggered: bridge.checkForUpdates() }
    Timer { id: noticeTimer; interval: 3500; onTriggered: window.notice = "" }

    // ── Reusable pieces ──
    component Card: Rectangle {
        color: Theme.surface
        border.color: Theme.border
        border.width: 1
        radius: Theme.radiusLg
    }
    component SectionLabel: Text {
        color: Theme.textMuted
        font.pixelSize: 11
        font.weight: Font.DemiBold
        font.letterSpacing: 0.4
    }
    component Field: TextField {
        Layout.fillWidth: true
        implicitHeight: 34
        leftPadding: 10; rightPadding: 10
        font.pixelSize: 13
        color: Theme.textStrong
        placeholderTextColor: Theme.textSubtle
        selectByMouse: true
        background: Rectangle {
            radius: Theme.radiusSm
            color: Theme.surface2
            border.width: 1
            border.color: parent.activeFocus ? Theme.accentFg : Theme.borderStrong
        }
    }
    component Toggle: CheckBox {
        id: toggle
        spacing: 8
        indicator: Rectangle {
            implicitWidth: 18; implicitHeight: 18
            x: toggle.leftPadding; y: (toggle.height - height) / 2
            radius: 4
            color: toggle.checked ? Theme.accent : Theme.surface2
            border.color: toggle.checked ? Theme.accent : Theme.borderStrong
            Text {
                anchors.centerIn: parent
                visible: toggle.checked
                text: "✓"; color: Theme.textOnAccent
                font.pixelSize: 12; font.bold: true
            }
        }
        contentItem: Text {
            leftPadding: toggle.indicator.width + toggle.spacing
            text: toggle.text
            color: Theme.text
            font.pixelSize: 13
            verticalAlignment: Text.AlignVCenter
        }
    }
    component GhostButton: Rectangle {
        id: ghost
        property alias label: ghostLabel.text
        property bool danger: false
        signal clicked()
        implicitHeight: 38
        implicitWidth: ghostLabel.implicitWidth + 28
        radius: Theme.radius
        color: ghostMa.containsMouse ? Theme.surface2 : Theme.surface
        border.width: 1
        border.color: danger ? Theme.danger : Theme.borderStrong
        Text {
            id: ghostLabel
            anchors.centerIn: parent
            font.pixelSize: 13; font.weight: Font.DemiBold
            color: ghost.danger ? Theme.danger : Theme.textStrong
        }
        MouseArea { id: ghostMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: ghost.clicked() }
    }
    component LinkButton: Text {
        id: link
        signal clicked()
        color: linkMa.containsMouse ? Theme.accentFg : Theme.textMuted
        font.pixelSize: 12
        font.underline: linkMa.containsMouse
        MouseArea { id: linkMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: link.clicked() }
    }

    // ── Layout ──
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        spacing: 14

        // Header: brand + status pill
        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Image { source: "qrc:/icons/icon.png"; sourceSize: Qt.size(28, 28); Layout.preferredWidth: 28; Layout.preferredHeight: 28 }
            Text { text: "Lunaris Agent"; color: Theme.textStrong; font.pixelSize: 17; font.weight: Font.DemiBold; Layout.fillWidth: true }
            Rectangle {
                implicitHeight: 26
                implicitWidth: statusRow.implicitWidth + 20
                radius: 13
                color: Theme.surface2
                border.color: Theme.border
                RowLayout {
                    id: statusRow
                    anchors.centerIn: parent
                    spacing: 6
                    Rectangle { width: 8; height: 8; radius: 4; color: window.statusColor }
                    Text { text: window.statusText; color: Theme.text; font.pixelSize: 12; font.weight: Font.Medium }
                }
            }
        }

        // Update banner
        Rectangle {
            visible: window.updateVersion !== ""
            Layout.fillWidth: true
            implicitHeight: 38
            radius: Theme.radius
            color: Theme.accentA(0.12)
            border.color: Theme.accentA(0.4)
            RowLayout {
                anchors.fill: parent; anchors.leftMargin: 12; anchors.rightMargin: 8
                Text { text: "Lunaris " + window.updateVersion + " is available"; color: Theme.textStrong; font.pixelSize: 12; Layout.fillWidth: true }
                LinkButton { text: "Download"; color: Theme.accentFg; onClicked: bridge.openUrl(window.updateUrl) }
            }
        }

        // This computer
        Card {
            Layout.fillWidth: true
            implicitHeight: thisPc.implicitHeight + 28
            ColumnLayout {
                id: thisPc
                anchors.fill: parent; anchors.margins: 14
                spacing: 6
                SectionLabel { text: "THIS COMPUTER" }
                Text {
                    text: window.agentName !== "" ? window.agentName : "—"
                    color: Theme.textStrong; font.pixelSize: 15; font.weight: Font.DemiBold
                    elide: Text.ElideRight; Layout.fillWidth: true
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Text {
                        text: window.agentId !== "" ? "ID " + window.agentId : "ID is assigned on first start"
                        color: Theme.textMuted; font.pixelSize: 12; font.family: "monospace"
                        elide: Text.ElideMiddle; Layout.fillWidth: true
                    }
                    LinkButton {
                        visible: window.agentId !== ""
                        text: "Copy"
                        onClicked: { copyText(window.agentId); flash("ID copied", false) }
                    }
                }
            }
        }

        // Server connection
        Card {
            Layout.fillWidth: true
            implicitHeight: serverCol.implicitHeight + 28
            ColumnLayout {
                id: serverCol
                anchors.fill: parent; anchors.margins: 14
                spacing: 8
                RowLayout {
                    Layout.fillWidth: true
                    SectionLabel { text: "SERVER"; Layout.fillWidth: true }
                    LinkButton { text: "Import config…"; onClicked: { bridge.importConfig(); bridge.loadConfig() } }
                }
                Text { text: "Address"; color: Theme.text; font.pixelSize: 12 }
                Field { id: fieldServerUrl; placeholderText: "ws://your-server:8090" }
                Text { text: "Agent token"; color: Theme.text; font.pixelSize: 12; Layout.topMargin: 2 }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    Field { id: fieldServerToken; echoMode: showToken.checked ? TextInput.Normal : TextInput.Password; placeholderText: "From the server's web UI or server_token.txt" }
                    LinkButton {
                        id: showToken
                        property bool checked: false
                        text: checked ? "Hide" : "Show"
                        onClicked: checked = !checked
                    }
                }
            }
        }

        // Options
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Toggle { id: cbAutostart; text: "Start when I sign in" }
            Toggle { id: cbCloseToTray; text: "Keep running in the tray when closed"; onCheckedChanged: window.closeToTray = checked }
        }

        // Actions
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Rectangle {
                id: primary
                Layout.fillWidth: true
                implicitHeight: 38
                radius: Theme.radius
                visible: !window.agentActive
                color: primaryMa.containsMouse ? Theme.accentHover : Theme.accent
                Text { anchors.centerIn: parent; text: "Start sharing"; color: Theme.textOnAccent; font.pixelSize: 13; font.weight: Font.DemiBold }
                MouseArea { id: primaryMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: saveAndStart() }
            }
            GhostButton {
                Layout.fillWidth: true
                visible: window.agentActive
                danger: true
                label: "Stop sharing"
                onClicked: bridge.stopAgent()
            }
            GhostButton { label: "Save"; onClicked: saveForm() }
        }

        Text {
            Layout.fillWidth: true
            visible: window.notice !== ""
            text: window.notice
            color: window.noticeError ? Theme.danger : Theme.textMuted
            font.pixelSize: 12
            wrapMode: Text.Wrap
        }

        // Activity log
        RowLayout {
            Layout.fillWidth: true
            spacing: 12
            LinkButton {
                text: (window.showLog ? "▾ " : "▸ ") + "Activity log (" + consoleModel.count + ")"
                color: Theme.textMuted
                onClicked: window.showLog = !window.showLog
            }
            Item { Layout.fillWidth: true }
            LinkButton {
                visible: window.showLog
                text: "Copy"
                onClicked: {
                    var lines = []; for (var i = 0; i < consoleModel.count; i++) lines.push(consoleModel.get(i).text)
                    copyText(lines.join("\n")); flash("Log copied", false)
                }
            }
            LinkButton { visible: window.showLog; text: "Clear"; onClicked: { consoleModel.clear(); bridge.clearLogs() } }
        }

        Rectangle {
            visible: window.showLog
            Layout.fillWidth: true; Layout.fillHeight: true; Layout.minimumHeight: 180
            color: Theme.surface2; border.color: Theme.border; border.width: 1; radius: Theme.radius
            ListView {
                id: consoleView
                anchors.fill: parent; anchors.margins: 6
                clip: true; cacheBuffer: 500
                model: ListModel { id: consoleModel }
                delegate: Text {
                    text: model.text; color: model.colorStr
                    font.family: "monospace"; font.pixelSize: 11; lineHeight: 1.25
                    leftPadding: 4; width: consoleView.width - 12
                    wrapMode: Text.WrapAnywhere
                }
                ScrollBar.vertical: ScrollBar {}
                onCountChanged: { if (count > 0) positionViewAtEnd() }
            }
        }

        Item { Layout.fillHeight: !window.showLog }
    }

    // ── Signals ──
    Connections {
        target: bridge
        function onStatusChanged(active, connected) { window.agentActive = active; window.agentConnected = connected }
        function onLogMessage(msg) {
            var m = msg.toString ? msg.toString() : String(msg)
            var s = m.toLowerCase()
            var clr = Theme.textMuted
            if (s.indexOf("error") >= 0 || s.indexOf("fail") >= 0) clr = Theme.danger
            else if (s.indexOf("warn") >= 0) clr = Theme.warning
            else if (s.indexOf("info") >= 0) clr = Theme.text
            consoleModel.append({ text: m, colorStr: String(clr) })
            if (consoleModel.count > 500) consoleModel.remove(0)
        }
        function onConfigLoaded(srv, tok, name, id, autostart, closetray) {
            fieldServerUrl.text = srv; fieldServerToken.text = tok
            window.agentName = name; window.agentId = id
            cbAutostart.checked = autostart; cbCloseToTray.checked = closetray
            window.closeToTray = closetray
        }
        function onConfigSaved(success, error) {
            if (success) flash("Settings saved", false); else flash("Could not save settings: " + error, true)
        }
        function onImportCompleted(success, error) {
            if (success) flash("Configuration imported", false); else if (error !== "") flash("Import failed: " + error, true)
        }
        function onUpdateAvailable(version, url) { window.updateVersion = version; window.updateUrl = url }
    }

    onClosing: (close) => { if (window.closeToTray) { close.accepted = false; window.hide() } }
    Component.onCompleted: { bridge.loadConfig(); window.show() }
}
