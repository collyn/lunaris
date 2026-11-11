import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Shapes

Rectangle {
    id: dashboardRoot
    anchors.fill: parent
    color: Theme.bg

    // Exposed properties/signals
    property string token: ""
    property string username: ""
    property string serverUrl: ""

    signal startSessionRequested(string server, string token, string hostId, string hostName, int appId, string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol, string encoder, string displayId, bool virtualDisplay)

    // Internal UI state properties
    property bool isLoggedIn: false
    property string authError: ""
    property bool authLoading: false
    // Login screen shows "connect directly by IP" instead of the server sign-in.
    property bool directMode: false
    // Shown inside the Home "Server" tab: Home has its own address box and
    // brand, and the window isn't dragged from here.
    property bool embedded: false
    clip: embedded

    // Direct connection (no server): the agent's address ("host" or
    // "host:port", default port 8091) and its access password.
    function connectDirect(address, password) {
        var addr = address.trim().replace(/^wss?:\/\//, "").replace(/^https?:\/\//, "").replace(/\/.*$/, "");
        if (addr.length === 0 || password.trim().length === 0) {
            dashboardRoot.authError = "Enter the computer's address and its access password";
            return;
        }
        // Add the default port unless one is given ("[v6]:port" or "host:port").
        var hasPort = addr.charAt(0) === "[" ? addr.indexOf("]:") !== -1 : addr.indexOf(":") !== -1;
        if (!hasPort) addr = addr + ":8091";
        dashboardRoot.authError = "";
        bridge.saveDirectTarget(addr, password.trim());
        dashboardRoot.startSessionRequested(
            "ws://" + addr, password.trim(), "direct", addr, 0,
            dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec,
            dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda,
            dashboardRoot.configRenderBackend, dashboardRoot.configInputProtocol,
            dashboardRoot.configEncoder, dashboardRoot.configDisplay, dashboardRoot.configVirtualDisplay);
    }


    // Remote control of `host` in the browser: the web console signed in as
    // this app's user (session in the URL fragment, never sent to the
    // server) connecting straight to the computer.
    function openHostInBrowser(host) {
        Qt.openUrlExternally(String(dashboardRoot.serverUrl).replace(/\/+$/, "")
            + "/#session=" + encodeURIComponent(dashboardRoot.token)
            + "&user=" + encodeURIComponent(dashboardRoot.username)
            + "&connect=" + encodeURIComponent(host.id)
            + "&name=" + encodeURIComponent(host.name));
    }

    // Connect straight to a computer's desktop (app 0, the only one an
    // agent offers) with the current stream settings.
    function connectHost(host) {
        dashboardRoot.startSessionRequested(
            dashboardRoot.serverUrl, dashboardRoot.token, host.id, host.name, 0,
            dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec,
            dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda,
            dashboardRoot.configRenderBackend, dashboardRoot.configInputProtocol,
            dashboardRoot.configEncoder, dashboardRoot.configDisplay, dashboardRoot.configVirtualDisplay);
    }

    property var hostsList: []
    property bool hostsLoading: false
    property string hostsError: ""

    property var selectedHost: null
    property var appsList: []
    property bool appsLoading: false
    property string appsError: ""

    // Settings Modal state
    property var activeConfigHost: null
    property string configRes: "1920x1080"
    property int configFps: 60
    property string configCodec: "h264"
    property int configBitrate: 8000
    property int configQueueLimit: 256
    property bool configDisableCuda: false
    property string configRenderBackend: configDisableCuda ? "software" : "auto_gpu"
    property string configInputProtocol: "webrtc"
    property string configEncoder: "auto"
    property string configDisplay: "default"
    property bool configVirtualDisplay: false
    property var pendingDeleteHost: null



    // Agent configuration token
    property string agentToken: ""
    property bool agentTokenLoading: false
    property string agentTokenError: ""

    // Local Agent Control state
    property string localAgentHostname: ""
    property bool localAgentRunning: false



    // Connect to bridge signals
    Connections {
        target: bridge

        function onLoginResult(success, errorMsg, tok, user, srv) {
            dashboardRoot.authLoading = false;
            if (success) {
                dashboardRoot.token = tok;
                dashboardRoot.username = user;
                dashboardRoot.serverUrl = srv;
                dashboardRoot.isLoggedIn = true;
                dashboardRoot.authError = "";
                bridge.fetchHosts();
            } else {
                dashboardRoot.authError = errorMsg;
            }
        }



        function onCredentialsLoaded(success, srv, tok, user) {
            if (success) {
                dashboardRoot.token = tok;
                dashboardRoot.username = user;
                dashboardRoot.serverUrl = srv;
                dashboardRoot.isLoggedIn = true;
                bridge.fetchHosts();
            } else {
                dashboardRoot.isLoggedIn = false;
            }
        }

        function onHostsResult(success, errorMsg, hostsJson) {
            dashboardRoot.hostsLoading = false;
            if (success) {
                dashboardRoot.hostsList = JSON.parse(hostsJson);
                dashboardRoot.hostsError = "";
            } else {
                dashboardRoot.hostsError = errorMsg;
                if (errorMsg === "Unauthorized") {
                    dashboardRoot.isLoggedIn = false;
                    bridge.logout();
                }
            }
        }



        function onAppsResult(success, errorMsg, hostId, appsJson) {
            dashboardRoot.appsLoading = false;
            if (success) {
                dashboardRoot.appsList = JSON.parse(appsJson);
                dashboardRoot.appsError = "";
            } else {
                dashboardRoot.appsError = errorMsg;
            }
        }

        function onAgentTokenResult(success, errorMsg, tok) {
            dashboardRoot.agentTokenLoading = false;
            if (success) {
                dashboardRoot.agentToken = tok;
                dashboardRoot.agentTokenError = "";
            } else {
                dashboardRoot.agentToken = "";
                dashboardRoot.agentTokenError = errorMsg;
            }
        }
    }

    Component.onCompleted: {
        bridge.loadSavedCredentials();
        dashboardRoot.localAgentHostname = bridge.getLocalHostname();
    }



    // Main layout switcher: Login Page or Dashboard Page
    Loader {
        id: viewLoader
        anchors.fill: parent
        z: 1
        sourceComponent: dashboardRoot.isLoggedIn ? mainDashboardComponent : loginComponent
    }

    // ----------------------------------------------------
    // COMPONENT: Login / Register Form
    // ----------------------------------------------------
    Component {
        id: loginComponent
        Item {
            anchors.fill: parent

            Component.onCompleted: {
                // Inside Home the address box keeps the focus.
                if (!dashboardRoot.embedded) serverInput.forceActiveFocus();
            }

            // Central glass card (near the top inside Home, where space is short)
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                y: dashboardRoot.embedded ? 16 : Math.max(0, (parent.height - height) / 2)
                width: 420
                height: (dashboardRoot.directMode ? 420 : 480) + (dashboardRoot.authError !== "" ? 24 : 0)
                radius: 20
                color: Theme.surfaceA(0.8)
                border.color: Theme.ink(0.08)
                border.width: 1

                Behavior on height {
                    NumberAnimation { duration: 250; easing.type: Easing.OutCubic }
                }

                // Inner content
                Column {
                    anchors.fill: parent
                    anchors.margins: 40
                    spacing: 20

                    // Logo Title
                    Text {
                        text: dashboardRoot.directMode ? "Connect directly" : (dashboardRoot.embedded ? "Sign in to a Lunaris server" : "Welcome back")
                        font.pixelSize: 22
                        font.weight: Font.DemiBold
                        color: Theme.textStrong
                        anchors.horizontalCenter: parent.horizontalCenter
                    }

                    Text {
                        text: dashboardRoot.directMode ? "To a computer on your network or VPN, no server needed" : (dashboardRoot.embedded ? "See and connect to the computers registered on your server" : "Access your remote desktop network")
                        font.pixelSize: 13
                        color: Theme.textMuted
                        anchors.horizontalCenter: parent.horizontalCenter
                    }

                    Item { width: 1; height: 10 } // Spacer

                    // Direct connection inputs
                    Column {
                        visible: dashboardRoot.directMode
                        width: parent.width
                        spacing: 12

                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: "Computer address"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: directAddressInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: directAddressInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: bridge.savedDirectAddress()
                                    verticalAlignment: Text.AlignVCenter
                                    selectByMouse: true
                                    activeFocusOnTab: true
                                    KeyNavigation.tab: directPasswordInput
                                    Keys.onReturnPressed: directButton.clicked()
                                    Keys.onEnterPressed: directButton.clicked()
                                    Text {
                                        anchors.fill: parent; verticalAlignment: Text.AlignVCenter
                                        visible: parent.text.length === 0
                                        text: "192.168.1.20 or 192.168.1.20:8091"
                                        color: Theme.textSubtle; font.pixelSize: 13
                                    }
                                }
                            }
                        }

                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: "Access password (shown in the agent window)"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: directPasswordInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: directPasswordInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: bridge.savedDirectPassword()
                                    echoMode: TextInput.Password
                                    verticalAlignment: Text.AlignVCenter
                                    selectByMouse: true
                                    activeFocusOnTab: true
                                    KeyNavigation.tab: directAddressInput
                                    Keys.onReturnPressed: directButton.clicked()
                                    Keys.onEnterPressed: directButton.clicked()
                                }
                            }
                        }
                    }

                    // Inputs
                    Column {
                        visible: !dashboardRoot.directMode
                        width: parent.width
                        spacing: 12

                        // Server Address
                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: "Signaling Server Host"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: serverInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: serverInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: "http://127.0.0.1:8090"
                                    verticalAlignment: Text.AlignVCenter
                                    selectByMouse: true
                                    activeFocusOnTab: true
                                    KeyNavigation.tab: usernameInput
                                    Keys.onReturnPressed: submitButton.clicked()
                                    Keys.onEnterPressed: submitButton.clicked()
                                }
                            }
                        }

                        // Username
                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: "Username"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: usernameInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: usernameInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: ""
                                    verticalAlignment: Text.AlignVCenter
                                    selectByMouse: true
                                    activeFocusOnTab: true
                                    KeyNavigation.tab: passwordInput
                                    Keys.onReturnPressed: submitButton.clicked()
                                    Keys.onEnterPressed: submitButton.clicked()
                                }
                            }
                        }

                        // Password
                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: "Password"; color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: passwordInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: passwordInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: ""
                                    echoMode: TextInput.Password
                                    verticalAlignment: Text.AlignVCenter
                                    selectByMouse: true
                                    activeFocusOnTab: true
                                    KeyNavigation.tab: serverInput
                                    Keys.onReturnPressed: submitButton.clicked()
                                    Keys.onEnterPressed: submitButton.clicked()
                                }
                            }
                        }


                    }

                    // Error text
                    Text {
                        text: dashboardRoot.authError
                        color: "#ef4444"
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        visible: text.length > 0
                    }

                    // Direct connect button
                    Button {
                        id: directButton
                        visible: dashboardRoot.directMode
                        width: parent.width
                        onClicked: dashboardRoot.connectDirect(directAddressInput.text, directPasswordInput.text)
                        background: Rectangle {
                            implicitHeight: 40
                            radius: 8
                            color: directButton.hovered ? Theme.accentHover : Theme.accent
                        }
                        contentItem: Item {
                            anchors.fill: parent
                            Text {
                                text: "Connect"
                                color: Theme.textOnAccent
                                font.weight: Font.DemiBold
                                font.pixelSize: 13
                                anchors.centerIn: parent
                            }
                        }
                    }

                    // Submit Button
                    Button {
                        id: submitButton
                        visible: !dashboardRoot.directMode
                        width: parent.width
                        enabled: !dashboardRoot.authLoading
                        onClicked: {
                            dashboardRoot.authError = "";
                            if (serverInput.text.trim().length === 0 || usernameInput.text.trim().length === 0 || passwordInput.text.trim().length === 0) {
                                dashboardRoot.authError = "All fields are required";
                                return;
                            }
                            dashboardRoot.authLoading = true;
                            bridge.login(serverInput.text.trim(), usernameInput.text.trim(), passwordInput.text);
                        }

                        background: Rectangle {
                            implicitHeight: 40
                            radius: 8
                            color: submitButton.hovered ? Theme.accentHover : Theme.accent
                            opacity: dashboardRoot.authLoading ? 0.6 : 1.0
                        }

                        contentItem: Item {
                            anchors.fill: parent
                            Text {
                                text: dashboardRoot.authLoading ? "Signing in..." : "Sign in"
                                color: Theme.textOnAccent
                                font.weight: Font.DemiBold
                                font.pixelSize: 13
                                anchors.centerIn: parent
                            }
                        }
                    }

                    // Switch between server sign-in and direct connection
                    Text {
                        visible: !dashboardRoot.embedded
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: dashboardRoot.directMode ? "Sign in to a Lunaris server instead" : "Connect directly by IP address"
                        color: modeSwitchMa.containsMouse ? Theme.accentFg : Theme.textMuted
                        font.pixelSize: 12
                        font.underline: modeSwitchMa.containsMouse
                        MouseArea {
                            id: modeSwitchMa
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                dashboardRoot.authError = "";
                                dashboardRoot.directMode = !dashboardRoot.directMode;
                                if (dashboardRoot.directMode) directAddressInput.forceActiveFocus(); else serverInput.forceActiveFocus();
                            }
                        }
                    }


                }
            }
        }
    }

    // ----------------------------------------------------
    // COMPONENT: Main Dashboard Area
    // ----------------------------------------------------
    Component {
        id: mainDashboardComponent
        Item {
            anchors.fill: parent

            // Horizontal Top Navbar
            Rectangle {
                id: navbar
                height: 64
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                color: Theme.bg
                border.color: Theme.ink(0.03)
                border.width: 1

                // Allow dragging the window by holding and dragging the navbar
                DragHandler {
                    target: null
                    enabled: !dashboardRoot.embedded
                    onActiveChanged: {
                        if (active) {
                            window.startSystemMove();
                        }
                    }
                }

                // Brand Title & Tech Pill
                Row {
                    visible: !dashboardRoot.embedded
                    anchors.left: parent.left
                    anchors.leftMargin: 24
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 12

                    Image {
                        source: "icon.png"
                        width: 26; height: 26
                        sourceSize.width: 52; sourceSize.height: 52
                        smooth: true
                        anchors.verticalCenter: parent.verticalCenter
                    }

                    Text {
                        text: "Lunaris"
                        font.pixelSize: 15
                        font.weight: Font.DemiBold
                        color: Theme.textStrong
                        anchors.verticalCenter: parent.verticalCenter
                    }

                    Rectangle {
                        width: 52; height: 18; radius: 9
                        color: Theme.surface2
                        border.color: Theme.border
                        anchors.verticalCenter: parent.verticalCenter
                        Text {
                            text: Qt.application.version !== "" ? "v" + Qt.application.version : ""
                            color: Theme.textMuted
                            font.pixelSize: 10
                            font.bold: true
                            anchors.centerIn: parent
                        }
                    }
                }

                // Right Panel
                Row {
                    anchors.right: parent.right
                    anchors.rightMargin: 24
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 20

                    // Theme toggle: system -> light -> dark
                    Button {
                        id: themeBtn
                        visible: !dashboardRoot.embedded
                        anchors.verticalCenter: parent.verticalCenter
                        hoverEnabled: true
                        onClicked: Theme.mode = Theme.mode === "system" ? "light" : (Theme.mode === "light" ? "dark" : "system")
                        ToolTip.visible: hovered
                        ToolTip.text: "Theme: " + Theme.mode
                        background: Rectangle {
                            implicitWidth: 32; implicitHeight: 32; radius: Theme.radiusSm
                            color: themeBtn.hovered ? Theme.surface2 : "transparent"
                            border.color: Theme.border; border.width: 1
                        }
                        contentItem: Text {
                            text: Theme.mode === "light" ? "\u2600" : (Theme.mode === "dark" ? "\u263E" : "A")
                            color: Theme.textMuted
                            font.pixelSize: 14
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                    }

                    // Signed-in user and server
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 1
                        Text {
                            text: dashboardRoot.username
                            font.pixelSize: 13
                            font.weight: Font.DemiBold
                            color: Theme.textStrong
                            anchors.right: parent.right
                        }
                        Text {
                            text: dashboardRoot.serverUrl
                            font.pixelSize: 11
                            color: Theme.textSubtle
                            anchors.right: parent.right
                        }
                    }

                    // Logout Button (Sleek vector exit door outline)
                    Button {
                        id: logoutBtn
                        anchors.verticalCenter: parent.verticalCenter
                        onClicked: {
                            bridge.logout();
                            dashboardRoot.isLoggedIn = false;
                            dashboardRoot.token = "";
                            dashboardRoot.username = "";

                        }
                        background: Rectangle {
                            implicitWidth: 32; implicitHeight: 32; radius: 6
                            color: logoutBtn.hovered ? Qt.rgba(239/255, 68/255, 68/255, 0.1) : "transparent"
                            border.color: logoutBtn.hovered ? "#ef4444" : Theme.ink(0.08)
                            border.width: 1
                        }
                        contentItem: Item {
                            implicitWidth: 16; implicitHeight: 16
                            // Sleek Vector exit arrow icon
                            Item {
                                width: 16; height: 16
                                anchors.centerIn: parent
                                Rectangle {
                                    x: 1; y: 1; width: 10; height: 14; radius: 1.5
                                    color: "transparent"; border.color: logoutBtn.hovered ? "#ef4444" : Theme.text; border.width: 1.5
                                }
                                Rectangle {
                                    x: 10; y: 3.5; width: 2; height: 9; color: Theme.bg
                                }
                                Rectangle {
                                    x: 6; y: 7.25; width: 8; height: 1.5; color: logoutBtn.hovered ? "#ef4444" : Theme.text
                                }
                                Rectangle {
                                    x: 10; y: 4.5; width: 1.5; height: 4.5; rotation: 45; color: logoutBtn.hovered ? "#ef4444" : Theme.text; antialiasing: true
                                }
                                Rectangle {
                                    x: 10; y: 7; width: 1.5; height: 4.5; rotation: -45; color: logoutBtn.hovered ? "#ef4444" : Theme.text; antialiasing: true
                                }
                            }
                        }
                    }
                }
            }

            // Main Content Area
            Item {
                id: mainViewport
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.topMargin: 64 // Below navbar
                anchors.bottom: parent.bottom
                anchors.margins: dashboardRoot.embedded ? 20 : 40

                Loader {
                    anchors.fill: parent
                    sourceComponent: dashboardRoot.selectedHost !== null 
                        ? appDirectoryComponent 
                        : hostsGridComponent
                }
            }
        }
    }

    // ----------------------------------------------------
    // COMPONENT: Grid view of paired host devices
    // ----------------------------------------------------
    Component {
        id: hostsGridComponent
        Item {
            anchors.fill: parent

            // Header Row
            Item {
                id: sectionHeader
                width: parent.width
                height: 50
                anchors.top: parent.top

                Column {
                    anchors.left: parent.left
                    spacing: 4
                    Text {
                        text: "Device Directory"
                        font.pixelSize: 24
                        font.bold: true
                        color: Theme.textStrong
                    }
                    Text {
                        text: "Manage and connect to active remote agent streams"
                        font.pixelSize: 13
                        color: Theme.textSubtle
                    }
                }

                Row {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 12

                    // Sync Devices Button
                    Button {
                        id: syncBtn
                        onClicked: {
                            dashboardRoot.hostsLoading = true;
                            bridge.fetchHosts();
                        }
                        background: Rectangle {
                            implicitWidth: 120; implicitHeight: 38; radius: 8
                            color: syncBtn.hovered ? Theme.ink(0.05) : "transparent"
                            border.color: syncBtn.hovered ? Theme.accentFg : Theme.ink(0.15)
                            border.width: 1
                        }
                        contentItem: Text {
                            text: "Sync Devices"
                            color: Theme.text
                            font.bold: true
                            font.pixelSize: 13
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                    }


                }
            }

            // Hosts Grid
            ScrollView {
                anchors.top: sectionHeader.bottom
                anchors.bottom: parent.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.topMargin: 32
                clip: true

                GridView {
                    id: hostsGrid
                    anchors.fill: parent
                    topMargin: 8
                    cellWidth: 260
                    cellHeight: 180
                    model: dashboardRoot.hostsList.length > 0 ? dashboardRoot.hostsList : 0

                    delegate: Item {
                        id: hostDelegate
                        width: 260
                        height: 180

                        property bool isHostHovered: cardMouseArea.containsMouse || webBtn.hovered || gearBtn.hovered || deleteBtn.hovered || connectBtn.hovered

                        // Static background click handler to avoid hover-translation flicker
                        MouseArea {
                            id: cardMouseArea
                            width: 240; height: 160
                            hoverEnabled: true
                            onClicked: {
                                if (modelData.status === "Online") dashboardRoot.connectHost(modelData);
                            }
                        }

                        // Device card (translates up 4px smoothly on hover based on parent static MouseArea)
                        Rectangle {
                            id: card
                            width: 240
                            height: 160
                            radius: 14
                            color: Theme.surface
                            border.color: isHostHovered ? Theme.borderStrong : Theme.border
                            border.width: 1

                            Behavior on border.color { ColorAnimation { duration: 150 } }

                            // Device Card content
                            Item {
                                anchors.fill: parent
                                anchors.margins: 18

                                // Host info block
                                Column {
                                    id: hostInfo
                                    anchors.left: parent.left
                                    anchors.top: parent.top
                                    anchors.right: parent.right
                                    spacing: 4

                                    Text {
                                        text: modelData.name
                                        font.pixelSize: 18
                                        font.bold: true
                                        color: Theme.textStrong
                                        elide: Text.ElideRight
                                        width: parent.width
                                    }
                                    Text {
                                        text: "ID: " + modelData.id.substring(0, 8) + "..."
                                        font.pixelSize: 11
                                        font.family: "monospace"
                                        color: Theme.textSubtle
                                    }
                                }

                                // Divider top
                                Rectangle {
                                    width: parent.width; height: 1
                                    anchors.top: hostInfo.bottom
                                    anchors.topMargin: 8
                                    color: Theme.ink(0.03)
                                }

                                // Detail rows inside standard border-bracket body
                                Column {
                                    anchors.left: parent.left
                                    anchors.right: parent.right
                                    anchors.bottom: cardFooter.top
                                    anchors.bottomMargin: 10
                                    spacing: 6

                                    Row {
                                        width: parent.width
                                        Text { text: "IP Address"; font.pixelSize: 11; color: Theme.textSubtle; width: 80 }
                                        Text { text: modelData.ip_address || "Connected via Agent"; font.pixelSize: 11; font.bold: true; font.family: "monospace"; color: Theme.text }
                                    }

                                    Row {
                                        width: parent.width
                                        Text { text: "Status"; font.pixelSize: 11; color: Theme.textSubtle; width: 80 }
                                        Row {
                                            spacing: 6
                                            Rectangle {
                                                width: 8; height: 8; radius: 4
                                                anchors.verticalCenter: parent.verticalCenter
                                                color: modelData.status === "Online" ? "#22c55e" : (modelData.status === "Busy" ? "#f59e0b" : "#ef4444")
                                            }
                                            Text {
                                                text: modelData.status
                                                font.pixelSize: 11
                                                font.bold: true
                                                color: modelData.status === "Online" ? "#22c55e" : (modelData.status === "Busy" ? "#f59e0b" : "#ef4444")
                                            }
                                        }
                                    }
                                }

                                // Primary action (matches the web client's "Connect")
                                Button {
                                    id: connectBtn
                                    visible: modelData.status === "Online"
                                    anchors.left: parent.left
                                    anchors.bottom: parent.bottom
                                    z: 10
                                    onClicked: dashboardRoot.connectHost(modelData)
                                    background: Rectangle {
                                        implicitWidth: 84; implicitHeight: 32; radius: Theme.radiusSm
                                        color: connectBtn.hovered ? Theme.accentHover : Theme.accent
                                    }
                                    contentItem: Text {
                                        text: "Connect"
                                        color: Theme.textOnAccent
                                        font.pixelSize: 13
                                        font.weight: Font.DemiBold
                                        horizontalAlignment: Text.AlignHCenter
                                        verticalAlignment: Text.AlignVCenter
                                    }
                                }

                                // Card actions footer (bottom right, sits on top of visual card)
                                Row {
                                    id: cardFooter
                                    anchors.right: parent.right
                                    anchors.bottom: parent.bottom
                                    spacing: 8
                                    z: 10

                                    // Remote control in the browser (web console, signed in)
                                    Button {
                                        id: webBtn
                                        visible: modelData.status === "Online"
                                        hoverEnabled: true
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: "Open in the browser"
                                        Accessible.name: "Open in the browser"
                                        onClicked: dashboardRoot.openHostInBrowser(modelData)
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: webBtn.hovered ? Theme.accentA(0.1) : "transparent"
                                            border.color: webBtn.hovered ? Theme.accentFg : "transparent"
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Icon { anchors.centerIn: parent; name: "globe"; size: 17; color: webBtn.hovered ? Theme.accentFg : Theme.text }
                                        }
                                    }

                                    // Settings Gear Icon (Sleek custom vector gear)
                                    Button {
                                        id: gearBtn
                                        visible: modelData.status === "Online" || modelData.status === "Busy"
                                        onClicked: {
                                            // Only reset to defaults when switching to a different host
                                            if (dashboardRoot.activeConfigHost === null || dashboardRoot.activeConfigHost.id !== modelData.id) {
                                                dashboardRoot.configRes = "1920x1080";
                                                dashboardRoot.configFps = 60;
                                                dashboardRoot.configCodec = "h264";
                                                dashboardRoot.configBitrate = 8000;
                                                dashboardRoot.configQueueLimit = 256;
                                                dashboardRoot.configDisableCuda = false;
                                                dashboardRoot.configRenderBackend = "auto_gpu";
                                                dashboardRoot.configInputProtocol = "webrtc";
                                                dashboardRoot.configEncoder = "auto";
                                                dashboardRoot.configDisplay = "default";
                                                dashboardRoot.configVirtualDisplay = false;
                                            }
                                            dashboardRoot.activeConfigHost = modelData;
                                            settingsModal.setCurrentSettings(dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec, dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda, dashboardRoot.configInputProtocol, dashboardRoot.configEncoder, dashboardRoot.configDisplay, dashboardRoot.configVirtualDisplay, dashboardRoot.configRenderBackend);
                                            // Fetch encoder/display capabilities from agent
                                            if (dashboardRoot.serverUrl && dashboardRoot.token && modelData.id) {
                                                bridge.fetchCapabilities(dashboardRoot.serverUrl, dashboardRoot.token, modelData.id);
                                            }
                                            settingsModal.open();
                                        }
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: gearBtn.hovered ? Theme.accentA(0.1) : "transparent"
                                            border.color: gearBtn.hovered ? Theme.accentFg : "transparent"
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Item {
                                                width: 16; height: 16
                                                anchors.centerIn: parent
                                                Rectangle {
                                                    anchors.centerIn: parent; width: 10; height: 10; radius: 5
                                                    color: "transparent"; border.color: gearBtn.hovered ? Theme.accentFg : Theme.text; border.width: 1.8
                                                }
                                                Repeater {
                                                    model: 8
                                                    Rectangle {
                                                        anchors.centerIn: parent; width: 2.2; height: 13.5
                                                        color: gearBtn.hovered ? Theme.accent : Theme.text; rotation: index * 45; antialiasing: true
                                                    }
                                                }
                                                Rectangle {
                                                    anchors.centerIn: parent; width: 6.4; height: 6.4; radius: 3.2; color: Theme.surface
                                                }
                                            }
                                        }
                                    }

                                    Button {
                                        id: deleteBtn
                                        onClicked: {
                                            dashboardRoot.pendingDeleteHost = modelData;
                                            deleteHostDialog.open();
                                        }
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: deleteBtn.hovered ? Qt.rgba(239/255, 68/255, 68/255, 0.1) : "transparent"
                                            border.color: deleteBtn.hovered ? "#ef4444" : "transparent"
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Item {
                                                width: 16; height: 16
                                                anchors.centerIn: parent
                                                Rectangle {
                                                    x: 4; y: 5; width: 8; height: 9; radius: 1
                                                    color: "transparent"
                                                    border.color: deleteBtn.hovered ? "#ef4444" : Theme.text
                                                    border.width: 1.4
                                                }
                                                Rectangle {
                                                    x: 3; y: 3; width: 10; height: 1.5; radius: 0.75
                                                    color: deleteBtn.hovered ? "#ef4444" : Theme.text
                                                }
                                                Rectangle { x: 6; y: 1.5; width: 4; height: 1.4; radius: 0.7; color: deleteBtn.hovered ? "#ef4444" : Theme.text }
                                                Rectangle { x: 6.2; y: 7; width: 1.2; height: 5; radius: 0.6; color: deleteBtn.hovered ? "#ef4444" : Theme.text }
                                                Rectangle { x: 9; y: 7; width: 1.2; height: 5; radius: 0.6; color: deleteBtn.hovered ? "#ef4444" : Theme.text }
                                            }
                                        }
                                    }

                                }
                            }
                        }
                    }
                }
            }

            // Empty state view
            Column {
                anchors.centerIn: parent
                visible: dashboardRoot.hostsList.length === 0 && !dashboardRoot.hostsLoading
                spacing: 16
                Text { text: "No agents connected yet"; font.pixelSize: 16; font.bold: true; color: Theme.textMuted; anchors.horizontalCenter: parent.horizontalCenter }
                Text { text: "Agents will appear here automatically once registered"; font.pixelSize: 12; color: Theme.textSubtle; anchors.horizontalCenter: parent.horizontalCenter }
            }
        }
    }



    // ----------------------------------------------------
    // COMPONENT: Application Directory (Grid View)
    // ----------------------------------------------------
    Component {
        id: appDirectoryComponent
        Item {
            anchors.fill: parent

            // Back Header & Breadcrumbs
            Row {
                id: backHeader
                spacing: 16
                anchors.top: parent.top

                Button {
                    id: backBtn
                    onClicked: {
                        dashboardRoot.selectedHost = null;
                        dashboardRoot.appsList = [];
                        bridge.fetchHosts();
                    }
                    background: Rectangle {
                        implicitWidth: 36; implicitHeight: 36; radius: 18
                        color: backBtn.hovered ? Theme.accentA(0.1) : "transparent"
                        border.color: backBtn.hovered ? Theme.accentFg : "transparent"
                        border.width: 1
                        Behavior on color { ColorAnimation { duration: 150 } }
                        Behavior on border.color { ColorAnimation { duration: 150 } }
                    }
                    contentItem: Item {
                        implicitWidth: 16; implicitHeight: 16
                        Shape {
                            id: backArrow
                            anchors.centerIn: parent
                            width: 8; height: 12
                            
                            property color arrowColor: backBtn.hovered ? Theme.accentFg : Theme.text
                            
                            ShapePath {
                                strokeColor: backArrow.arrowColor
                                strokeWidth: 2
                                fillColor: "transparent"
                                capStyle: ShapePath.RoundCap
                                joinStyle: ShapePath.RoundJoin
                                
                                startX: 6
                                startY: 2
                                
                                PathLine { x: 2; y: 6 }
                                PathLine { x: 6; y: 10 }
                            }
                        }
                    }
                }

                Column {
                    anchors.verticalCenter: backBtn.verticalCenter
                    Row {
                        spacing: 6
                        Text { text: "Device Directory"; font.pixelSize: 12; color: Theme.textSubtle }
                        Text { text: "/"; font.pixelSize: 12; color: Theme.textSubtle }
                        Text { text: dashboardRoot.selectedHost.name; font.pixelSize: 12; font.bold: true; color: Theme.accentFg }
                    }
                }
            }

            Text {
                id: appsTitle
                text: "Applications Directory"
                font.pixelSize: 22
                font.bold: true
                color: Theme.textStrong
                anchors.top: backHeader.bottom
                anchors.topMargin: 12
            }

            Text {
                id: appsSubtitle
                text: "Select and stream apps from " + dashboardRoot.selectedHost.name
                font.pixelSize: 12
                color: Theme.textSubtle
                anchors.top: appsTitle.bottom
                anchors.topMargin: 4
            }

            // Removed Direct Desktop button as it is already included as a standard app option by default

            // Apps Grid
            ScrollView {
                anchors.top: appsSubtitle.bottom
                anchors.bottom: parent.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.topMargin: 32
                clip: true

                GridView {
                    id: appsGrid
                    anchors.fill: parent
                    topMargin: 8
                    cellWidth: 200
                    cellHeight: 270
                    model: dashboardRoot.appsList.length > 0 ? dashboardRoot.appsList : 0

                    delegate: Item {
                        id: appDelegate
                        width: 200
                        height: 270

                        property bool isCardHovered: cardHover.containsMouse || playMouse.containsMouse

                        // Static background hover trigger (prevents flickering during card translate)
                        MouseArea {
                            id: cardHover
                            width: 180; height: 250
                            hoverEnabled: true
                            onClicked: {
                                dashboardRoot.startSessionRequested(
                                    dashboardRoot.serverUrl,
                                    dashboardRoot.token,
                                    dashboardRoot.selectedHost.id,
                                    dashboardRoot.selectedHost.name,
                                    modelData.id,
                                    dashboardRoot.configRes,
                                    dashboardRoot.configFps,
                                    dashboardRoot.configCodec,
                                    dashboardRoot.configBitrate,
                                    dashboardRoot.configQueueLimit,
                                    dashboardRoot.configDisableCuda,
                                    dashboardRoot.configRenderBackend,
                                    dashboardRoot.configInputProtocol,
                                    dashboardRoot.configEncoder,
                                    dashboardRoot.configDisplay,
                                    dashboardRoot.configVirtualDisplay
                                );
                            }
                        }

                        Rectangle {
                            id: appCard
                            width: 180
                            height: 250
                            radius: Theme.radiusLg
                            clip: true
                            color: isCardHovered ? Theme.surface2 : Theme.surface
                            Behavior on color { ColorAnimation { duration: 120 } }

                            // Border drawn on top so cover images never hide it
                            Rectangle {
                                anchors.fill: parent
                                radius: parent.radius
                                color: "transparent"
                                border.color: isCardHovered ? Theme.accentFg : Theme.border
                                border.width: 1
                                z: 20
                                Behavior on border.color { ColorAnimation { duration: 120 } }
                            }

                            // Base64 cover image if exists
                            Image {
                                anchors.fill: parent
                                fillMode: Image.PreserveAspectCrop
                                visible: !!modelData.icon_base64
                                source: modelData.icon_base64 ? "data:image/png;base64," + modelData.icon_base64 : ""
                            }

                            // Glassmorphic overlay for cover images on hover
                            Rectangle {
                                anchors.fill: parent
                                color: Theme.surfaceA(0.75)
                                visible: !!modelData.icon_base64
                                opacity: isCardHovered ? 1.0 : 0.0
                                z: 4
                                Behavior on opacity { NumberAnimation { duration: 200 } }
                            }

                            // Fallback Vector Monitor Icon
                            Item {
                                id: monitorIcon
                                anchors.fill: parent
                                visible: !modelData.icon_base64
                                z: 2

                                // Monitor Screen Frame (Centered in the card)
                                Rectangle {
                                    id: monitorScreen
                                    width: 82; height: 52; radius: 7
                                    color: "transparent"
                                    border.color: Theme.ink(0.22)
                                    border.width: 3
                                    anchors.centerIn: parent
                                }

                                // Monitor Stand
                                Rectangle {
                                    id: monitorStand
                                    width: 14; height: 12
                                    color: Theme.ink(0.22)
                                    anchors.top: monitorScreen.bottom
                                    anchors.horizontalCenter: monitorScreen.horizontalCenter
                                }

                                // Monitor Base
                                Rectangle {
                                    id: monitorBase
                                    width: 36; height: 3; radius: 1
                                    color: Theme.ink(0.22)
                                    anchors.top: monitorStand.bottom
                                    anchors.horizontalCenter: monitorScreen.horizontalCenter
                                }
                            }

                            // App Title Text (Aligned near the bottom)
                            Text {
                                id: titleText
                                text: modelData.title
                                font.pixelSize: 14
                                font.weight: Font.DemiBold
                                color: Theme.textStrong
                                anchors.bottom: parent.bottom
                                anchors.bottomMargin: 24
                                anchors.horizontalCenter: parent.horizontalCenter
                                z: 4
                                visible: !modelData.icon_base64 || isCardHovered
                                horizontalAlignment: Text.AlignHCenter
                                width: parent.width - 30
                                wrapMode: Text.Wrap
                            }

                            // Play Button (Just the play triangle in the center, circle border/background removed)
                            Rectangle {
                                id: playButtonContainer
                                width: 60; height: 60
                                color: "transparent"
                                anchors.centerIn: parent
                                z: 10
                                opacity: isCardHovered ? 1.0 : 0.0
                                visible: opacity > 0.0

                                Behavior on opacity { NumberAnimation { duration: 150 } }

                                Canvas {
                                    id: playTriangle
                                    anchors.centerIn: parent
                                    anchors.horizontalCenterOffset: 3
                                    width: 24; height: 24
                                    antialiasing: true
                                    
                                    property color triangleColor: playMouse.containsMouse ? Theme.accentFg : Theme.textStrong
                                    onTriangleColorChanged: requestPaint()

                                    onPaint: {
                                        var ctx = getContext("2d");
                                        ctx.reset();
                                        ctx.fillStyle = triangleColor;
                                        ctx.beginPath();
                                        ctx.moveTo(width * 0.3, height * 0.2);
                                        ctx.lineTo(width * 0.8, height * 0.5);
                                        ctx.lineTo(width * 0.3, height * 0.8);
                                        ctx.closePath();
                                        ctx.fill();
                                    }
                                }

                                MouseArea {
                                    id: playMouse
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    onClicked: {
                                        dashboardRoot.startSessionRequested(
                                            dashboardRoot.serverUrl,
                                            dashboardRoot.token,
                                            dashboardRoot.selectedHost.id,
                                            dashboardRoot.selectedHost.name,
                                            modelData.id,
                                            dashboardRoot.configRes,
                                            dashboardRoot.configFps,
                                            dashboardRoot.configCodec,
                                            dashboardRoot.configBitrate,
                                            dashboardRoot.configQueueLimit,
                                            dashboardRoot.configDisableCuda,
                                            dashboardRoot.configRenderBackend,
                                            dashboardRoot.configInputProtocol,
                                            dashboardRoot.configEncoder,
                                            dashboardRoot.configDisplay,
                                            dashboardRoot.configVirtualDisplay
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Empty games state view
            Column {
                anchors.centerIn: parent
                visible: dashboardRoot.appsList.length === 0 && !dashboardRoot.appsLoading
                spacing: 12
                Text { text: "No games/apps available"; font.pixelSize: 14; color: Theme.textMuted; anchors.horizontalCenter: parent.horizontalCenter }
            }
        }
    }

    Rectangle {
        id: deleteHostOverlay
        anchors.fill: parent
        z: 1001
        visible: dashboardRoot.pendingDeleteHost !== null
        color: Theme.surfaceA(0.78)

        MouseArea {
            anchors.fill: parent
            onClicked: dashboardRoot.pendingDeleteHost = null
        }

        Rectangle {
            id: deleteHostCard
            width: 440
            height: 190
            anchors.centerIn: parent
            radius: 14
            color: Theme.surface
            border.color: Theme.ink(0.10)
            border.width: 1

            MouseArea { anchors.fill: parent }

            Rectangle {
                id: deleteIconBox
                x: 24; y: 24
                width: 44; height: 44
                radius: 10
                color: Qt.rgba(239/255, 68/255, 68/255, 0.10)
                border.color: Qt.rgba(239/255, 68/255, 68/255, 0.25)
                border.width: 1

                Item {
                    width: 22; height: 22
                    anchors.centerIn: parent
                    Rectangle { x: 5; y: 7; width: 12; height: 12; radius: 1.5; color: "transparent"; border.color: "#ef4444"; border.width: 1.8 }
                    Rectangle { x: 3; y: 4; width: 16; height: 2; radius: 1; color: "#ef4444" }
                    Rectangle { x: 8; y: 1.5; width: 6; height: 2; radius: 1; color: "#ef4444" }
                    Rectangle { x: 8; y: 10; width: 1.5; height: 6; radius: 0.75; color: "#ef4444" }
                    Rectangle { x: 13; y: 10; width: 1.5; height: 6; radius: 0.75; color: "#ef4444" }
                }
            }

            Column {
                anchors.left: deleteIconBox.right
                anchors.leftMargin: 16
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.top: parent.top
                anchors.topMargin: 24
                spacing: 7

                Text {
                    text: "Delete Host"
                    color: Theme.textStrong
                    font.pixelSize: 18
                    font.bold: true
                }
                Text {
                    width: parent.width
                    wrapMode: Text.Wrap
                    color: Theme.text
                    font.pixelSize: 13
                    lineHeight: 1.25
                    text: dashboardRoot.pendingDeleteHost
                        ? "Delete host '" + dashboardRoot.pendingDeleteHost.name + "' from this server? Active sessions for this host will be stopped."
                        : "Delete this host?"
                }
            }

            Row {
                anchors.right: parent.right
                anchors.rightMargin: 24
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 22
                spacing: 10

                Button {
                    id: cancelDeleteBtn
                    text: "Cancel"
                    onClicked: dashboardRoot.pendingDeleteHost = null
                    background: Rectangle {
                        implicitWidth: 96; implicitHeight: 36; radius: 8
                        color: cancelDeleteBtn.hovered ? Theme.ink(0.05) : "transparent"
                        border.color: Theme.ink(0.16)
                        border.width: 1
                    }
                    contentItem: Text { text: cancelDeleteBtn.text; color: cancelDeleteBtn.hovered ? Theme.textStrong : Theme.text; font.bold: true; font.pixelSize: 13; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                }

                Button {
                    id: confirmDeleteBtn
                    text: "Delete Host"
                    onClicked: {
                        if (dashboardRoot.pendingDeleteHost) {
                            dashboardRoot.hostsLoading = true;
                            bridge.deleteHost(dashboardRoot.pendingDeleteHost.id);
                            dashboardRoot.pendingDeleteHost = null;
                        }
                    }
                    background: Rectangle {
                        implicitWidth: 120; implicitHeight: 36; radius: 8
                        color: confirmDeleteBtn.hovered ? "#dc2626" : "#ef4444"
                        border.color: Qt.rgba(239/255, 68/255, 68/255, 0.35)
                        border.width: 1
                    }
                    contentItem: Text { text: confirmDeleteBtn.text; color: Theme.textStrong; font.bold: true; font.pixelSize: 13; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                }
            }
        }
    }

    // Settings Modal
    Settings {
        id: settingsModal
        anchors.centerIn: parent
        z: 1000

        onApplySettings: (res, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol, encoder, displayId, virtualDisplay) => {
            dashboardRoot.configRes = res;
            dashboardRoot.configFps = fps;
            dashboardRoot.configCodec = codec;
            dashboardRoot.configBitrate = bitrate;
            dashboardRoot.configQueueLimit = queueLimit;
            dashboardRoot.configDisableCuda = disableCuda;
            dashboardRoot.configRenderBackend = renderBackend;
            dashboardRoot.configInputProtocol = inputProtocol;
            dashboardRoot.configEncoder = encoder;
            dashboardRoot.configDisplay = displayId;
            dashboardRoot.configVirtualDisplay = virtualDisplay;
        }

        onConfigureVirtualDisplay: (enable, refreshHz) => {
            if (dashboardRoot.serverUrl && dashboardRoot.token && dashboardRoot.activeConfigHost && dashboardRoot.activeConfigHost.id) {
                bridge.configureVirtualDisplay(dashboardRoot.serverUrl, dashboardRoot.token, dashboardRoot.activeConfigHost.id, enable, 1920, 1080, refreshHz);
            }
        }
    }
}
