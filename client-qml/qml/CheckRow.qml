import QtQuick
import QtQuick.Controls

CheckBox {
    id: cb
    spacing: 10
    indicator: Rectangle {
        implicitWidth: 18
        implicitHeight: 18
        x: cb.leftPadding
        anchors.verticalCenter: parent.verticalCenter
        radius: 4
        color: cb.checked ? Theme.accent : "transparent"
        border.color: cb.checked ? Theme.accent : Theme.borderStrong
        border.width: 1.5
        Icon { anchors.centerIn: parent; visible: cb.checked; name: "check"; size: 14; strokeWidth: 2.4; color: "#ffffff" }
    }
    contentItem: Text {
        leftPadding: cb.indicator.width + cb.spacing
        text: cb.text
        color: Theme.text
        font.pixelSize: 13
        verticalAlignment: Text.AlignVCenter
        wrapMode: Text.WordWrap
    }
}
