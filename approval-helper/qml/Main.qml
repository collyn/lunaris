import QtQuick
import QtQuick.Controls
import Qt.labs.platform as Platform
import com.lunaris.approval 1.0

// lunaris-approval-helper (src/main.rs): the tray of the person at this
// computer for the Lunaris agent running as a daemon (the Windows service,
// `--cli` on Linux). No settings, no agent: a small window that comes up
// only to ask "accept this connection?" (ApprovalDialog.qml).
ApplicationWindow {
    id: window
    width: 480
    height: 280
    minimumWidth: 360
    minimumHeight: 240
    visible: false
    title: "Lunaris Agent"
    color: Theme.bg
    flags: Qt.Window

    ApprovalBridge { id: bridge }

    property bool connected: false
    Timer {
        interval: 1000; running: true; repeat: true; triggeredOnStart: true
        onTriggered: window.connected = bridge.connected()
    }

    // Shown while an agent lets this helper answer.
    Platform.SystemTrayIcon {
        id: trayIcon
        visible: window.connected
        icon.source: "qrc:/icons/icon.png"
        tooltip: "Lunaris Agent — you are asked before anyone connects without the password"
        menu: Platform.Menu {
            Platform.MenuItem {
                text: "You are asked before anyone connects without the password"
                enabled: false
            }
        }
    }

    // Nothing to show without a question: closing only hides it (the
    // helper keeps running for the next one).
    onClosing: (close) => { close.accepted = false; window.hide() }

    ApprovalDialog {
        id: dialog
        agentBridge: bridge
        hostWindow: window
        tray: trayIcon
        onClosed: window.hide()
    }
}
