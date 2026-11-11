import QtQuick
import QtQuick.Controls

Button {
    id: pb
    hoverEnabled: true
    implicitHeight: 36
    leftPadding: 18
    rightPadding: 18
    contentItem: Text {
        text: pb.text
        color: pb.enabled ? Theme.textOnAccent : Theme.textSubtle
        font.pixelSize: 14
        font.weight: Font.DemiBold
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }
    background: Rectangle {
        radius: Theme.radiusSm
        color: !pb.enabled ? Theme.surface3 : (pb.down || pb.hovered ? Theme.accentHover : Theme.accent)
    }
}
