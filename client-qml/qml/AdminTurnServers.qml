import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Server administration: STUN/TURN relays handed to every connection (same
// features as the web console's TURN servers tab).
Item {
    id: page
    property var servers: []
    property bool loading: false
    property string error: ""

    ApiClient { id: api }

    function reload() {
        loading = true;
        api.call("GET", "/api/admin/turn-servers", null, function (ok, data, err) {
            loading = false;
            error = ok ? "" : err;
            if (ok) servers = data || [];
        });
    }
    Component.onCompleted: reload()

    RowLayout {
        id: header
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: 10
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 4
            Text { text: I18n.t("TURN servers"); font.pixelSize: 22; font.bold: true; color: Theme.textStrong }
            Text { text: I18n.t("Relays used when two computers cannot connect directly"); font.pixelSize: 13; color: Theme.textSubtle; Layout.fillWidth: true; elide: Text.ElideRight }
        }
        IconButton { iconName: "refresh"; tip: I18n.t("Refresh"); onClicked: page.reload() }
        PrimaryButton { text: "+  " + I18n.t("Add server"); onClicked: createDialog.openNew() }
    }

    Text {
        id: errorText
        anchors.top: header.bottom
        anchors.topMargin: 12
        anchors.left: parent.left
        anchors.right: parent.right
        visible: page.error !== ""
        text: page.error
        color: Theme.danger
        font.pixelSize: 13
        wrapMode: Text.WordWrap
    }

    Rectangle {
        id: table
        anchors.top: errorText.visible ? errorText.bottom : header.bottom
        anchors.topMargin: 16
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        radius: Theme.radius
        color: Theme.surface
        border.color: Theme.border
        clip: true
        readonly property real flex: Math.max(200, width - 28 - 90 - 34 - 3 * 12)
        readonly property real wUrl: flex * 0.65
        readonly property real wUser: flex * 0.35

        Rectangle {
            id: thead
            width: parent.width
            height: 38
            color: Theme.surface2
            radius: Theme.radius
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: Theme.radius; color: Theme.surface2 }
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.border }
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 16
                anchors.rightMargin: 12
                spacing: 12
                Text { Layout.preferredWidth: table.wUrl; text: I18n.t("Address"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Text { Layout.preferredWidth: table.wUser; text: I18n.t("Username"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Text { Layout.preferredWidth: 90; text: I18n.t("Password"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Item { Layout.preferredWidth: 34 }
            }
        }

        ListView {
            id: list
            anchors.top: thead.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            clip: true
            model: page.servers
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar {}
            delegate: Rectangle {
                width: list.width
                height: 50
                color: rowMa.containsMouse ? Theme.surface2 : "transparent"
                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.border; visible: index < list.count - 1 }
                MouseArea { id: rowMa; anchors.fill: parent; hoverEnabled: true; acceptedButtons: Qt.NoButton }
                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 16
                    anchors.rightMargin: 12
                    spacing: 12
                    RowLayout {
                        Layout.preferredWidth: table.wUrl
                        Layout.maximumWidth: table.wUrl
                        spacing: 8
                        Rectangle {
                            implicitWidth: kindText.implicitWidth + 12; implicitHeight: 20; radius: 10
                            color: String(modelData.urls).indexOf("stun:") === 0 ? Theme.surface3 : Theme.accentA(0.15)
                            Text { id: kindText; anchors.centerIn: parent; text: String(modelData.urls).split(":")[0].toUpperCase(); font.pixelSize: 10; font.weight: Font.DemiBold; color: String(modelData.urls).indexOf("stun:") === 0 ? Theme.textMuted : Theme.accentFg }
                        }
                        Text { Layout.fillWidth: true; Layout.preferredWidth: 10; text: modelData.urls; color: Theme.textStrong; font.pixelSize: 13; font.family: "monospace"; elide: Text.ElideRight }
                    }
                    Text { Layout.preferredWidth: table.wUser; Layout.maximumWidth: table.wUser; text: modelData.username || "—"; color: modelData.username ? Theme.text : Theme.textSubtle; font.pixelSize: 13; elide: Text.ElideRight }
                    Text { Layout.preferredWidth: 90; text: modelData.credential ? "••••••" : "—"; color: Theme.textSubtle; font.pixelSize: 13 }
                    IconButton { Layout.preferredWidth: 34; iconName: "trash"; tip: I18n.t("Delete"); iconColor: Theme.danger; onClicked: deleteDialog.openFor(modelData) }
                }
            }
        }

        Text {
            anchors.centerIn: parent
            width: parent.width - 40
            horizontalAlignment: Text.AlignHCenter
            visible: !page.loading && page.servers.length === 0 && page.error === ""
            text: I18n.t("No TURN servers. Connections use public STUN only, which can fail behind strict networks.")
            color: Theme.textSubtle
            font.pixelSize: 13
            wrapMode: Text.WordWrap
        }
        BusyIndicator { anchors.centerIn: parent; running: page.loading && page.servers.length === 0; visible: running }
    }

    DialogFrame {
        id: createDialog
        width: 440
        property string err: ""
        property bool busy: false
        function openNew() {
            tUrl.text = ""; tUser.text = ""; tCred.text = ""; err = ""; busy = false;
            open();
            tUrl.input.forceActiveFocus();
        }
        function accept() {
            var urls = tUrl.text.trim();
            if (urls === "") { err = I18n.t("Server URL is required"); return; }
            if (!/^(stun|turns?):/.test(urls)) { err = I18n.t("The address must start with stun:, turn: or turns: (e.g. turn:turn.example.com:3478)"); return; }
            var body = { urls: urls };
            if (tUser.text.trim() !== "") body.username = tUser.text.trim();
            if (tCred.text !== "") body.credential = tCred.text;
            busy = true; err = "";
            api.call("POST", "/api/admin/turn-servers", body, function (ok, data, e) {
                createDialog.busy = false;
                if (!ok) { createDialog.err = e; return; }
                createDialog.close();
                page.reload();
            });
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: I18n.t("Add TURN/STUN server"); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text { visible: createDialog.err !== ""; text: createDialog.err; color: Theme.danger; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Text { text: I18n.t("Address"); color: Theme.textMuted; font.pixelSize: 12 }
            InputField { id: tUrl; placeholder: "turn:turn.example.com:3478"; onAccepted: tUser.input.forceActiveFocus() }
            Text { text: I18n.t("Username (optional)"); color: Theme.textMuted; font.pixelSize: 12 }
            InputField { id: tUser; placeholder: I18n.t("Username"); onAccepted: tCred.input.forceActiveFocus() }
            Text { text: I18n.t("Password (optional)"); color: Theme.textMuted; font.pixelSize: 12 }
            InputField { id: tCred; placeholder: I18n.t("Password"); password: true; onAccepted: createDialog.accept() }
            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: 4
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: I18n.t("Cancel"); onClicked: createDialog.close() }
                PrimaryButton { text: I18n.t("Add"); enabled: !createDialog.busy; onClicked: createDialog.accept() }
            }
        }
    }

    DialogFrame {
        id: deleteDialog
        property var server: null
        property string err: ""
        function openFor(s) { server = s; err = ""; open(); }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: I18n.t("Delete server?"); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text {
                Layout.fillWidth: true
                text: deleteDialog.server ? I18n.t("%1 will no longer be offered to new connections.").arg(deleteDialog.server.urls) : ""
                color: Theme.textMuted
                font.pixelSize: 13
                wrapMode: Text.WrapAnywhere
            }
            Text { visible: deleteDialog.err !== ""; text: deleteDialog.err; color: Theme.danger; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: I18n.t("Cancel"); onClicked: deleteDialog.close() }
                SecondaryButton {
                    text: I18n.t("Delete")
                    danger: true
                    onClicked: api.call("DELETE", "/api/admin/turn-servers/" + encodeURIComponent(deleteDialog.server.id), null, function (ok, data, e) {
                        if (!ok) { deleteDialog.err = e; return; }
                        deleteDialog.close();
                        page.reload();
                    })
                }
            }
        }
    }
}
