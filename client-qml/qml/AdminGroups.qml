import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Server administration: groups (same features as the web console's Groups
// tab). A user sees the computers of the groups they belong to.
Item {
    id: page
    property var groups: []
    property var allUsers: []
    property var allHosts: []
    property bool loading: false
    property string error: ""

    ApiClient { id: api }

    function reload() {
        loading = true;
        api.call("GET", "/api/admin/groups", null, function (ok, data, err) {
            loading = false;
            error = ok ? "" : err;
            if (ok) groups = data || [];
        });
        api.call("GET", "/api/admin/users", null, function (ok, data) { if (ok) allUsers = data || []; });
        api.call("GET", "/api/hosts", null, function (ok, data) { if (ok) allHosts = data || []; });
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
            Text { text: I18n.t("Groups"); font.pixelSize: 22; font.bold: true; color: Theme.textStrong }
            Text { text: I18n.t("Users see only the computers of the groups they are in"); font.pixelSize: 13; color: Theme.textSubtle; Layout.fillWidth: true; elide: Text.ElideRight }
        }
        IconButton { iconName: "refresh"; tip: I18n.t("Refresh"); onClicked: page.reload() }
        PrimaryButton { text: "+  " + I18n.t("Add group"); onClicked: groupDialog.openFor(null) }
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

    GridView {
        id: grid
        anchors.top: errorText.visible ? errorText.bottom : header.bottom
        anchors.topMargin: 16
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        readonly property int cols: Math.max(1, Math.floor(width / 300))
        cellWidth: width / cols
        cellHeight: 150
        model: page.groups
        ScrollBar.vertical: ScrollBar {}
        delegate: Item {
            width: grid.cellWidth
            height: grid.cellHeight
            Rectangle {
                anchors.fill: parent
                anchors.margins: 6
                radius: Theme.radius
                color: Theme.surface
                border.color: cardMa.containsMouse ? Theme.borderStrong : Theme.border
                MouseArea { id: cardMa; anchors.fill: parent; hoverEnabled: true; onClicked: membersDialog.openFor(modelData) }
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 6
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        Icon { name: "grid"; size: 16; color: Theme.accentFg }
                        Text { Layout.fillWidth: true; text: modelData.name; color: Theme.textStrong; font.pixelSize: 15; font.weight: Font.DemiBold; elide: Text.ElideRight }
                        IconButton { iconName: "edit"; tip: I18n.t("Rename"); onClicked: groupDialog.openFor(modelData) }
                        IconButton { iconName: "trash"; tip: I18n.t("Delete"); iconColor: Theme.danger; onClicked: deleteDialog.openFor(modelData) }
                    }
                    Text {
                        Layout.fillWidth: true
                        text: modelData.note ? modelData.note : I18n.t("No description")
                        color: modelData.note ? Theme.textMuted : Theme.textSubtle
                        font.pixelSize: 12
                        elide: Text.ElideRight
                    }
                    Item { Layout.fillHeight: true }
                    RowLayout {
                        spacing: 14
                        RowLayout { spacing: 5; Icon { name: "user"; size: 13; color: Theme.textSubtle } Text { text: I18n.t("%1 users").arg(modelData.user_count); color: Theme.text; font.pixelSize: 12 } }
                        RowLayout { spacing: 5; Icon { name: "monitor"; size: 13; color: Theme.textSubtle } Text { text: I18n.t("%1 computers").arg(modelData.host_count); color: Theme.text; font.pixelSize: 12 } }
                        Item { Layout.fillWidth: true }
                        Text { text: I18n.t("Members") + " →"; color: Theme.accentFg; font.pixelSize: 12; font.weight: Font.DemiBold }
                    }
                }
            }
        }

        Text {
            anchors.centerIn: parent
            visible: !page.loading && page.groups.length === 0 && page.error === ""
            text: I18n.t("No groups yet. Groups decide which computers each user can see.")
            color: Theme.textSubtle
            font.pixelSize: 13
        }
        BusyIndicator { anchors.centerIn: parent; running: page.loading && page.groups.length === 0; visible: running }
    }

    // ---- Create / rename ----
    DialogFrame {
        id: groupDialog
        property var group: null
        property string err: ""
        property bool busy: false
        function openFor(g) {
            group = g; err = ""; busy = false;
            gName.text = g ? g.name : "";
            gNote.text = g ? (g.note || "") : "";
            open();
            gName.input.forceActiveFocus();
        }
        function accept() {
            if (gName.text.trim() === "") { err = I18n.t("Group name is required"); return; }
            busy = true; err = "";
            var body = { name: gName.text.trim(), note: gNote.text.trim() };
            var done = function (ok, data, e) {
                groupDialog.busy = false;
                if (!ok) { groupDialog.err = e; return; }
                groupDialog.close();
                page.reload();
            };
            if (group) api.call("PUT", "/api/admin/groups/" + encodeURIComponent(group.id), body, done);
            else api.call("POST", "/api/admin/groups", body, done);
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: groupDialog.group ? I18n.t("Edit group") : I18n.t("New group"); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text { visible: groupDialog.err !== ""; text: groupDialog.err; color: Theme.danger; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Text { text: I18n.t("Name"); color: Theme.textMuted; font.pixelSize: 12 }
            InputField { id: gName; placeholder: I18n.t("e.g. Office"); onAccepted: gNote.input.forceActiveFocus() }
            Text { text: I18n.t("Description (optional)"); color: Theme.textMuted; font.pixelSize: 12 }
            InputField { id: gNote; placeholder: I18n.t("Description"); onAccepted: groupDialog.accept() }
            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: 4
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: I18n.t("Cancel"); onClicked: groupDialog.close() }
                PrimaryButton { text: groupDialog.group ? I18n.t("Save") : I18n.t("Create"); enabled: !groupDialog.busy; onClicked: groupDialog.accept() }
            }
        }
    }

    // ---- Members ----
    DialogFrame {
        id: membersDialog
        width: Math.min(640, page.width - 40)
        property var group: null
        property var userIds: []
        property var hostIds: []
        property string err: ""
        property bool busy: false
        function openFor(g) {
            group = g; err = ""; busy = false;
            userIds = (g.users || []).map(function (u) { return u.id; });
            hostIds = (g.hosts || []).map(function (h) { return h.id; });
            open();
        }
        function toggled(list, id, on) {
            var out = list.filter(function (x) { return x !== id; });
            if (on) out.push(id);
            return out;
        }
        function accept() {
            busy = true; err = "";
            api.call("PUT", "/api/admin/groups/" + encodeURIComponent(group.id),
                     { name: group.name, note: group.note || "", user_ids: userIds, host_ids: hostIds },
                     function (ok, data, e) {
                membersDialog.busy = false;
                if (!ok) { membersDialog.err = e; return; }
                membersDialog.close();
                page.reload();
            });
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: membersDialog.group ? I18n.t("Members of %1").arg(membersDialog.group.name) : ""; color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold; Layout.fillWidth: true; elide: Text.ElideRight }
            Text { text: I18n.t("Ticked users can see and connect to the ticked computers."); color: Theme.textMuted; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Text { visible: membersDialog.err !== ""; text: membersDialog.err; color: Theme.danger; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            RowLayout {
                Layout.fillWidth: true
                spacing: 12
                Repeater {
                    model: [
                        { title: "Users", kind: "users" },
                        { title: "Computers", kind: "hosts" }
                    ]
                    ColumnLayout {
                        id: memberCol
                        readonly property bool isUsers: modelData.kind === "users"
                        readonly property var items: isUsers ? page.allUsers : page.allHosts
                        readonly property var picked: isUsers ? membersDialog.userIds : membersDialog.hostIds
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1
                        spacing: 6
                        Text { text: I18n.t(modelData.title) + " (" + memberCol.picked.length + "/" + memberCol.items.length + ")"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                        Rectangle {
                            Layout.fillWidth: true
                            Layout.preferredHeight: 260
                            radius: Theme.radiusSm
                            color: Theme.bg
                            border.color: Theme.border
                            Flickable {
                                anchors.fill: parent
                                anchors.margins: 6
                                contentHeight: itemsCol.implicitHeight
                                clip: true
                                boundsBehavior: Flickable.StopAtBounds
                                ScrollBar.vertical: ScrollBar {}
                                Column {
                                    id: itemsCol
                                    width: parent.width
                                    Repeater {
                                        model: memberCol.items
                                        CheckRow {
                                            width: itemsCol.width
                                            text: memberCol.isUsers ? modelData.username : (modelData.id && modelData.id !== modelData.name ? modelData.name + "  ·  " + modelData.id : modelData.name)
                                            checked: memberCol.picked.indexOf(modelData.id) >= 0
                                            onToggled: {
                                                if (memberCol.isUsers) membersDialog.userIds = membersDialog.toggled(membersDialog.userIds, modelData.id, checked);
                                                else membersDialog.hostIds = membersDialog.toggled(membersDialog.hostIds, modelData.id, checked);
                                            }
                                        }
                                    }
                                }
                            }
                            Text {
                                anchors.centerIn: parent
                                visible: memberCol.items.length === 0
                                text: memberCol.isUsers ? I18n.t("No users") : I18n.t("No computers")
                                color: Theme.textSubtle
                                font.pixelSize: 12
                            }
                        }
                    }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: 4
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: I18n.t("Cancel"); onClicked: membersDialog.close() }
                PrimaryButton { text: I18n.t("Save"); enabled: !membersDialog.busy; onClicked: membersDialog.accept() }
            }
        }
    }

    // ---- Delete ----
    DialogFrame {
        id: deleteDialog
        property var group: null
        property string err: ""
        function openFor(g) { group = g; err = ""; open(); }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: I18n.t("Delete group?"); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text {
                Layout.fillWidth: true
                text: deleteDialog.group ? I18n.t("The group %1 will be deleted. Its users lose access to its computers.").arg(deleteDialog.group.name) : ""
                color: Theme.textMuted
                font.pixelSize: 13
                wrapMode: Text.WordWrap
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
                    onClicked: api.call("DELETE", "/api/admin/groups/" + encodeURIComponent(deleteDialog.group.id), null, function (ok, data, e) {
                        if (!ok) { deleteDialog.err = e; return; }
                        deleteDialog.close();
                        page.reload();
                    })
                }
            }
        }
    }
}
