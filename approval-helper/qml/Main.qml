import QtQuick
import QtQuick.Controls
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

    // The tray icon and its menu are qt_app.cpp's (QSystemTrayIcon).

    // Nothing to show without a question: closing only hides it (the
    // helper keeps running for the next one).
    onClosing: (close) => { close.accepted = false; window.hide() }

    ApprovalDialog {
        id: dialog
        agentBridge: bridge
        hostWindow: window
        onClosed: window.hide()
    }
}
