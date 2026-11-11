import QtQuick
import QtQuick.Controls

Dialog {
    modal: true
    anchors.centerIn: Overlay.overlay
    width: 400
    padding: 22
    topPadding: 20
    background: Rectangle {
        radius: Theme.radiusLg
        color: Theme.surface
        border.color: Theme.borderStrong
    }
    Overlay.modal: Rectangle { color: Qt.rgba(0, 0, 0, 0.45) }
}
