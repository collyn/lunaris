import QtQuick
import QtQuick.Controls

Button {
    id: sb
    property bool danger: false
    hoverEnabled: true
    implicitHeight: 34
    leftPadding: 14
    rightPadding: 14
    contentItem: Text {
        text: sb.text
        color: sb.danger ? Theme.danger : Theme.text
        font.pixelSize: 13
        font.weight: Font.Medium
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        opacity: sb.enabled ? 1 : 0.5
    }
    background: Rectangle {
        radius: Theme.radiusSm
        color: sb.down ? Theme.surface3 : (sb.hovered ? Theme.surface2 : "transparent")
        border.color: sb.danger && sb.hovered ? Theme.danger : Theme.borderStrong
        border.width: 1
    }
}
