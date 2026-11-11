import QtQuick
import QtQuick.Controls

// A small borderless button showing an Icon, with a tooltip.
AbstractButton {
    id: ib
    property string iconName: ""
    property string tip: ""
    property color iconColor: Theme.textMuted
    property real iconSize: 17
    implicitWidth: 30
    implicitHeight: 30
    hoverEnabled: true
    Accessible.name: tip
    ToolTip.visible: hovered && tip !== ""
    ToolTip.text: tip
    ToolTip.delay: 400
    background: Rectangle {
        radius: Theme.radiusSm
        color: ib.down ? Theme.surface3 : (ib.hovered ? Theme.surface2 : "transparent")
    }
    contentItem: Item {
        Icon {
            anchors.centerIn: parent
            name: ib.iconName
            size: ib.iconSize
            color: ib.hovered ? Theme.textStrong : ib.iconColor
        }
    }
}
