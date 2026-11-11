import QtQuick
import QtQuick.Controls

ComboBox {
    id: control
    
    property real customWidth: 100
    // The in-stream menu is always dark (Theme.live); elsewhere follow the
    // app's theme with live: false.
    property bool live: true
    readonly property var pal: live ? Theme.live : Theme
    implicitWidth: customWidth
    implicitHeight: 28

    delegate: ItemDelegate {
        width: control.width
        height: 28
        contentItem: Text {
            text: modelData
            color: control.pal.textStrong
            font.pixelSize: 11
            font.bold: true
            elide: Text.ElideRight
            verticalAlignment: Text.AlignVCenter
            horizontalAlignment: Text.AlignHCenter
        }
        background: Rectangle {
            color: hovered ? control.pal.ink(0.08) : "transparent"
            radius: 6
        }
    }

    indicator: Canvas {
        id: canvas
        x: control.width - width - 10
        y: (control.height - height) / 2
        width: 8
        height: 5
        contextType: "2d"

        onPaint: {
            var ctx = canvas.getContext("2d");
            ctx.reset();
            ctx.moveTo(0, 0);
            ctx.lineTo(width, 0);
            ctx.lineTo(width / 2, height);
            ctx.closePath();
            ctx.fillStyle = control.pal.textMuted;
            ctx.fill();
        }
    }

    contentItem: Text {
        leftPadding: 12
        rightPadding: control.indicator.width + 12
        text: control.displayText
        font.pixelSize: 11
        font.bold: true
        color: control.pal.textStrong
        verticalAlignment: Text.AlignVCenter
        horizontalAlignment: Text.AlignLeft
        elide: Text.ElideRight
    }

    background: Rectangle {
        color: control.hovered ? control.pal.ink(0.12) : control.pal.ink(0.06)
        border.color: "transparent"
        radius: 8
    }

    popup: Popup {
        y: control.height + 4
        width: control.width
        implicitHeight: contentItem.implicitHeight > 200 ? 200 : contentItem.implicitHeight
        padding: 4

        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: control.popup.visible ? control.delegateModel : null
            currentIndex: control.highlightedIndex

            ScrollIndicator.vertical: ScrollIndicator { }
        }

        background: Rectangle {
            color: control.pal.surface2
            border.color: control.pal.ink(0.08)
            border.width: 1
            radius: 8
        }
    }
}
