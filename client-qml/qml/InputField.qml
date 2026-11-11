import QtQuick
import QtQuick.Layouts

// A one-line text box with a placeholder.
Rectangle {
    id: field
    property alias input: fieldInput
    property alias text: fieldInput.text
    property string placeholder: ""
    property bool password: false
    property int fontSize: 14
    signal accepted()
    Layout.fillWidth: true
    implicitWidth: 200
    implicitHeight: 40
    radius: Theme.radiusSm
    color: Theme.bg
    border.color: fieldInput.activeFocus ? Theme.accent : Theme.borderStrong
    border.width: 1
    TextInput {
        id: fieldInput
        anchors.fill: parent
        anchors.leftMargin: 12
        anchors.rightMargin: 12
        verticalAlignment: TextInput.AlignVCenter
        color: Theme.textStrong
        selectionColor: Theme.accentA(0.4)
        selectedTextColor: Theme.textStrong
        font.pixelSize: field.fontSize
        clip: true
        echoMode: field.password ? TextInput.Password : TextInput.Normal
        onAccepted: field.accepted()
        Accessible.name: field.placeholder
        Text {
            anchors.verticalCenter: parent.verticalCenter
            visible: fieldInput.text.length === 0
            text: field.placeholder
            color: Theme.textSubtle
            font.pixelSize: field.fontSize - 1
        }
    }
}
