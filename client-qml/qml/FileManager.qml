import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// File manager for one remote computer (RustDesk-style): this computer on the
// left, the remote one on the right, copy either way (files and folders),
// new folder / rename / delete on both sides, and the transfer queue below.
// Runs on a connection of its own (files.rs), stream or not.
Rectangle {
    id: fm
    color: Theme.bg
    visible: false

    // From files.rs (state_json): connection, remote folder, transfers.
    property var st: ({ connected: false, status: "", error: "", jobs: [] })
    property int seenVersion: -1
    property var places: ({ roots: [], home: "", sep: "/" })
    property var local: ({ path: "", parent: null, entries: [] })
    property string localError: ""
    property bool showHidden: false
    property var localSel: ({})
    property var remoteSel: ({})

    readonly property var remote: st.remote || ({ path: "", parent: null, entries: [] })
    readonly property var remoteRoots: (st.hello && st.hello.roots) ? st.hello.roots : []
    readonly property string remoteSep: (st.hello && st.hello.sep) ? st.hello.sep : "/"

    /// Opens for a computer of the server list (or direct: serverUrl ws://addr).
    function openFor(serverUrl, token, hostId, hostName) {
        bridge.filesOpen(serverUrl, token, hostId, hostName);
        show();
    }
    /// Opens for the computer being streamed.
    function openCurrent() {
        if (!bridge.filesOpenCurrent()) return false;
        show();
        return true;
    }
    function show() {
        places = JSON.parse(bridge.filesLocalPlaces());
        if (local.path === "") loadLocal(places.home);
        localSel = ({}); remoteSel = ({});
        seenVersion = -1;
        visible = true;
        forceActiveFocus();
    }
    function close() {
        visible = false;
        // Transfers keep going; the connection closes when nothing is left.
        if (!(st.active > 0)) bridge.filesClose();
    }

    function joinLocal(dir, name) { return (dir.endsWith("/") || dir.endsWith("\\")) ? dir + name : dir + places.sep + name; }
    function joinRemote(dir, name) { return (dir.endsWith("/") || dir.endsWith("\\")) ? dir + name : dir + remoteSep + name; }

    function loadLocal(path) {
        var d = JSON.parse(bridge.filesListLocal(path, showHidden));
        if (d.error) { localError = d.error; return; }
        localError = "";
        local = d;
        localSel = ({});
    }
    function loadRemote(path) { remoteSel = ({}); bridge.filesListRemote(path, showHidden); }

    function sorted(entries) {
        var list = (entries || []).slice();
        list.sort(function (a, b) {
            if (a.dir !== b.dir) return a.dir ? -1 : 1;
            return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
        });
        return list;
    }
    function names(sel) { return Object.keys(sel).filter(function (k) { return sel[k]; }); }
    function human(b) {
        if (b >= 1073741824) return (b / 1073741824).toFixed(1) + " GB";
        if (b >= 1048576) return (b / 1048576).toFixed(1) + " MB";
        if (b >= 1024) return Math.round(b / 1024) + " KB";
        return b + " B";
    }
    function when(s) { return s ? new Date(s * 1000).toLocaleString(Qt.locale(), Locale.ShortFormat) : ""; }

    function upload() {
        var sel = names(localSel);
        if (!sel.length || !remote.path) return;
        bridge.filesUpload(sel.map(function (n) { return joinLocal(local.path, n); }).join("\n"), remote.path);
    }
    function download() {
        var sel = names(remoteSel);
        if (!sel.length || !local.path) return;
        bridge.filesDownload(sel.join("\n"), remote.path, local.path);
    }

    Timer {
        interval: 120
        repeat: true
        running: fm.visible
        triggeredOnStart: true
        onTriggered: {
            var v = bridge.filesVersion();
            if (v === fm.seenVersion) return;
            fm.seenVersion = v;
            var wasActive = fm.st.active || 0;
            try { fm.st = JSON.parse(bridge.filesState()); } catch (e) {}
            // Downloads finished: show them in the local folder.
            if (wasActive > 0 && !(fm.st.active > 0) && fm.local.path !== "") fm.loadLocal(fm.local.path);
        }
    }

    Keys.onEscapePressed: if (!ask.visible) fm.close()
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (wheel) => wheel.accepted = true }

    // ---------------- a name prompt / confirmation
    Rectangle {
        id: ask
        property string title: ""
        property string message: ""
        property bool withName: true
        property var done: null
        function prompt(t, initial, cb) { title = t; message = ""; withName = true; nameField.text = initial; done = cb; visible = true; nameField.input.forceActiveFocus(); nameField.input.selectAll(); }
        function confirm(t, msg, cb) { title = t; message = msg; withName = false; done = cb; visible = true; forceActiveFocus(); }
        function finish(ok) { visible = false; if (ok && done) done(nameField.text.trim()); done = null; fm.forceActiveFocus(); }
        visible: false
        anchors.fill: parent
        z: 50
        color: Qt.rgba(0, 0, 0, 0.45)
        Keys.onEscapePressed: ask.finish(false)
        MouseArea { anchors.fill: parent; onClicked: ask.finish(false) }
        Rectangle {
            anchors.centerIn: parent
            width: Math.min(420, parent.width - 32)
            height: askCol.implicitHeight + 40
            radius: Theme.radiusLg
            color: Theme.surface
            border.color: Theme.border
            MouseArea { anchors.fill: parent }
            ColumnLayout {
                id: askCol
                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                anchors.margins: 20
                spacing: 12
                Text { text: ask.title; color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold; Layout.fillWidth: true; wrapMode: Text.WordWrap }
                Text { visible: ask.message !== ""; text: ask.message; color: Theme.textMuted; font.pixelSize: 13; Layout.fillWidth: true; wrapMode: Text.WordWrap }
                InputField { id: nameField; visible: ask.withName; Layout.fillWidth: true; onAccepted: ask.finish(true) }
                RowLayout {
                    Layout.alignment: Qt.AlignRight
                    spacing: 8
                    SecondaryButton { text: I18n.t("Cancel"); onClicked: ask.finish(false) }
                    PrimaryButton { text: ask.withName ? I18n.t("OK") : I18n.t("Delete"); onClicked: ask.finish(true) }
                }
            }
        }
    }

    // ---------------- header
    Rectangle {
        id: header
        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
        height: 56
        color: Theme.surface
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 1; color: Theme.border }
        Row {
            anchors.left: parent.left; anchors.leftMargin: 22
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10
            Text { text: I18n.t("Files"); color: Theme.textStrong; font.pixelSize: 18; font.weight: Font.DemiBold; anchors.verticalCenter: parent.verticalCenter }
            Rectangle {
                visible: (fm.st.host || "") !== ""
                anchors.verticalCenter: parent.verticalCenter
                radius: 10; height: 22; width: hostText.implicitWidth + 16
                color: Theme.surface2; border.color: Theme.border
                Text { id: hostText; anchors.centerIn: parent; text: fm.st.host || ""; color: Theme.textMuted; font.pixelSize: 12 }
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: fm.st.status ? I18n.t(fm.st.status) : ""
                color: Theme.textMuted; font.pixelSize: 13
            }
        }
        Row {
            anchors.right: parent.right; anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            CheckRow { text: I18n.t("Hidden files"); checked: fm.showHidden; onToggled: { fm.showHidden = checked; fm.loadLocal(fm.local.path); if (fm.remote.path) fm.loadRemote(fm.remote.path); } }
            IconButton { iconName: "close"; tip: I18n.t("Close"); onClicked: fm.close() }
        }
    }

    // ---------------- one side
    component Pane: Rectangle {
        id: pane
        property string title: ""
        property var listing: ({ path: "", parent: null, entries: [] })
        property var roots: []
        property var sel: ({})
        property string error: ""
        property bool enabledPane: true
        property string emptyText: ""
        signal navigate(string path)
        signal selectionChanged(var sel)
        signal activated(var entry)
        default property alias tools: toolRow.data
        readonly property var rows: fm.sorted(listing.entries)
        property int anchorIndex: -1

        radius: Theme.radius
        color: Theme.surface
        border.color: Theme.border

        function clickRow(i, mods) {
            var name = rows[i].name;
            var s = {};
            if (mods & Qt.ControlModifier) { for (var k in pane.sel) s[k] = pane.sel[k]; s[name] = !s[name]; anchorIndex = i; }
            else if ((mods & Qt.ShiftModifier) && anchorIndex >= 0) { for (var j = Math.min(anchorIndex, i); j <= Math.max(anchorIndex, i); j++) s[rows[j].name] = true; }
            else { s[name] = true; anchorIndex = i; }
            pane.selectionChanged(s);
        }

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 12
            spacing: 8
            RowLayout {
                Layout.fillWidth: true
                spacing: 6
                Text { text: pane.title; color: Theme.textStrong; font.pixelSize: 14; font.weight: Font.DemiBold }
                Item { Layout.fillWidth: true }
                ThemedCombo {
                    implicitWidth: 150; implicitHeight: 32
                    enabled: pane.enabledPane
                    // "Places" stays shown; picking one opens it.
                    options: [{ value: "", label: "Places" }].concat(pane.roots.map(function (r) { return { value: r.path, label: r.name }; }))
                    value: ""
                    onPicked: (v) => { if (v !== "") pane.navigate(v); currentIndex = 0; }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 4
                IconButton { iconName: "arrowUp"; tip: I18n.t("Up one folder"); enabled: pane.enabledPane && !!pane.listing.parent; opacity: enabled ? 1 : 0.4; onClicked: pane.navigate(pane.listing.parent) }
                IconButton { iconName: "refresh"; tip: I18n.t("Refresh"); enabled: pane.enabledPane; onClicked: pane.navigate(pane.listing.path) }
                InputField {
                    id: pathField
                    Layout.fillWidth: true
                    implicitHeight: 32
                    fontSize: 13
                    text: pane.listing.path || ""
                    onAccepted: pane.navigate(text)
                }
            }
            RowLayout { id: toolRow; Layout.fillWidth: true; spacing: 6 }
            Text { visible: pane.error !== ""; text: pane.error; color: Theme.danger; font.pixelSize: 12; Layout.fillWidth: true; wrapMode: Text.WordWrap }
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                radius: Theme.radiusSm
                color: Theme.bg
                border.color: Theme.border
                clip: true
                ListView {
                    id: list
                    anchors.fill: parent
                    anchors.margins: 1
                    model: pane.rows
                    boundsBehavior: Flickable.StopAtBounds
                    ScrollBar.vertical: ScrollBar {}
                    delegate: Rectangle {
                        required property var modelData
                        required property int index
                        width: list.width
                        height: 30
                        color: pane.sel[modelData.name] ? Theme.accentA(0.18) : (rowMouse.containsMouse ? Theme.surface2 : "transparent")
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 10; anchors.rightMargin: 10
                            spacing: 8
                            Icon { name: modelData.dir ? "folder" : "file"; size: 15; color: modelData.dir ? "#e3b341" : Theme.textMuted }
                            Text { text: modelData.name; color: Theme.textStrong; opacity: modelData.hidden ? 0.6 : 1; font.pixelSize: 13; elide: Text.ElideMiddle; Layout.fillWidth: true }
                            Text { text: modelData.dir ? "" : fm.human(modelData.size); color: Theme.textMuted; font.pixelSize: 12; Layout.preferredWidth: 70; horizontalAlignment: Text.AlignRight }
                            Text { visible: pane.width > 420; text: fm.when(modelData.mtime); color: Theme.textMuted; font.pixelSize: 12; Layout.preferredWidth: 130; elide: Text.ElideRight }
                        }
                        MouseArea {
                            id: rowMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            onClicked: (mouse) => pane.clickRow(index, mouse.modifiers)
                            onDoubleClicked: pane.activated(modelData)
                        }
                    }
                    Text {
                        anchors.centerIn: parent
                        width: parent.width - 40
                        visible: list.count === 0
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        text: pane.emptyText
                        color: Theme.textMuted
                        font.pixelSize: 13
                    }
                }
            }
        }
    }

    // ---------------- body
    Item {
        id: body
        anchors.left: parent.left; anchors.right: parent.right
        anchors.top: header.bottom; anchors.bottom: transfers.top
        anchors.margins: 14

        Pane {
            id: localPane
            anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom
            width: (parent.width - middle.width - 20) / 2
            title: I18n.t("This computer")
            listing: fm.local
            roots: fm.places.roots
            sel: fm.localSel
            error: fm.localError
            emptyText: I18n.t("This folder is empty")
            onNavigate: (p) => fm.loadLocal(p)
            onSelectionChanged: (s) => fm.localSel = s
            onActivated: (e) => { if (e.dir) fm.loadLocal(fm.joinLocal(fm.local.path, e.name)); }
            SecondaryButton { text: I18n.t("New folder"); onClicked: ask.prompt(I18n.t("New folder name"), I18n.t("New folder"), function (n) { if (n) { var err = bridge.filesLocalOp("mkdir", fm.joinLocal(fm.local.path, n), ""); fm.localError = err; fm.loadLocal(fm.local.path); } }) }
            SecondaryButton { text: I18n.t("Rename"); enabled: fm.names(fm.localSel).length === 1; onClicked: { var old = fm.names(fm.localSel)[0]; ask.prompt(I18n.t("Rename to"), old, function (n) { if (n && n !== old) { fm.localError = bridge.filesLocalOp("rename", fm.joinLocal(fm.local.path, old), n); fm.loadLocal(fm.local.path); } }); } }
            SecondaryButton { text: I18n.t("Delete"); danger: true; enabled: fm.names(fm.localSel).length > 0; onClicked: { var list = fm.names(fm.localSel); ask.confirm(I18n.t("Delete %1 item(s) on this computer?").arg(list.length), list.slice(0, 6).join("\n") + (list.length > 6 ? "\n…" : ""), function () { for (var i = 0; i < list.length; i++) { var err = bridge.filesLocalOp("remove", fm.joinLocal(fm.local.path, list[i]), ""); if (err) { fm.localError = err; break; } } fm.loadLocal(fm.local.path); }); } }
            Item { Layout.fillWidth: true }
        }

        // Copy buttons between the panes.
        Column {
            id: middle
            anchors.centerIn: parent
            width: 120
            spacing: 10
            PrimaryButton {
                width: parent.width
                text: I18n.t("Upload") + " →"
                enabled: fm.st.connected && fm.names(fm.localSel).length > 0
                onClicked: fm.upload()
            }
            SecondaryButton {
                width: parent.width
                text: "← " + I18n.t("Download")
                enabled: fm.st.connected && fm.names(fm.remoteSel).length > 0
                onClicked: fm.download()
            }
        }

        Pane {
            id: remotePane
            anchors.right: parent.right; anchors.top: parent.top; anchors.bottom: parent.bottom
            width: localPane.width
            title: (fm.st.host || I18n.t("Remote computer"))
            listing: fm.remote
            roots: fm.remoteRoots
            sel: fm.remoteSel
            enabledPane: fm.st.connected
            error: fm.st.error ? I18n.t(fm.st.error) : ""
            emptyText: fm.st.connected ? I18n.t("This folder is empty. Drop files here to upload them.") : (fm.st.status ? I18n.t(fm.st.status) : "")
            onNavigate: (p) => fm.loadRemote(p)
            onSelectionChanged: (s) => fm.remoteSel = s
            onActivated: (e) => { if (e.dir) fm.loadRemote(fm.joinRemote(fm.remote.path, e.name)); }
            SecondaryButton { text: I18n.t("New folder"); enabled: fm.st.connected; onClicked: ask.prompt(I18n.t("New folder name"), I18n.t("New folder"), function (n) { if (n) bridge.filesRemoteOp("mkdir", fm.joinRemote(fm.remote.path, n), ""); }) }
            SecondaryButton { text: I18n.t("Rename"); enabled: fm.st.connected && fm.names(fm.remoteSel).length === 1; onClicked: { var old = fm.names(fm.remoteSel)[0]; ask.prompt(I18n.t("Rename to"), old, function (n) { if (n && n !== old) bridge.filesRemoteOp("rename", fm.joinRemote(fm.remote.path, old), n); }); } }
            SecondaryButton { text: I18n.t("Delete"); danger: true; enabled: fm.st.connected && fm.names(fm.remoteSel).length > 0; onClicked: { var list = fm.names(fm.remoteSel); ask.confirm(I18n.t("Delete %1 item(s) on %2? This cannot be undone.").arg(list.length).arg(fm.st.host || ""), list.slice(0, 6).join("\n") + (list.length > 6 ? "\n…" : ""), function () { for (var i = 0; i < list.length; i++) bridge.filesRemoteOp("remove", fm.joinRemote(fm.remote.path, list[i]), ""); fm.remoteSel = ({}); }); } }
            Item { Layout.fillWidth: true }
        }
        // Files dropped on the remote side are uploaded into its folder.
        DropArea {
            anchors.fill: remotePane
            keys: ["text/uri-list"]
            enabled: fm.st.connected
            onDropped: (drop) => {
                if (!drop.hasUrls) return;
                bridge.filesUpload(drop.urls.map(function (u) { return u.toString(); }).join("\n"), fm.remote.path);
                drop.accept();
            }
            Rectangle { anchors.fill: parent; radius: Theme.radius; visible: parent.containsDrag; color: Theme.accentA(0.12); border.color: Theme.accent; border.width: 2 }
        }
    }

    // ---------------- transfers
    Rectangle {
        id: transfers
        anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
        height: Math.min(200, 44 + jobList.contentHeight + 8)
        color: Theme.surface
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; height: 1; color: Theme.border }
        RowLayout {
            id: transfersHead
            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
            anchors.leftMargin: 18; anchors.rightMargin: 14; anchors.topMargin: 10
            spacing: 12
            Text { text: I18n.t("Transfers"); color: Theme.textStrong; font.pixelSize: 14; font.weight: Font.DemiBold }
            Text {
                text: fm.st.active > 0 ? I18n.t("%1 in progress").arg(fm.st.active) : ((fm.st.jobs || []).length ? I18n.t("All done") : I18n.t("Nothing yet"))
                color: Theme.textMuted; font.pixelSize: 13
            }
            Item { Layout.fillWidth: true }
            SecondaryButton {
                visible: (fm.st.jobs || []).some(function (j) { return j.state !== "running" && j.state !== "queued"; })
                text: I18n.t("Clear finished")
                onClicked: bridge.filesClearFinished()
            }
        }
        ListView {
            id: jobList
            anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
            anchors.top: transfersHead.bottom
            anchors.leftMargin: 18; anchors.rightMargin: 14; anchors.topMargin: 6; anchors.bottomMargin: 6
            clip: true
            model: fm.st.jobs || []
            ScrollBar.vertical: ScrollBar {}
            delegate: RowLayout {
                required property var modelData
                width: jobList.width
                height: 26
                spacing: 10
                Text { text: modelData.up ? "↑" : "↓"; color: Theme.textMuted; font.pixelSize: 13 }
                Text { text: modelData.name; color: Theme.textStrong; font.pixelSize: 12; elide: Text.ElideMiddle; Layout.preferredWidth: jobList.width * 0.32 }
                Rectangle {
                    Layout.fillWidth: true
                    height: 6; radius: 3
                    color: Theme.dark ? Qt.rgba(1, 1, 1, 0.08) : Qt.rgba(0, 0, 0, 0.08)
                    Rectangle {
                        height: parent.height; radius: 3
                        width: parent.width * (modelData.size > 0 ? Math.min(1, modelData.done / modelData.size) : (modelData.state === "done" ? 1 : 0))
                        color: modelData.state === "failed" ? Theme.danger : ((modelData.state === "done" || modelData.state === "skipped") ? Theme.success : Theme.accent)
                    }
                }
                Text {
                    Layout.preferredWidth: jobList.width * 0.26
                    elide: Text.ElideRight
                    font.pixelSize: 12
                    color: modelData.state === "failed" ? Theme.danger : Theme.textMuted
                    text: {
                        var s = modelData.state;
                        if (s === "running") return fm.human(modelData.done) + " / " + fm.human(modelData.size) + (modelData.speed > 0 ? "  " + fm.human(modelData.speed) + "/s" : "");
                        if (s === "queued") return I18n.t("Waiting");
                        if (s === "done") return I18n.t("Done");
                        if (s === "skipped") return I18n.t(modelData.message || "Skipped");
                        if (s === "cancelled") return I18n.t("Cancelled");
                        return I18n.t(modelData.message || "Failed");
                    }
                }
                IconButton {
                    visible: modelData.state === "running" || modelData.state === "queued"
                    iconName: "close"; iconSize: 14; implicitWidth: 24; implicitHeight: 24
                    tip: I18n.t("Cancel")
                    onClicked: bridge.filesCancel(modelData.id)
                }
            }
        }
    }
}
