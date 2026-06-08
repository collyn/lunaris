import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Shapes

Rectangle {
    id: dashboardRoot
    anchors.fill: parent
    color: "#0a0b10"

    // Exposed properties/signals
    property string token: ""
    property string username: ""
    property string serverUrl: ""

    signal startSessionRequested(string server, string token, string hostId, string hostName, int appId, string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol, string encoder, string displayId, bool virtualDisplay)

    // Internal UI state properties
    property bool isLoggedIn: false
    property string authError: ""
    property bool authLoading: false


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
    property bool configDisableCuda: Qt.platform.os === "linux"
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



    // Soft fading decorative orbs matching React Web ambient glow
    Item {
        id: blueOrb
        x: -150; y: -150
        width: 500; height: 500
        z: 0
        Rectangle { anchors.centerIn: parent; width: 500; height: 500; radius: 250; color: Qt.rgba(0/255, 240/255, 255/255, 0.005) }
        Rectangle { anchors.centerIn: parent; width: 400; height: 400; radius: 200; color: Qt.rgba(0/255, 240/255, 255/255, 0.01) }
        Rectangle { anchors.centerIn: parent; width: 300; height: 300; radius: 150; color: Qt.rgba(0/255, 240/255, 255/255, 0.025) }
        Rectangle { anchors.centerIn: parent; width: 200; height: 200; radius: 100; color: Qt.rgba(0/255, 240/255, 255/255, 0.04) }
    }
    Item {
        id: purpleOrb
        x: parent.width - 350; y: parent.height - 350
        width: 600; height: 600
        z: 0
        Rectangle { anchors.centerIn: parent; width: 600; height: 600; radius: 300; color: Qt.rgba(99/255, 102/255, 241/255, 0.005) }
        Rectangle { anchors.centerIn: parent; width: 480; height: 480; radius: 240; color: Qt.rgba(99/255, 102/255, 241/255, 0.01) }
        Rectangle { anchors.centerIn: parent; width: 360; height: 360; radius: 180; color: Qt.rgba(99/255, 102/255, 241/255, 0.025) }
        Rectangle { anchors.centerIn: parent; width: 240; height: 240; radius: 120; color: Qt.rgba(99/255, 102/255, 241/255, 0.04) }
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
                serverInput.forceActiveFocus();
            }

            // Central glass card
            Rectangle {
                anchors.centerIn: parent
                width: 420
                height: 440
                radius: 20
                color: Qt.rgba(15/255, 22/255, 38/255, 0.8)
                border.color: Qt.rgba(255/255, 255/255, 255/255, 0.08)
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
                        text: "L U N A R I S"
                        font.pixelSize: 28
                        font.bold: true
                        color: "#00f0ff"
                        anchors.horizontalCenter: parent.horizontalCenter
                    }

                    Text {
                        text: "Sign in to connect to hosts"
                        font.pixelSize: 12
                        color: "#94a3b8"
                        anchors.horizontalCenter: parent.horizontalCenter
                    }

                    Item { width: 1; height: 10 } // Spacer

                    // Inputs
                    Column {
                        width: parent.width
                        spacing: 12

                        // Server Address
                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: "Signaling Server Host"; color: "#cbd5e1"; font.pixelSize: 11; font.bold: true }
                            Rectangle {
                                width: parent.width; height: 38; color: "#0f172a"; radius: 8; border.color: serverInput.activeFocus ? "#00f0ff" : Qt.rgba(255, 255, 255, 0.08)
                                border.width: 1
                                TextInput {
                                    id: serverInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: "#ffffff"; font.pixelSize: 13
                                    text: "http://127.0.0.1:8080"
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
                            Text { text: "Username"; color: "#cbd5e1"; font.pixelSize: 11; font.bold: true }
                            Rectangle {
                                width: parent.width; height: 38; color: "#0f172a"; radius: 8; border.color: usernameInput.activeFocus ? "#00f0ff" : Qt.rgba(255, 255, 255, 0.08)
                                border.width: 1
                                TextInput {
                                    id: usernameInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: "#ffffff"; font.pixelSize: 13
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
                            Text { text: "Password"; color: "#cbd5e1"; font.pixelSize: 11; font.bold: true }
                            Rectangle {
                                width: parent.width; height: 38; color: "#0f172a"; radius: 8; border.color: passwordInput.activeFocus ? "#00f0ff" : Qt.rgba(255, 255, 255, 0.08)
                                border.width: 1
                                TextInput {
                                    id: passwordInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: "#ffffff"; font.pixelSize: 13
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

                    // Submit Button
                    Button {
                        id: submitButton
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
                            color: submitButton.hovered ? "#00e0ff" : "#00f0ff"
                            opacity: dashboardRoot.authLoading ? 0.6 : 1.0
                        }

                        contentItem: Item {
                            anchors.fill: parent
                            Text {
                                text: dashboardRoot.authLoading ? "Authenticating..." : "LOGIN"
                                color: "#000000"
                                font.bold: true
                                font.pixelSize: 13
                                font.letterSpacing: 1
                                anchors.centerIn: parent
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
                color: "#0a0b12"
                border.color: Qt.rgba(255/255, 255/255, 255/255, 0.03)
                border.width: 1

                // Allow dragging the window by holding and dragging the navbar
                DragHandler {
                    target: null
                    onActiveChanged: {
                        if (active) {
                            window.startSystemMove();
                        }
                    }
                }

                // Brand Title & Tech Pill
                Row {
                    anchors.left: parent.left
                    anchors.leftMargin: 24
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 12

                    Text {
                        text: "$"
                        font.pixelSize: 22
                        font.bold: true
                        color: "#00f0ff"
                    }

                    Text {
                        text: "Lunaris"
                        font.pixelSize: 18
                        font.bold: true
                        color: "#ffffff"
                    }

                    Rectangle {
                        width: 52; height: 18; radius: 9
                        color: Qt.rgba(0/255, 240/255, 255/255, 0.15)
                        anchors.verticalCenter: parent.verticalCenter
                        Text {
                            text: "v0.1.0"
                            color: "#00f0ff"
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

                    // SERVER Info
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        Text {
                            text: "SERVER:"
                            font.pixelSize: 9
                            font.bold: true
                            color: "#64748b"
                            anchors.right: parent.right
                        }
                        Text {
                            text: dashboardRoot.serverUrl
                            font.pixelSize: 12
                            font.bold: true
                            color: "#00f0ff"
                        }
                    }

                    // USER Info
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        Text {
                            text: "USER:"
                            font.pixelSize: 9
                            font.bold: true
                            color: "#64748b"
                            anchors.right: parent.right
                        }
                        Text {
                            text: dashboardRoot.username
                            font.pixelSize: 12
                            font.bold: true
                            color: "#ffffff"
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
                            border.color: logoutBtn.hovered ? "#ef4444" : Qt.rgba(255,255,255,0.08)
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
                                    color: "transparent"; border.color: logoutBtn.hovered ? "#ef4444" : "#cbd5e1"; border.width: 1.5
                                }
                                Rectangle {
                                    x: 10; y: 3.5; width: 2; height: 9; color: "#0a0b12"
                                }
                                Rectangle {
                                    x: 6; y: 7.25; width: 8; height: 1.5; color: logoutBtn.hovered ? "#ef4444" : "#cbd5e1"
                                }
                                Rectangle {
                                    x: 10; y: 4.5; width: 1.5; height: 4.5; rotation: 45; color: logoutBtn.hovered ? "#ef4444" : "#cbd5e1"; antialiasing: true
                                }
                                Rectangle {
                                    x: 10; y: 7; width: 1.5; height: 4.5; rotation: -45; color: logoutBtn.hovered ? "#ef4444" : "#cbd5e1"; antialiasing: true
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
                anchors.margins: 40

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
                        color: "#ffffff"
                    }
                    Text {
                        text: "Manage and connect to active remote agent streams"
                        font.pixelSize: 13
                        color: "#64748b"
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
                            color: syncBtn.hovered ? Qt.rgba(255,255,255,0.05) : "transparent"
                            border.color: syncBtn.hovered ? "#00f0ff" : Qt.rgba(255,255,255,0.15)
                            border.width: 1
                        }
                        contentItem: Text {
                            text: "Sync Devices"
                            color: "#cbd5e1"
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

                        property bool isHostHovered: cardMouseArea.containsMouse || gearBtn.hovered || deleteBtn.hovered

                        // Static background click handler to avoid hover-translation flicker
                        MouseArea {
                            id: cardMouseArea
                            width: 240; height: 160
                            hoverEnabled: true
                            onClicked: {
                                if (modelData.status === "Online") {
                                    dashboardRoot.selectedHost = modelData;
                                    dashboardRoot.appsLoading = true;
                                    dashboardRoot.appsList = [];
                                    bridge.fetchApps(modelData.id);
                                }
                            }
                        }

                        // Device card (translates up 4px smoothly on hover based on parent static MouseArea)
                        Rectangle {
                            id: card
                            width: 240
                            height: 160
                            radius: 14
                            color: Qt.rgba(15/255, 22/255, 38/255, 0.8)
                            border.color: isHostHovered ? Qt.rgba(0, 240/255, 255/255, 0.3) : Qt.rgba(255, 255, 255, 0.08)
                            border.width: 1

                            y: isHostHovered ? -4 : 0
                            Behavior on y { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }
                            Behavior on border.color { ColorAnimation { duration: 150 } }

                            // Center-fade horizontal gradient glowing top edge matching React
                            Rectangle {
                                width: parent.width; height: 3
                                anchors.top: parent.top
                                radius: 3
                                gradient: Gradient {
                                    orientation: Gradient.Horizontal
                                    GradientStop { position: 0.0; color: "transparent" }
                                    GradientStop { position: 0.5; color: modelData.status === "Online" ? "#00f0ff" : (modelData.status === "Busy" ? "#f59e0b" : "#ef4444") }
                                    GradientStop { position: 1.0; color: "transparent" }
                                }
                            }

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
                                        color: "#ffffff"
                                        elide: Text.ElideRight
                                        width: parent.width
                                    }
                                    Text {
                                        text: "ID: " + modelData.id.substring(0, 8) + "..."
                                        font.pixelSize: 11
                                        font.family: "monospace"
                                        color: "#64748b"
                                    }
                                }

                                // Divider top
                                Rectangle {
                                    width: parent.width; height: 1
                                    anchors.top: hostInfo.bottom
                                    anchors.topMargin: 8
                                    color: Qt.rgba(255, 255, 255, 0.03)
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
                                        Text { text: "IP Address"; font.pixelSize: 11; color: "#64748b"; width: 80 }
                                        Text { text: modelData.ip_address || "Connected via Agent"; font.pixelSize: 11; font.bold: true; font.family: "monospace"; color: "#cbd5e1" }
                                    }

                                    Row {
                                        width: parent.width
                                        Text { text: "Status"; font.pixelSize: 11; color: "#64748b"; width: 80 }
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

                                // Card actions footer (bottom right, sits on top of visual card)
                                Row {
                                    id: cardFooter
                                    anchors.right: parent.right
                                    anchors.bottom: parent.bottom
                                    spacing: 8
                                    z: 10

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
                                                dashboardRoot.configDisableCuda = (Qt.platform.os === "linux");
                                                dashboardRoot.configRenderBackend = dashboardRoot.configDisableCuda ? "software" : "auto_gpu";
                                                dashboardRoot.configInputProtocol = "webrtc";
                                                dashboardRoot.configEncoder = "auto";
                                                dashboardRoot.configDisplay = "default";
                                                dashboardRoot.configVirtualDisplay = false;
                                            }
                                            dashboardRoot.activeConfigHost = modelData;
                                            settingsModal.setCurrentSettings(dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec, dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda, dashboardRoot.configInputProtocol, dashboardRoot.configEncoder, dashboardRoot.configDisplay, dashboardRoot.configVirtualDisplay, dashboardRoot.configRenderBackend);
                                            settingsModal.open();
                                        }
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: gearBtn.hovered ? Qt.rgba(0/255, 240/255, 255/255, 0.1) : "transparent"
                                            border.color: gearBtn.hovered ? "#00f0ff" : "transparent"
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Item {
                                                width: 16; height: 16
                                                anchors.centerIn: parent
                                                Rectangle {
                                                    anchors.centerIn: parent; width: 10; height: 10; radius: 5
                                                    color: "transparent"; border.color: gearBtn.hovered ? "#00f0ff" : "#cbd5e1"; border.width: 1.8
                                                }
                                                Repeater {
                                                    model: 8
                                                    Rectangle {
                                                        anchors.centerIn: parent; width: 2.2; height: 13.5
                                                        color: gearBtn.hovered ? "#00f0ff" : "#cbd5e1"; rotation: index * 45; antialiasing: true
                                                    }
                                                }
                                                Rectangle {
                                                    anchors.centerIn: parent; width: 6.4; height: 6.4; radius: 3.2; color: "#0f172a"
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
                                                    border.color: deleteBtn.hovered ? "#ef4444" : "#cbd5e1"
                                                    border.width: 1.4
                                                }
                                                Rectangle {
                                                    x: 3; y: 3; width: 10; height: 1.5; radius: 0.75
                                                    color: deleteBtn.hovered ? "#ef4444" : "#cbd5e1"
                                                }
                                                Rectangle { x: 6; y: 1.5; width: 4; height: 1.4; radius: 0.7; color: deleteBtn.hovered ? "#ef4444" : "#cbd5e1" }
                                                Rectangle { x: 6.2; y: 7; width: 1.2; height: 5; radius: 0.6; color: deleteBtn.hovered ? "#ef4444" : "#cbd5e1" }
                                                Rectangle { x: 9; y: 7; width: 1.2; height: 5; radius: 0.6; color: deleteBtn.hovered ? "#ef4444" : "#cbd5e1" }
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
                Text { text: "No agents connected yet"; font.pixelSize: 16; font.bold: true; color: "#94a3b8"; anchors.horizontalCenter: parent.horizontalCenter }
                Text { text: "Agents will appear here automatically once registered"; font.pixelSize: 12; color: "#64748b"; anchors.horizontalCenter: parent.horizontalCenter }
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
                        color: backBtn.hovered ? Qt.rgba(0, 240/255, 255/255, 0.1) : "transparent"
                        border.color: backBtn.hovered ? "#00f0ff" : "transparent"
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
                            
                            property color arrowColor: backBtn.hovered ? "#00f0ff" : "#cbd5e1"
                            
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
                        Text { text: "Device Directory"; font.pixelSize: 12; color: "#64748b" }
                        Text { text: "/"; font.pixelSize: 12; color: "#64748b" }
                        Text { text: dashboardRoot.selectedHost.name; font.pixelSize: 12; font.bold: true; color: "#00f0ff" }
                    }
                }
            }

            Text {
                id: appsTitle
                text: "Applications Directory"
                font.pixelSize: 22
                font.bold: true
                color: "#ffffff"
                anchors.top: backHeader.bottom
                anchors.topMargin: 12
            }

            Text {
                id: appsSubtitle
                text: "Select and stream apps from " + dashboardRoot.selectedHost.name
                font.pixelSize: 12
                color: "#64748b"
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
                            radius: 20
                            clip: true
                            color: "#0d0f17"
                            border.color: isCardHovered ? "#00f0ff" : Qt.rgba(255, 255, 255, 0.08)
                            border.width: isCardHovered ? 1.5 : 1

                            // y-axis lift animation on hover
                            y: isCardHovered ? -4 : 0
                            Behavior on y { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }
                            Behavior on border.color { ColorAnimation { duration: 150 } }

                            // Fallback gradient background
                            Rectangle {
                                anchors.fill: parent
                                gradient: Gradient {
                                    GradientStop { position: 0.0; color: "#0d0f17" }
                                    GradientStop { position: 1.0; color: "#181b26" }
                                }
                                visible: !modelData.icon_base64
                            }

                            // Base64 cover image if exists
                            Image {
                                anchors.fill: parent
                                fillMode: Image.PreserveAspectCrop
                                visible: modelData.icon_base64 && modelData.icon_base64.length > 0
                                source: modelData.icon_base64 ? "data:image/png;base64," + modelData.icon_base64 : ""
                            }

                            // Glassmorphic overlay for cover images on hover
                            Rectangle {
                                anchors.fill: parent
                                color: Qt.rgba(10/255, 12/255, 20/255, 0.75)
                                visible: modelData.icon_base64 && modelData.icon_base64.length > 0
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
                                    border.color: Qt.rgba(255, 255, 255, 0.12)
                                    border.width: 3.5
                                    anchors.centerIn: parent
                                }

                                // Monitor Stand
                                Rectangle {
                                    id: monitorStand
                                    width: 14; height: 12
                                    color: Qt.rgba(255, 255, 255, 0.12)
                                    anchors.top: monitorScreen.bottom
                                    anchors.horizontalCenter: monitorScreen.horizontalCenter
                                }

                                // Monitor Base
                                Rectangle {
                                    id: monitorBase
                                    width: 36; height: 3.5; radius: 1
                                    color: Qt.rgba(255, 255, 255, 0.12)
                                    anchors.top: monitorStand.bottom
                                    anchors.horizontalCenter: monitorScreen.horizontalCenter
                                }
                            }

                            // App Title Text (Aligned near the bottom)
                            Text {
                                id: titleText
                                text: modelData.title
                                font.pixelSize: 14
                                font.bold: true
                                color: "#ffffff"
                                anchors.bottom: parent.bottom
                                anchors.bottomMargin: 24
                                anchors.horizontalCenter: parent.horizontalCenter
                                z: 4
                                visible: !modelData.icon_base64 || isCardHovered
                                horizontalAlignment: Text.AlignHCenter
                                width: parent.width - 30
                                wrapMode: Text.Wrap
                            }

                            // Watermark Text (uppercase behind title)
                            Text {
                                id: watermarkText
                                text: modelData.title ? modelData.title.toUpperCase() : ""
                                font.pixelSize: 28
                                font.bold: true
                                font.letterSpacing: 2
                                color: Qt.rgba(255, 255, 255, 0.03)
                                anchors.centerIn: titleText
                                z: 3
                                visible: !modelData.icon_base64 || isCardHovered
                                horizontalAlignment: Text.AlignHCenter
                                width: parent.width - 20
                                elide: Text.ElideRight
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
                                    
                                    property color triangleColor: playMouse.containsMouse ? "#00f0ff" : "#ffffff"
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
                Text { text: "No games/apps available"; font.pixelSize: 14; color: "#94a3b8"; anchors.horizontalCenter: parent.horizontalCenter }
            }
        }
    }

    Rectangle {
        id: deleteHostOverlay
        anchors.fill: parent
        z: 1001
        visible: dashboardRoot.pendingDeleteHost !== null
        color: Qt.rgba(4/255, 6/255, 10/255, 0.78)

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
            color: "#0f1626"
            border.color: Qt.rgba(255, 255, 255, 0.10)
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
                    color: "#ffffff"
                    font.pixelSize: 18
                    font.bold: true
                }
                Text {
                    width: parent.width
                    wrapMode: Text.Wrap
                    color: "#cbd5e1"
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
                        color: cancelDeleteBtn.hovered ? Qt.rgba(255, 255, 255, 0.05) : "transparent"
                        border.color: Qt.rgba(255, 255, 255, 0.16)
                        border.width: 1
                    }
                    contentItem: Text { text: cancelDeleteBtn.text; color: cancelDeleteBtn.hovered ? "#ffffff" : "#cbd5e1"; font.bold: true; font.pixelSize: 13; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
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
                    contentItem: Text { text: confirmDeleteBtn.text; color: "#ffffff"; font.bold: true; font.pixelSize: 13; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
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
    }
}
