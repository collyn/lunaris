import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Server administration: user accounts (same features as the web console's
// Users tab): list, create, edit role/status/password/groups, delete.
Item {
    id: page
    property string currentUser: ""
    property var users: []
    property var groups: []
    property bool loading: false
    property string error: ""

    ApiClient { id: api }

    function reload() {
        loading = true;
        api.call("GET", "/api/admin/users", null, function (ok, data, err) {
            loading = false;
            error = ok ? "" : err;
            if (ok) users = data || [];
        });
        api.call("GET", "/api/admin/groups", null, function (ok, data) {
            if (ok) groups = data || [];
        });
    }
    Component.onCompleted: reload()

    function fmtDate(s) {
        if (!s) return "—";
        var d = new Date(String(s).replace(" ", "T"));
        return isNaN(d.getTime()) ? String(s) : d.toLocaleDateString(Qt.locale(), Locale.ShortFormat);
    }

    // ---- Header ----
    RowLayout {
        id: header
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: 10
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 4
            Text { text: I18n.t("Users"); font.pixelSize: 22; font.bold: true; color: Theme.textStrong }
            Text { text: I18n.t("Create, edit, and manage user accounts and their permissions"); font.pixelSize: 13; color: Theme.textSubtle; Layout.fillWidth: true; elide: Text.ElideRight }
        }
        IconButton { iconName: "refresh"; tip: I18n.t("Refresh"); onClicked: page.reload() }
        PrimaryButton { text: "+  " + I18n.t("Add user"); onClicked: createDialog.openNew() }
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

    // ---- Table ----
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

        readonly property real wRole: 90
        readonly property real wStatus: 110
        readonly property real wCreated: 110
        readonly property real wActions: 76
        // What is left after the fixed columns, the margins and the gaps.
        readonly property real flex: Math.max(160, width - 28 - wRole - wStatus - wCreated - wActions - 5 * 12)
        readonly property real wName: flex * 0.4
        readonly property real wGroups: flex * 0.6

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
                Text { Layout.preferredWidth: table.wName; text: I18n.t("Username"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Text { Layout.preferredWidth: table.wRole; text: I18n.t("Role"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Text { Layout.preferredWidth: table.wStatus; text: I18n.t("Status"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Text { Layout.preferredWidth: table.wGroups; text: I18n.t("Groups"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Text { Layout.preferredWidth: table.wCreated; text: I18n.t("Created"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.DemiBold }
                Item { Layout.preferredWidth: table.wActions }
            }
        }

        ListView {
            id: list
            anchors.top: thead.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            clip: true
            model: page.users
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar {}
            delegate: Rectangle {
                width: list.width
                height: 52
                color: rowMa.containsMouse ? Theme.surface2 : "transparent"
                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.border; visible: index < list.count - 1 }
                MouseArea { id: rowMa; anchors.fill: parent; hoverEnabled: true; acceptedButtons: Qt.NoButton }
                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 16
                    anchors.rightMargin: 12
                    spacing: 12
                    RowLayout {
                        Layout.preferredWidth: table.wName
                        Layout.maximumWidth: table.wName
                        spacing: 8
                        Icon { name: "user"; size: 15; color: Theme.textSubtle }
                        Text { Layout.fillWidth: true; Layout.preferredWidth: 10; text: modelData.username; color: Theme.textStrong; font.pixelSize: 14; font.weight: Font.DemiBold; elide: Text.ElideRight }
                    }
                    Item {
                        Layout.preferredWidth: table.wRole
                        implicitHeight: 22
                        Rectangle {
                            height: 22
                            width: roleText.implicitWidth + 16
                            radius: 11
                            anchors.verticalCenter: parent.verticalCenter
                            color: modelData.role === "admin" ? Theme.accentA(0.15) : Theme.surface3
                            Text { id: roleText; anchors.centerIn: parent; text: modelData.role === "admin" ? I18n.t("Admin") : I18n.t("User"); color: modelData.role === "admin" ? Theme.accentFg : Theme.textMuted; font.pixelSize: 11; font.weight: Font.DemiBold }
                        }
                    }
                    RowLayout {
                        Layout.preferredWidth: table.wStatus
                        spacing: 6
                        Rectangle { width: 8; height: 8; radius: 4; color: modelData.is_active ? Theme.success : Theme.textSubtle }
                        Text { text: modelData.is_active ? I18n.t("Active") : I18n.t("Disabled"); color: Theme.text; font.pixelSize: 13 }
                        Item { Layout.fillWidth: true }
                    }
                    Text {
                        Layout.preferredWidth: table.wGroups
                        Layout.maximumWidth: table.wGroups
                        text: (modelData.groups && modelData.groups.length) ? modelData.groups.map(function (g) { return g.name; }).join(", ") : "—"
                        color: (modelData.groups && modelData.groups.length) ? Theme.text : Theme.textSubtle
                        font.pixelSize: 13
                        elide: Text.ElideRight
                    }
                    Text { Layout.preferredWidth: table.wCreated; text: page.fmtDate(modelData.created_at); color: Theme.textSubtle; font.pixelSize: 12; font.family: "monospace" }
                    Row {
                        Layout.preferredWidth: table.wActions
                        spacing: 4
                        IconButton { iconName: "edit"; tip: I18n.t("Edit"); onClicked: editDialog.openFor(modelData) }
                        IconButton {
                            iconName: "trash"
                            tip: I18n.t("Delete")
                            iconColor: Theme.danger
                            enabled: modelData.username !== page.currentUser
                            opacity: enabled ? 1 : 0.35
                            onClicked: deleteDialog.openFor(modelData)
                        }
                    }
                }
            }
        }

        Text {
            anchors.centerIn: parent
            visible: !page.loading && page.users.length === 0 && page.error === ""
            text: I18n.t("No users yet. Click \"Add user\" to create one.")
            color: Theme.textSubtle
            font.pixelSize: 13
        }
        BusyIndicator { anchors.centerIn: parent; running: page.loading && page.users.length === 0; visible: running }
    }

    // ---- Create ----
    DialogFrame {
        id: createDialog
        property string err: ""
        property bool busy: false
        property string role: "user"
        function openNew() {
            cUser.text = ""; cPass.text = ""; role = "user"; err = ""; busy = false;
            open();
            cUser.input.forceActiveFocus();
        }
        function accept() {
            if (cUser.text.trim() === "" || cPass.text === "") { err = I18n.t("Username and password are required"); return; }
            busy = true; err = "";
            api.call("POST", "/api/admin/users", { username: cUser.text.trim(), password: cPass.text, role: role }, function (ok, data, e) {
                createDialog.busy = false;
                if (!ok) { createDialog.err = e; return; }
                createDialog.close();
                page.reload();
            });
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: I18n.t("New user"); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text { visible: createDialog.err !== ""; text: createDialog.err; color: Theme.danger; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Text { text: I18n.t("Username"); color: Theme.textMuted; font.pixelSize: 12 }
            InputField { id: cUser; placeholder: I18n.t("Username"); onAccepted: cPass.input.forceActiveFocus() }
            Text { text: I18n.t("Password"); color: Theme.textMuted; font.pixelSize: 12 }
            InputField { id: cPass; placeholder: I18n.t("At least 6 characters"); password: true; onAccepted: createDialog.accept() }
            Text { text: I18n.t("Role"); color: Theme.textMuted; font.pixelSize: 12 }
            ThemedCombo {
                Layout.fillWidth: true
                options: [{ value: "user", label: "User" }, { value: "admin", label: "Admin" }]
                value: createDialog.role
                onPicked: (v) => createDialog.role = v
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.topMargin: 4
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: I18n.t("Cancel"); onClicked: createDialog.close() }
                PrimaryButton { text: I18n.t("Create"); enabled: !createDialog.busy; onClicked: createDialog.accept() }
            }
        }
    }

    // ---- Edit ----
    DialogFrame {
        id: editDialog
        width: 440
        property var user: null
        property string role: "user"
        property bool active: true
        property var groupIds: []
        property string err: ""
        property bool busy: false
        function openFor(u) {
            user = u; role = u.role; active = u.is_active;
            groupIds = (u.groups || []).map(function (g) { return g.id; });
            ePass.text = ""; err = ""; busy = false;
            open();
        }
        function toggleGroup(id, on) {
            var ids = groupIds.filter(function (x) { return x !== id; });
            if (on) ids.push(id);
            groupIds = ids;
        }
        function accept() {
            var body = { role: role, is_active: active, group_ids: groupIds };
            if (ePass.text !== "") body.password = ePass.text;
            busy = true; err = "";
            api.call("PUT", "/api/admin/users/" + encodeURIComponent(user.id), body, function (ok, data, e) {
                editDialog.busy = false;
                if (!ok) { editDialog.err = e; return; }
                editDialog.close();
                page.reload();
            });
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: editDialog.user ? I18n.t("Edit user %1").arg(editDialog.user.username) : ""; color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold; Layout.fillWidth: true; elide: Text.ElideRight }
            Text { visible: editDialog.err !== ""; text: editDialog.err; color: Theme.danger; font.pixelSize: 12; wrapMode: Text.WordWrap; Layout.fillWidth: true }
            Text { text: I18n.t("Role"); color: Theme.textMuted; font.pixelSize: 12 }
            ThemedCombo {
                Layout.fillWidth: true
                options: [{ value: "user", label: "User" }, { value: "admin", label: "Admin" }]
                value: editDialog.role
                onPicked: (v) => editDialog.role = v
            }
            CheckRow { text: I18n.t("Account active (can sign in)"); checked: editDialog.active; onToggled: editDialog.active = checked }
            Text { text: I18n.t("New password (leave blank to keep the current one)"); color: Theme.textMuted; font.pixelSize: 12; Layout.fillWidth: true; wrapMode: Text.WordWrap }
            InputField { id: ePass; placeholder: I18n.t("New password"); password: true; onAccepted: editDialog.accept() }
            Text { visible: page.groups.length > 0; text: I18n.t("Groups"); color: Theme.textMuted; font.pixelSize: 12 }
            Rectangle {
                visible: page.groups.length > 0
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(160, groupCol.implicitHeight + 12)
                radius: Theme.radiusSm
                color: Theme.bg
                border.color: Theme.border
                Flickable {
                    anchors.fill: parent
                    anchors.margins: 6
                    contentHeight: groupCol.implicitHeight
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    Column {
                        id: groupCol
                        width: parent.width
                        Repeater {
                            model: page.groups
                            CheckRow {
                                width: groupCol.width
                                text: modelData.name
                                checked: editDialog.groupIds.indexOf(modelData.id) >= 0
                                onToggled: editDialog.toggleGroup(modelData.id, checked)
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
                SecondaryButton { text: I18n.t("Cancel"); onClicked: editDialog.close() }
                PrimaryButton { text: I18n.t("Save"); enabled: !editDialog.busy; onClicked: editDialog.accept() }
            }
        }
    }

    // ---- Delete ----
    DialogFrame {
        id: deleteDialog
        property var user: null
        property string err: ""
        function openFor(u) { user = u; err = ""; open(); }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: I18n.t("Delete user?"); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text {
                Layout.fillWidth: true
                text: deleteDialog.user ? I18n.t("The account %1 will be deleted. This cannot be undone.").arg(deleteDialog.user.username) : ""
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
                    onClicked: api.call("DELETE", "/api/admin/users/" + encodeURIComponent(deleteDialog.user.id), null, function (ok, data, e) {
                        if (!ok) { deleteDialog.err = e; return; }
                        deleteDialog.close();
                        page.reload();
                    })
                }
            }
        }
    }
}
