import QtQuick
import QtQuick.Controls

// A drop-down in the app's colours. `options` is a list of values or of
// { value, label }; `value` is the selected value; `picked(value)` fires on a
// user choice.
ComboBox {
    id: combo
    property var options: []
    property var value
    signal picked(var value)

    readonly property var labels: options.map(function (o) { return I18n.t(typeof o === "object" ? o.label : String(o)); })
    function indexOfValue(v) {
        for (var i = 0; i < options.length; i++) {
            var ov = typeof options[i] === "object" ? options[i].value : options[i];
            if (String(ov) === String(v)) return i;
        }
        return -1;
    }

    implicitWidth: 260
    implicitHeight: 40
    model: labels
    currentIndex: indexOfValue(value)
    onActivated: (index) => {
        var o = options[index];
        combo.picked(typeof o === "object" ? o.value : o);
    }
    Accessible.name: displayText

    background: Rectangle {
        radius: Theme.radiusSm
        color: Theme.bg
        border.color: combo.activeFocus || combo.popup.visible ? Theme.accent : (combo.hovered ? Theme.borderStrong : Theme.border)
        border.width: 1
    }
    contentItem: Text {
        leftPadding: 14
        rightPadding: 34
        text: combo.currentIndex >= 0 ? combo.displayText : "—"
        color: Theme.textStrong
        font.pixelSize: 13
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }
    indicator: Icon {
        x: combo.width - width - 12
        anchors.verticalCenter: parent.verticalCenter
        name: "chevronDown"
        size: 15
        color: Theme.textMuted
    }
    delegate: ItemDelegate {
        width: combo.width
        height: 34
        highlighted: combo.highlightedIndex === index
        contentItem: Text {
            text: modelData
            color: index === combo.currentIndex ? Theme.accentFg : Theme.text
            font.pixelSize: 13
            font.weight: index === combo.currentIndex ? Font.DemiBold : Font.Normal
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
        background: Rectangle { color: highlighted ? Theme.surface2 : "transparent" }
    }
    popup: Popup {
        y: combo.height + 4
        width: combo.width
        implicitHeight: Math.min(contentItem.implicitHeight + 8, 320)
        padding: 4
        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: combo.popup.visible ? combo.delegateModel : null
            currentIndex: combo.highlightedIndex
            ScrollIndicator.vertical: ScrollIndicator {}
        }
        background: Rectangle {
            radius: Theme.radiusSm
            color: Theme.surface
            border.color: Theme.borderStrong
        }
    }
}
