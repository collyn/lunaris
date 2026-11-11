import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The app's main screen: "This computer" (let others control it) on the
// left, "Control a remote computer" with recent, favorite and server
// computers on the right, and the settings.
Rectangle {
    id: home
    color: Theme.bg

    signal startSessionRequested(string server, string token, string hostId, string hostName, int appId, string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol, string encoder, string displayId, bool virtualDisplay)

    // The server sign-in and its computers (Server tab); main.qml reaches it
    // as `dashboardView`.
    property alias dashboard: serverDashboard

    // "home" | "settings"
    property string page: "home"
    // "recent" | "favorites" | "server"
    property string peerTab: "recent"
    property string searchText: ""

    // This computer (see host.rs): enabled, running, port, password,
    // addresses, hostname, error.
    property var hostStatus: ({ enabled: false, running: false, port: 8091, password: "", addresses: [], hostname: "", error: "" })
    property bool passwordVisible: false
    property string hostError: ""

    // Saved computers: [{ address, name, password, favorite, lastUsed, os }]
    property var peers: []

    // Defaults for new connections (Settings > Display), saved in app_settings.
    property var prefs: ({
        theme: "system",
        res: "1920x1080",
        fps: 60,
        codec: "h264",
        bitrate: 15000,
        renderBackend: "auto_gpu"
    })

    property string statusText: ""
    property bool statusIsError: false

    // ---------------------------------------------------------------
    // State and persistence
    // ---------------------------------------------------------------
    function loadAll() {
        var stored = bridge.loadStore("app_settings");
        if (stored !== "") {
            try {
                var p = JSON.parse(stored);
                var merged = Object.assign({}, home.prefs);
                for (var k in p) merged[k] = p[k];
                home.prefs = merged;
            } catch (e) { console.log("app_settings: " + e); }
        }
        Theme.mode = home.prefs.theme || "system";
        window.closeToTray = home.prefs.closeToTray !== false;
        var storedPeers = bridge.loadStore("peers");
        if (storedPeers !== "") {
            try { home.peers = JSON.parse(storedPeers).peers || []; } catch (e) { console.log("peers: " + e); }
        }
        // Earlier versions remembered one direct connection.
        var lastAddress = bridge.savedDirectAddress();
        if (lastAddress !== "" && findPeer(lastAddress) === null) {
            rememberPeer(lastAddress, bridge.savedDirectPassword(), true);
        }
        refreshHost();
    }

    function setPref(key, value) {
        var next = Object.assign({}, home.prefs);
        next[key] = value;
        home.prefs = next;
        if (key === "theme") Theme.mode = value;
        bridge.saveStore("app_settings", JSON.stringify(home.prefs));
    }

    function savePeers() {
        bridge.saveStore("peers", JSON.stringify({ peers: home.peers }));
    }

    function refreshHost() {
        try { home.hostStatus = JSON.parse(bridge.hostStatus()); } catch (e) { console.log("host status: " + e); }
    }

    function setRemoteControl(on) {
        var err = on ? bridge.hostStart() : bridge.hostStop();
        home.hostError = err;
        // The agent needs a moment to open its port.
        refreshHost();
        hostRefreshSoon.restart();
    }

    function setHostPassword(password) {
        var err = bridge.hostSetPassword(password);
        home.hostError = err;
        refreshHost();
        return err;
    }

    function setHostPort(port) {
        var err = bridge.hostSetPort(port);
        home.hostError = err;
        refreshHost();
        hostRefreshSoon.restart();
        return err;
    }

    // "host" or "host:port" (default port 8091), without scheme or path.
    function normalizeAddress(text) {
        var addr = String(text || "").trim()
            .replace(/^wss?:\/\//, "").replace(/^https?:\/\//, "").replace(/\/.*$/, "");
        if (addr.length === 0) return "";
        var hasPort = addr.charAt(0) === "[" ? addr.indexOf("]:") !== -1 : addr.indexOf(":") !== -1;
        return hasPort ? addr : addr + ":8091";
    }

    function displayAddress(addr) {
        return addr.endsWith(":8091") ? addr.slice(0, -5) : addr;
    }

    function findPeer(address) {
        for (var i = 0; i < home.peers.length; i++) {
            if (home.peers[i].address === address) return home.peers[i];
        }
        return null;
    }

    function updatePeer(address, changes) {
        var list = home.peers.slice();
        for (var i = 0; i < list.length; i++) {
            if (list[i].address === address) {
                list[i] = Object.assign({}, list[i], changes);
                home.peers = list;
                savePeers();
                return;
            }
        }
    }

    function rememberPeer(address, password, remember) {
        var existing = findPeer(address);
        var changes = { lastUsed: Date.now() };
        if (remember) changes.password = password;
        if (existing) {
            updatePeer(address, changes);
        } else {
            var list = home.peers.slice();
            list.push(Object.assign({ address: address, name: "", password: "", favorite: false, os: "" }, changes));
            home.peers = list;
            savePeers();
        }
    }

    function removePeer(address) {
        home.peers = home.peers.filter(function (p) { return p.address !== address; });
        savePeers();
    }

    function visiblePeers() {
        var q = home.searchText.trim().toLowerCase();
        var list = home.peers.filter(function (p) {
            if (home.peerTab === "favorites" && !p.favorite) return false;
            if (q === "") return true;
            return (p.address + " " + (p.name || "")).toLowerCase().indexOf(q) !== -1;
        });
        list.sort(function (a, b) { return (b.lastUsed || 0) - (a.lastUsed || 0); });
        return list;
    }

    // Connect to a computer by address; asks for the password unless saved.
    function requestConnect(text) {
        var addr = normalizeAddress(text);
        if (addr === "") {
            setStatus("Enter the address of the computer to control", true);
            addressInput.forceActiveFocus();
            return;
        }
        var peer = findPeer(addr);
        if (peer && peer.password) {
            connectTo(addr, peer.password, true);
        } else {
            passwordDialog.openFor(addr);
        }
    }

    function connectTo(addr, password, remember) {
        rememberPeer(addr, password, remember);
        var peer = findPeer(addr);
        var name = (peer && peer.name) ? peer.name : displayAddress(addr);
        setStatus("Connecting to " + name + "…", false);
        bridge.saveDirectTarget(addr, remember ? password : "");
        home.startSessionRequested(
            "ws://" + addr, password, "direct", name, 0,
            home.prefs.res, home.prefs.fps, home.prefs.codec, home.prefs.bitrate, 256,
            home.prefs.renderBackend === "software", home.prefs.renderBackend,
            "webrtc", "auto", "default", false);
    }

    function setStatus(text, isError) {
        home.statusText = text;
        home.statusIsError = isError;
    }

    // A stream ended with an error (main.qml).
    function showError(message) {
        setStatus(message, true);
        home.page = "home";
    }

    Component.onCompleted: loadAll()

    Timer {
        // Pick up changes made elsewhere (the agent starting, a new address).
        interval: 4000
        running: home.visible
        repeat: true
        onTriggered: home.refreshHost()
    }

    Timer {
        id: hostRefreshSoon
        interval: 1200
        onTriggered: home.refreshHost()
    }

    // ---------------------------------------------------------------
    // Small building blocks
    // ---------------------------------------------------------------
    component TopTab: AbstractButton {
        id: tt
        property string iconName: ""
        property bool current: false
        hoverEnabled: true
        implicitHeight: 34
        implicitWidth: ttRow.implicitWidth + 24
        background: Rectangle {
            radius: Theme.radiusSm
            color: tt.current ? Theme.surface2 : (tt.hovered ? Theme.ink(0.04) : "transparent")
            border.color: tt.current ? Theme.border : "transparent"
        }
        contentItem: Item {
            Row {
                id: ttRow
                anchors.centerIn: parent
                spacing: 8
                Icon { name: tt.iconName; size: 15; color: tt.current ? Theme.accentFg : Theme.textMuted; anchors.verticalCenter: parent.verticalCenter }
                Text { text: tt.text; color: tt.current ? Theme.textStrong : Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; anchors.verticalCenter: parent.verticalCenter }
            }
        }
    }

    component PeerTab: AbstractButton {
        id: ptab
        property string iconName: ""
        property bool current: false
        hoverEnabled: true
        implicitHeight: 36
        implicitWidth: ptRow.implicitWidth + 16
        ToolTip.visible: hovered
        ToolTip.text: text
        ToolTip.delay: 600
        background: Item {
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: 2
                radius: 1
                color: ptab.current ? Theme.accentFg : "transparent"
            }
        }
        contentItem: Item {
            Row {
                id: ptRow
                anchors.centerIn: parent
                spacing: 7
                Icon { name: ptab.iconName; size: 16; color: ptab.current ? Theme.textStrong : (ptab.hovered ? Theme.text : Theme.textMuted); anchors.verticalCenter: parent.verticalCenter }
                Text { text: ptab.text; color: ptab.current ? Theme.textStrong : (ptab.hovered ? Theme.text : Theme.textMuted); font.pixelSize: 13; font.weight: ptab.current ? Font.DemiBold : Font.Normal; anchors.verticalCenter: parent.verticalCenter }
            }
        }
    }

    // ---------------------------------------------------------------
    // Layout: tabs on top, page below, status bar at the bottom
    // ---------------------------------------------------------------
    Rectangle {
        id: topBar
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: 48
        color: Theme.surface
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 1; color: Theme.border }

        Row {
            anchors.left: parent.left
            anchors.leftMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6

            Image {
                source: "icon.png"
                width: 22; height: 22
                sourceSize.width: 44; sourceSize.height: 44
                smooth: true
                anchors.verticalCenter: parent.verticalCenter
            }
            Item { width: 8; height: 1 }
            TopTab { text: "Home"; iconName: "home"; current: home.page === "home"; onClicked: home.page = "home" }
            TopTab { text: "Settings"; iconName: "settings"; current: home.page === "settings"; onClicked: home.page = "settings" }
        }
    }

    Rectangle {
        id: statusBar
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 32
        color: Theme.surface
        Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; height: 1; color: Theme.border }

        Row {
            anchors.left: parent.left
            anchors.leftMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            Rectangle {
                width: 8; height: 8; radius: 4
                anchors.verticalCenter: parent.verticalCenter
                color: home.statusIsError ? Theme.danger : (home.hostStatus.running ? Theme.success : Theme.textSubtle)
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                color: home.statusIsError ? Theme.danger : Theme.textMuted
                font.pixelSize: 12
                elide: Text.ElideRight
                width: home.width - 220
                text: home.statusText !== "" ? home.statusText
                    : (home.hostStatus.running ? "Ready. Others can control this computer with its address and password."
                                               : "Ready. Remote control of this computer is off.")
            }
        }
        Text {
            anchors.right: parent.right
            anchors.rightMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            text: "Lunaris " + bridge.appVersion()
            color: Theme.textSubtle
            font.pixelSize: 11
        }
    }

    Item {
        id: pageArea
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: topBar.bottom
        anchors.bottom: statusBar.top

        // ===========================================================
        // HOME
        // ===========================================================
        Item {
            anchors.fill: parent
            visible: home.page === "home"

            // ---------------- This computer ----------------
            Rectangle {
                id: sidePanel
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: 300
                color: Theme.surface
                Rectangle { anchors.right: parent.right; anchors.top: parent.top; anchors.bottom: parent.bottom; width: 1; color: Theme.border }

                Flickable {
                    anchors.fill: parent
                    anchors.rightMargin: 1
                    contentHeight: sideColumn.implicitHeight + 40
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds

                    ColumnLayout {
                        id: sideColumn
                        x: 22
                        y: 22
                        width: sidePanel.width - 44
                        spacing: 16

                        ColumnLayout {
                            spacing: 4
                            Layout.fillWidth: true
                            Text { text: "This computer"; color: Theme.textStrong; font.pixelSize: 19; font.weight: Font.DemiBold }
                            Text {
                                Layout.fillWidth: true
                                text: "Let another computer control this one with the address and password below."
                                color: Theme.textMuted
                                font.pixelSize: 12
                                wrapMode: Text.WordWrap
                                lineHeight: 1.2
                            }
                        }

                        // On/off
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: 52
                            radius: Theme.radius
                            color: Theme.surface2
                            border.color: Theme.border
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: 14
                                anchors.rightMargin: 8
                                spacing: 10
                                Icon { name: "power"; size: 17; color: home.hostStatus.running ? Theme.success : Theme.textMuted }
                                ColumnLayout {
                                    spacing: 1
                                    Layout.fillWidth: true
                                    Text { text: "Allow remote control"; color: Theme.textStrong; font.pixelSize: 13; font.weight: Font.Medium }
                                    Text {
                                        text: home.hostStatus.running ? "On" : (home.hostStatus.enabled ? "Starting…" : "Off")
                                        color: home.hostStatus.running ? Theme.success : Theme.textSubtle
                                        font.pixelSize: 11
                                    }
                                }
                                Toggle {
                                    id: remoteSwitch
                                    checked: home.hostStatus.enabled
                                    onToggled: {
                                        home.setRemoteControl(checked);
                                        checked = Qt.binding(function () { return home.hostStatus.enabled; });
                                    }
                                    Accessible.name: "Allow remote control"
                                }
                            }
                        }

                        // Address
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 6
                            opacity: home.hostStatus.enabled ? 1 : 0.55
                            Rectangle { Layout.fillWidth: true; height: 1; color: Theme.border }
                            Text { text: "Address"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium; Layout.topMargin: 4 }
                            Repeater {
                                model: home.hostStatus.addresses
                                delegate: RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 4
                                    readonly property string full: modelData + (home.hostStatus.port === 8091 ? "" : ":" + home.hostStatus.port)
                                    Text {
                                        Layout.fillWidth: true
                                        text: parent.full
                                        color: Theme.textStrong
                                        font.pixelSize: index === 0 ? 22 : 15
                                        font.weight: index === 0 ? Font.DemiBold : Font.Medium
                                        font.letterSpacing: 0.3
                                        elide: Text.ElideRight
                                    }
                                    IconButton {
                                        iconName: "copy"
                                        tip: "Copy address"
                                        onClicked: { clipboardHelper.copy(parent.full); home.setStatus("Address copied", false); }
                                    }
                                }
                            }
                            Text {
                                visible: home.hostStatus.addresses.length === 0
                                text: "No network connection"
                                color: Theme.textSubtle
                                font.pixelSize: 13
                            }
                            Text {
                                visible: home.hostStatus.hostname !== ""
                                text: home.hostStatus.hostname + (home.hostStatus.port === 8091 ? "" : " · port " + home.hostStatus.port)
                                color: Theme.textSubtle
                                font.pixelSize: 11
                            }
                        }

                        // Password
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 6
                            opacity: home.hostStatus.enabled ? 1 : 0.55
                            Rectangle { Layout.fillWidth: true; height: 1; color: Theme.border }
                            Text { text: "Password"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium; Layout.topMargin: 4 }
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 2
                                Text {
                                    Layout.fillWidth: true
                                    text: home.passwordVisible ? home.hostStatus.password : home.hostStatus.password.replace(/./g, "•")
                                    color: Theme.textStrong
                                    font.pixelSize: 17
                                    font.weight: Font.Medium
                                    font.family: home.passwordVisible ? "monospace" : ""
                                    elide: Text.ElideRight
                                }
                                IconButton {
                                    iconName: home.passwordVisible ? "eyeOff" : "eye"
                                    tip: home.passwordVisible ? "Hide password" : "Show password"
                                    onClicked: home.passwordVisible = !home.passwordVisible
                                }
                                IconButton {
                                    iconName: "copy"
                                    tip: "Copy password"
                                    onClicked: { clipboardHelper.copy(home.hostStatus.password); home.setStatus("Password copied", false); }
                                }
                            }
                            RowLayout {
                                spacing: 6
                                SecondaryButton {
                                    text: "New password"
                                    onClicked: {
                                        if (home.setHostPassword("") === "") {
                                            home.passwordVisible = true;
                                            home.setStatus("New password set. Computers using the old one can no longer connect.", false);
                                        }
                                    }
                                }
                                SecondaryButton {
                                    text: "Set my own"
                                    onClicked: customPasswordDialog.open()
                                }
                            }
                        }

                        Text {
                            Layout.fillWidth: true
                            visible: home.hostError !== "" || home.hostStatus.error !== ""
                            text: home.hostError !== "" ? home.hostError : home.hostStatus.error
                            color: Theme.danger
                            font.pixelSize: 12
                            wrapMode: Text.WordWrap
                        }

                        // Note on security (direct connections aren't TLS).
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: noteText.implicitHeight + 24
                            radius: Theme.radius
                            color: Theme.accentA(0.08)
                            border.color: Theme.accentA(0.25)
                            RowLayout {
                                anchors.fill: parent
                                anchors.margins: 12
                                spacing: 10
                                Icon { name: "shield"; size: 16; color: Theme.accentFg; Layout.alignment: Qt.AlignTop }
                                Text {
                                    id: noteText
                                    Layout.fillWidth: true
                                    text: "Direct connections are meant for your own network or a VPN. Video and input are encrypted; to reach this computer from the Internet, use a Lunaris server."
                                    color: Theme.textMuted
                                    font.pixelSize: 11
                                    wrapMode: Text.WordWrap
                                    lineHeight: 1.2
                                }
                            }
                        }
                    }
                }
            }

            // ---------------- Control a remote computer ----------------
            Item {
                id: mainArea
                anchors.left: sidePanel.right
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom

                Rectangle {
                    id: connectCard
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.margins: 24
                    width: Math.min(560, parent.width - 48)
                    // The Server tab needs the room (and has its own list).
                    visible: home.peerTab !== "server"
                    height: visible ? connectColumn.implicitHeight + 40 : 0
                    radius: Theme.radiusLg
                    color: Theme.surface
                    border.color: Theme.border

                    ColumnLayout {
                        id: connectColumn
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 20
                        spacing: 14

                        Text { text: "Control a remote computer"; color: Theme.textStrong; font.pixelSize: 17; font.weight: Font.DemiBold }

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 10
                            Rectangle {
                                Layout.fillWidth: true
                                implicitHeight: 42
                                radius: Theme.radiusSm
                                color: Theme.bg
                                border.color: addressInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: addressInput
                                    anchors.fill: parent
                                    anchors.leftMargin: 14
                                    anchors.rightMargin: 14
                                    verticalAlignment: TextInput.AlignVCenter
                                    color: Theme.textStrong
                                    selectionColor: Theme.accentA(0.4)
                                    font.pixelSize: 17
                                    clip: true
                                    onAccepted: home.requestConnect(text)
                                    Accessible.name: "Computer address"
                                    Text {
                                        anchors.verticalCenter: parent.verticalCenter
                                        visible: addressInput.text.length === 0
                                        text: "Address, e.g. 192.168.1.20"
                                        color: Theme.textSubtle
                                        font.pixelSize: 15
                                    }
                                }
                            }
                            PrimaryButton {
                                text: "Connect"
                                implicitHeight: 42
                                onClicked: home.requestConnect(addressInput.text)
                            }
                        }
                        Text {
                            Layout.fillWidth: true
                            text: "Enter the address shown under \"This computer\" on the other computer. Computers on a Lunaris server are under Server below."
                            color: Theme.textSubtle
                            font.pixelSize: 11
                            wrapMode: Text.WordWrap
                        }
                    }
                }

                // Tabs: recent / favorites / server, search on the right
                Item {
                    id: peerBar
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: connectCard.visible ? connectCard.bottom : parent.top
                    anchors.topMargin: connectCard.visible ? 18 : 12
                    anchors.leftMargin: 24
                    anchors.rightMargin: 24
                    height: 38
                    Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 1; color: Theme.border }

                    Row {
                        anchors.left: parent.left
                        anchors.bottom: parent.bottom
                        spacing: 6
                        PeerTab { text: "Recent"; iconName: "clock"; current: home.peerTab === "recent"; onClicked: home.peerTab = "recent" }
                        PeerTab { text: "Favorites"; iconName: "star"; current: home.peerTab === "favorites"; onClicked: home.peerTab = "favorites" }
                        PeerTab { text: "Server"; iconName: "server"; current: home.peerTab === "server"; onClicked: home.peerTab = "server" }
                    }

                    Rectangle {
                        visible: home.peerTab !== "server"
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.verticalCenterOffset: -3
                        width: 200
                        height: 30
                        radius: Theme.radiusSm
                        color: Theme.surface
                        border.color: searchInput.activeFocus ? Theme.accent : Theme.border
                        Icon { id: searchIcon; name: "search"; size: 14; color: Theme.textSubtle; anchors.left: parent.left; anchors.leftMargin: 9; anchors.verticalCenter: parent.verticalCenter }
                        TextInput {
                            id: searchInput
                            anchors.left: searchIcon.right
                            anchors.leftMargin: 7
                            anchors.right: parent.right
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            color: Theme.text
                            font.pixelSize: 12
                            clip: true
                            onTextChanged: home.searchText = text
                            Accessible.name: "Search computers"
                            Text { visible: searchInput.text.length === 0; text: "Search"; color: Theme.textSubtle; font.pixelSize: 12; anchors.verticalCenter: parent.verticalCenter }
                        }
                    }
                }

                // Saved computers
                Item {
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: peerBar.bottom
                    anchors.bottom: parent.bottom
                    visible: home.peerTab !== "server"

                    GridView {
                        id: peerGrid
                        anchors.fill: parent
                        anchors.leftMargin: 24
                        anchors.topMargin: 16
                        clip: true
                        cellWidth: 232
                        cellHeight: 168
                        model: home.visiblePeers()
                        boundsBehavior: Flickable.StopAtBounds
                        delegate: Item {
                            width: peerGrid.cellWidth
                            height: peerGrid.cellHeight
                            PeerCard {
                                anchors.fill: parent
                                anchors.rightMargin: 16
                                anchors.bottomMargin: 16
                                peer: modelData
                            }
                        }
                    }

                    Column {
                        anchors.centerIn: parent
                        visible: peerGrid.count === 0
                        spacing: 10
                        Icon {
                            anchors.horizontalCenter: parent.horizontalCenter
                            name: home.peerTab === "favorites" ? "star" : "clock"
                            size: 30
                            color: Theme.textSubtle
                        }
                        Text {
                            anchors.horizontalCenter: parent.horizontalCenter
                            text: home.searchText !== "" ? "No computer matches \"" + home.searchText + "\""
                                : (home.peerTab === "favorites" ? "No favorites yet" : "No recent connections")
                            color: Theme.textMuted
                            font.pixelSize: 14
                            font.weight: Font.Medium
                        }
                        Text {
                            anchors.horizontalCenter: parent.horizontalCenter
                            visible: home.searchText === ""
                            text: home.peerTab === "favorites" ? "Add a computer to favorites from its ⋮ menu."
                                                              : "Computers you connect to will show up here."
                            color: Theme.textSubtle
                            font.pixelSize: 12
                        }
                    }
                }

                // Lunaris server: sign-in and its computers
                Dashboard {
                    id: serverDashboard
                    embedded: true
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: peerBar.bottom
                    anchors.bottom: parent.bottom
                    anchors.fill: undefined
                    visible: home.peerTab === "server"
                    onStartSessionRequested: (server, token, hostId, hostName, appId, res, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol, encoder, displayId, virtualDisplay) => {
                        home.setStatus("Connecting to " + hostName + "…", false);
                        home.startSessionRequested(server, token, hostId, hostName, appId, res, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol, encoder, displayId, virtualDisplay);
                    }
                }
            }
        }

        // ===========================================================
        // SETTINGS
        // ===========================================================
        AppSettings {
            anchors.fill: parent
            visible: home.page === "settings"
            homeView: home
        }
    }

    // ---------------------------------------------------------------
    // Saved computer card
    // ---------------------------------------------------------------
    component PeerCard: Rectangle {
        id: card
        property var peer: ({})
        readonly property string title: peer.name ? peer.name : home.displayAddress(peer.address || "")
        readonly property var cardColors: ["#3b4a6b", "#4a3b6b", "#2f5560", "#5a4632", "#3e5a3a", "#5c3a4a"]
        radius: Theme.radius
        color: Theme.surface
        border.color: cardMa.containsMouse ? Theme.borderStrong : Theme.border
        clip: true

        function colorFor(text) {
            var h = 0;
            for (var i = 0; i < text.length; i++) h = (h * 31 + text.charCodeAt(i)) % 997;
            return cardColors[h % cardColors.length];
        }

        MouseArea {
            id: cardMa
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onDoubleClicked: home.requestConnect(card.peer.address)
            onClicked: home.requestConnect(card.peer.address)
        }

        Rectangle {
            id: cardTop
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            height: parent.height - 44
            radius: Theme.radius
            color: card.colorFor(card.peer.address || "")
            // square off the bottom corners
            Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: Theme.radius; color: parent.color }

            Column {
                anchors.centerIn: parent
                spacing: 8
                Icon { anchors.horizontalCenter: parent.horizontalCenter; name: "monitor"; size: 34; color: "#ffffff"; strokeWidth: 1.6 }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: card.width - 24
                    horizontalAlignment: Text.AlignHCenter
                    text: card.peer.name ? card.peer.name : (card.peer.os || "Computer")
                    color: Qt.rgba(1, 1, 1, 0.85)
                    font.pixelSize: 12
                    elide: Text.ElideRight
                }
            }
            Icon {
                visible: !!card.peer.favorite
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: 10
                name: "star"
                size: 15
                color: "#fcd34d"
            }
        }

        RowLayout {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 44
            anchors.leftMargin: 12
            anchors.rightMargin: 6
            spacing: 6
            Text {
                Layout.fillWidth: true
                text: home.displayAddress(card.peer.address || "")
                color: Theme.textStrong
                font.pixelSize: 13
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }
            IconButton {
                id: moreBtn
                iconName: "more"
                tip: "More"
                onClicked: peerMenu.popup(moreBtn, 0, moreBtn.height)
            }
        }

        ThemedMenu {
            id: peerMenu
            MenuItem { text: "Connect"; onTriggered: home.requestConnect(card.peer.address) }
            MenuItem {
                text: card.peer.favorite ? "Remove from favorites" : "Add to favorites"
                onTriggered: home.updatePeer(card.peer.address, { favorite: !card.peer.favorite })
            }
            MenuItem { text: "Rename…"; onTriggered: renameDialog.openFor(card.peer) }
            MenuItem {
                text: "Forget password"
                enabled: !!card.peer.password
                onTriggered: home.updatePeer(card.peer.address, { password: "" })
            }
            MenuSeparator {}
            MenuItem { text: "Remove"; onTriggered: home.removePeer(card.peer.address) }
        }
    }

    // ---------------------------------------------------------------
    // Dialogs
    // ---------------------------------------------------------------
    // Clipboard via a hidden TextEdit (no extra module needed).
    QtObject {
        id: clipboardHelper
        function copy(text) {
            clipboardEdit.text = text;
            clipboardEdit.selectAll();
            clipboardEdit.copy();
            clipboardEdit.text = "";
        }
    }
    TextEdit { id: clipboardEdit; visible: false }

    DialogFrame {
        id: passwordDialog
        property string address: ""
        function openFor(addr) {
            address = addr;
            pwField.text = "";
            rememberBox.checked = true;
            open();
            pwField.input.forceActiveFocus();
        }
        function accept() {
            if (pwField.text.trim() === "") return;
            close();
            home.connectTo(address, pwField.text.trim(), rememberBox.checked);
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: "Password for " + home.displayAddress(passwordDialog.address); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text {
                Layout.fillWidth: true
                text: "Enter the password shown under \"This computer\" on that computer."
                color: Theme.textMuted
                font.pixelSize: 12
                wrapMode: Text.WordWrap
            }
            InputField { id: pwField; placeholder: "Password"; password: true; onAccepted: passwordDialog.accept() }
            CheckRow { id: rememberBox; text: "Remember password"; checked: true }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: "Cancel"; onClicked: passwordDialog.close() }
                PrimaryButton { text: "Connect"; onClicked: passwordDialog.accept() }
            }
        }
    }

    DialogFrame {
        id: renameDialog
        property string address: ""
        function openFor(peer) {
            address = peer.address;
            nameField.text = peer.name || "";
            open();
            nameField.input.forceActiveFocus();
        }
        function accept() {
            home.updatePeer(address, { name: nameField.text.trim() });
            close();
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: "Rename " + home.displayAddress(renameDialog.address); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            InputField { id: nameField; placeholder: "Name, e.g. Office PC"; onAccepted: renameDialog.accept() }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: "Cancel"; onClicked: renameDialog.close() }
                PrimaryButton { text: "Save"; onClicked: renameDialog.accept() }
            }
        }
    }

    DialogFrame {
        id: customPasswordDialog
        property string error: ""
        onOpened: { customField.text = ""; error = ""; customField.input.forceActiveFocus(); }
        function accept() {
            var err = home.setHostPassword(customField.text);
            if (err !== "") { error = err; return; }
            home.passwordVisible = true;
            home.setStatus("Password changed. Computers using the old one can no longer connect.", false);
            close();
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: "Set the password"; color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text {
                Layout.fillWidth: true
                text: "Anyone with this computer's address and this password can control it. Use at least 6 characters."
                color: Theme.textMuted
                font.pixelSize: 12
                wrapMode: Text.WordWrap
            }
            InputField { id: customField; placeholder: "New password"; onAccepted: customPasswordDialog.accept() }
            Text { visible: customPasswordDialog.error !== ""; text: customPasswordDialog.error; color: Theme.danger; font.pixelSize: 12 }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: "Cancel"; onClicked: customPasswordDialog.close() }
                PrimaryButton { text: "Save"; onClicked: customPasswordDialog.accept() }
            }
        }
    }
}
