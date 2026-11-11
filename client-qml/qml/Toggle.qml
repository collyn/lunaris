import QtQuick
import QtQuick.Controls

// On/off switch in the app's colors (Fusion follows the system palette).
Switch {
    id: sw
    implicitWidth: 44
    implicitHeight: 26
    padding: 0
    indicator: Rectangle {
        implicitWidth: 40
        implicitHeight: 22
        x: sw.width - width
        anchors.verticalCenter: parent.verticalCenter
        radius: 11
        color: sw.checked ? Theme.accent : Theme.surface3
        border.color: sw.checked ? Theme.accent : Theme.borderStrong
        Behavior on color { ColorAnimation { duration: 120 } }
        Rectangle {
            x: sw.checked ? parent.width - width - 3 : 3
            anchors.verticalCenter: parent.verticalCenter
            width: 16
            height: 16
            radius: 8
            color: "#ffffff"
            Behavior on x { NumberAnimation { duration: 120 } }
        }
    }
    contentItem: Item {}
}
