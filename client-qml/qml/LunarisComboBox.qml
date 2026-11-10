import QtQuick
import QtQuick.Controls

ComboBox {
    id: control
    
    property real customWidth: 100
    implicitWidth: customWidth
    implicitHeight: 28

    delegate: ItemDelegate {
        width: control.width
        height: 28
        contentItem: Text {
            text: modelData
            color: Theme.live.textStrong
            font.pixelSize: 11
            font.bold: true
            elide: Text.ElideRight
            verticalAlignment: Text.AlignVCenter
            horizontalAlignment: Text.AlignHCenter
        }
        background: Rectangle {
            color: hovered ? Theme.live.ink(0.08) : "transparent"
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
            ctx.fillStyle = Theme.live.textMuted;
            ctx.fill();
        }
    }

    contentItem: Text {
        leftPadding: 12
        rightPadding: control.indicator.width + 12
        text: control.displayText
        font.pixelSize: 11
        font.bold: true
        color: Theme.live.textStrong
        verticalAlignment: Text.AlignVCenter
        horizontalAlignment: Text.AlignLeft
        elide: Text.ElideRight
    }

    background: Rectangle {
        color: control.hovered ? Theme.live.ink(0.12) : Theme.live.ink(0.06)
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
            color: Theme.live.surface2
            border.color: Theme.live.ink(0.08)
            border.width: 1
            radius: 8
        }
    }
}
