import QtQuick
import QtQuick.Controls

RadioButton {
    id: rb
    spacing: 10
    indicator: Rectangle {
        implicitWidth: 18
        implicitHeight: 18
        x: rb.leftPadding
        anchors.verticalCenter: parent.verticalCenter
        radius: 9
        color: "transparent"
        border.color: rb.checked ? Theme.accent : Theme.borderStrong
        border.width: 1.5
        Rectangle { anchors.centerIn: parent; width: 8; height: 8; radius: 4; color: Theme.accent; visible: rb.checked }
    }
    contentItem: Text {
        leftPadding: rb.indicator.width + rb.spacing
        text: rb.text
        color: Theme.text
        font.pixelSize: 13
        verticalAlignment: Text.AlignVCenter
    }
}
