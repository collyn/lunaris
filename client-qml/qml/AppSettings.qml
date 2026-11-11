import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Settings: a section list on the left, the section's cards on the right.
Rectangle {
    id: settings
    color: Theme.bg

    // The Home screen: prefs, this computer's status and actions.
    property var homeView: null
    property string section: "general"
    property string portError: ""

    readonly property var sections: [
        { key: "general", label: "General", icon: "settings" },
        { key: "security", label: "Security", icon: "shield" },
        { key: "network", label: "Network", icon: "link" },
        { key: "display", label: "Display", icon: "monitor" },
        { key: "about", label: "About", icon: "info" }
    ]

    function pref(key) { return homeView ? homeView.prefs[key] : undefined }
    function setPref(key, value) { if (homeView) homeView.setPref(key, value) }

    // ---------------- building blocks ----------------
    component Card: Rectangle {
        default property alias content: cardBody.data
        property string title: ""
        property string subtitle: ""
        Layout.fillWidth: true
        Layout.maximumWidth: 620
        implicitHeight: cardBody.y + cardBody.implicitHeight + 20
        radius: Theme.radiusLg
        color: Theme.surface
        border.color: Theme.border
        Text {
            id: cardTitle
            visible: parent.title !== ""
            x: 20; y: 16
            text: parent.title
            color: Theme.textStrong
            font.pixelSize: 16
            font.weight: Font.DemiBold
        }
        Text {
            id: cardSubtitle
            visible: parent.subtitle !== ""
            x: 20
            anchors.top: cardTitle.bottom
            anchors.topMargin: 4
            width: parent.width - 40
            text: parent.subtitle
            color: Theme.textMuted
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }
        ColumnLayout {
            id: cardBody
            x: 20
            y: (parent.title !== "" ? 50 : 18) + (parent.subtitle !== "" ? cardSubtitle.implicitHeight + 6 : 0)
            width: parent.width - 40
            spacing: 10
        }
    }

    component SettingRow: RowLayout {
        property string label: ""
        property string hint: ""
        default property alias control: rowControl.data
        Layout.fillWidth: true
        spacing: 16
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Text { text: label; color: Theme.text; font.pixelSize: 13; font.weight: Font.Medium }
            Text { visible: hint !== ""; Layout.fillWidth: true; text: hint; color: Theme.textSubtle; font.pixelSize: 11; wrapMode: Text.WordWrap }
        }
        Item {
            id: rowControl
            implicitWidth: childrenRect.width
            implicitHeight: Math.max(childrenRect.height, 30)
        }
    }

    // Choice between a few values: segmented buttons.
    component Segmented: Row {
        id: seg
        property var options: []      // [{ value, label }]
        property var value
        signal picked(var value)
        spacing: 0
        Repeater {
            model: seg.options
            delegate: AbstractButton {
                id: segBtn
                readonly property bool active: String(seg.value) === String(modelData.value)
                hoverEnabled: true
                implicitHeight: 30
                implicitWidth: segText.implicitWidth + 22
                Accessible.name: modelData.label
                onClicked: seg.picked(modelData.value)
                background: Rectangle {
                    color: segBtn.active ? Theme.accentA(0.18) : (segBtn.hovered ? Theme.surface2 : Theme.bg)
                    border.color: segBtn.active ? Theme.accent : Theme.borderStrong
                    radius: 0
                    Rectangle { visible: index === 0; anchors.left: parent.left; width: 1; height: parent.height; color: parent.border.color }
                }
                contentItem: Text {
                    id: segText
                    text: modelData.label
                    color: segBtn.active ? Theme.accentFg : Theme.text
                    font.pixelSize: 12
                    font.weight: segBtn.active ? Font.DemiBold : Font.Normal
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
        }
    }

    // ---------------- section list ----------------
    Rectangle {
        id: nav
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: 220
        color: Theme.surface
        Rectangle { anchors.right: parent.right; anchors.top: parent.top; anchors.bottom: parent.bottom; width: 1; color: Theme.border }

        Column {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.topMargin: 20
            anchors.leftMargin: 12
            anchors.rightMargin: 12
            spacing: 2

            Text { text: "Settings"; color: Theme.textStrong; font.pixelSize: 19; font.weight: Font.DemiBold; leftPadding: 10; bottomPadding: 14 }

            Repeater {
                model: settings.sections
                delegate: AbstractButton {
                    id: navBtn
                    readonly property bool current: settings.section === modelData.key
                    width: parent.width
                    implicitHeight: 38
                    hoverEnabled: true
                    onClicked: settings.section = modelData.key
                    Accessible.name: modelData.label
                    background: Rectangle {
                        radius: Theme.radiusSm
                        color: navBtn.current ? Theme.accentA(0.12) : (navBtn.hovered ? Theme.surface2 : "transparent")
                        Rectangle { visible: navBtn.current; width: 3; height: 18; radius: 1.5; color: Theme.accent; anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter }
                    }
                    contentItem: Row {
                        leftPadding: 12
                        spacing: 12
                        Icon { name: modelData.icon; size: 17; color: navBtn.current ? Theme.accentFg : Theme.textMuted; anchors.verticalCenter: parent.verticalCenter }
                        Text { text: modelData.label; color: navBtn.current ? Theme.textStrong : Theme.text; font.pixelSize: 14; font.weight: navBtn.current ? Font.DemiBold : Font.Normal; anchors.verticalCenter: parent.verticalCenter }
                    }
                }
            }
        }
    }

    // ---------------- section content ----------------
    Flickable {
        id: contentFlick
        anchors.left: nav.right
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        contentHeight: sectionColumn.implicitHeight + 48
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        ColumnLayout {
            id: sectionColumn
            x: 28
            y: 24
            width: Math.min(620, contentFlick.width - 56)
            spacing: 16

            // ===================== General =====================
            Card {
                visible: settings.section === "general"
                title: "Remote control of this computer"
                subtitle: "While on, other computers can control this one with its address and password (Home)."
                SettingRow {
                    label: homeView && homeView.hostStatus.running ? "On" : (homeView && homeView.hostStatus.enabled ? "Starting…" : "Off")
                    hint: "Starts again with the app."
                    PrimaryButton {
                        visible: !(homeView && homeView.hostStatus.enabled)
                        text: "Turn on"
                        onClicked: homeView.setRemoteControl(true)
                    }
                    SecondaryButton {
                        visible: !!(homeView && homeView.hostStatus.enabled)
                        text: "Turn off"
                        danger: true
                        onClicked: homeView.setRemoteControl(false)
                    }
                }
            }

            Card {
                visible: settings.section === "general"
                title: "Theme"
                Segmented {
                    options: [{ value: "system", label: "Follow system" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]
                    value: settings.pref("theme") || "system"
                    onPicked: (v) => settings.setPref("theme", v)
                }
            }

            Card {
                visible: settings.section === "general"
                title: "Startup"
                CheckRow {
                    text: "Start Lunaris when I sign in"
                    checked: bridge.isAutostartEnabled()
                    onToggled: {
                        bridge.setAutostartEnabled(checked);
                        window.autostartEnabled = bridge.isAutostartEnabled();
                    }
                }
                CheckRow {
                    text: "Keep running in the system tray when the window is closed"
                    checked: settings.pref("closeToTray") !== false
                    onToggled: {
                        settings.setPref("closeToTray", checked);
                        window.closeToTray = checked;
                    }
                }
            }

            Card {
                visible: settings.section === "general"
                title: "Updates"
                SettingRow {
                    label: "Lunaris " + bridge.appVersion()
                    hint: "Checks GitHub for a newer release."
                    SecondaryButton { text: "Check now"; onClicked: bridge.checkForUpdates() }
                }
            }

            // ===================== Security =====================
            Card {
                visible: settings.section === "security"
                title: "Access password"
                subtitle: "Anyone with this computer's address and this password can control it."
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Text {
                        Layout.fillWidth: true
                        text: homeView ? (homeView.passwordVisible ? homeView.hostStatus.password : homeView.hostStatus.password.replace(/./g, "•")) : ""
                        color: Theme.textStrong
                        font.pixelSize: 16
                        font.family: homeView && homeView.passwordVisible ? "monospace" : Qt.application.font.family
                    }
                    IconButton {
                        iconName: homeView && homeView.passwordVisible ? "eyeOff" : "eye"
                        tip: homeView && homeView.passwordVisible ? "Hide password" : "Show password"
                        onClicked: homeView.passwordVisible = !homeView.passwordVisible
                    }
                }
                RowLayout {
                    spacing: 8
                    SecondaryButton {
                        text: "New random password"
                        onClicked: { if (homeView.setHostPassword("") === "") homeView.passwordVisible = true; }
                    }
                    InputField {
                        id: securityPassword
                        Layout.fillWidth: false
                        implicitWidth: 180
                        implicitHeight: 34
                        fontSize: 13
                        placeholder: "Or type a password"
                        onAccepted: savePwBtn.clicked()
                    }
                    SecondaryButton {
                        id: savePwBtn
                        text: "Save"
                        enabled: securityPassword.text.trim().length > 0
                        onClicked: {
                            if (homeView.setHostPassword(securityPassword.text) === "") {
                                securityPassword.text = "";
                                homeView.passwordVisible = true;
                            }
                        }
                    }
                }
                Text {
                    visible: homeView && homeView.hostError !== ""
                    text: homeView ? homeView.hostError : ""
                    color: Theme.danger
                    font.pixelSize: 12
                }
            }

            Card {
                visible: settings.section === "security"
                title: "Wrong passwords"
                Text {
                    Layout.fillWidth: true
                    text: "After 5 wrong passwords in a minute from the same address, that address has to wait a minute before trying again."
                    color: Theme.textMuted
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
            }

            // ===================== Network =====================
            Card {
                visible: settings.section === "network"
                title: "Direct connections"
                subtitle: "Port other computers use to reach this one. Allow it in the firewall to connect from your network."
                SettingRow {
                    label: "Port"
                    hint: "Default 8091. With another port, other computers connect to address:port."
                    RowLayout {
                        spacing: 8
                        InputField {
                            id: portField
                            Layout.fillWidth: false
                            implicitWidth: 90
                            implicitHeight: 34
                            fontSize: 13
                            text: homeView ? String(homeView.hostStatus.port) : "8091"
                            input.validator: IntValidator { bottom: 1; top: 65535 }
                            onAccepted: savePortBtn.clicked()
                        }
                        SecondaryButton {
                            id: savePortBtn
                            text: "Save"
                            enabled: homeView && portField.text !== String(homeView.hostStatus.port)
                            onClicked: settings.portError = homeView.setHostPort(parseInt(portField.text))
                        }
                    }
                }
                Text { visible: settings.portError !== ""; text: settings.portError; color: Theme.danger; font.pixelSize: 12 }
            }

            LocalServerCard {
                visible: settings.section === "network"
                Layout.fillWidth: true
                Layout.maximumWidth: 620
                homeView: settings.homeView
            }

            Card {
                visible: settings.section === "network"
                title: "Server port"
                SettingRow {
                    label: "Port"
                    hint: "Default 8090. Change it if another program uses that port."
                    RowLayout {
                        spacing: 8
                        InputField {
                            id: serverPortField
                            Layout.fillWidth: false
                            implicitWidth: 90
                            implicitHeight: 34
                            fontSize: 13
                            text: homeView ? String(homeView.serverStatus.port) : "8090"
                            input.validator: IntValidator { bottom: 1; top: 65535 }
                            onAccepted: saveServerPortBtn.clicked()
                        }
                        SecondaryButton {
                            id: saveServerPortBtn
                            text: "Save"
                            enabled: homeView && serverPortField.text !== String(homeView.serverStatus.port)
                            onClicked: homeView.setServerPort(parseInt(serverPortField.text))
                        }
                    }
                }
            }

            Card {
                visible: settings.section === "network"
                title: "Signed-in server"
                subtitle: "A server lets you reach computers outside your network and manage users, groups and computers from its web page."
                SettingRow {
                    label: homeView && homeView.dashboard.isLoggedIn ? "Signed in as " + homeView.dashboard.username : "Not signed in"
                    hint: homeView && homeView.dashboard.isLoggedIn ? homeView.dashboard.serverUrl : "Sign in from Home > Server."
                    RowLayout {
                        spacing: 8
                        SecondaryButton {
                            visible: !!(homeView && homeView.dashboard.isLoggedIn)
                            text: "Open web page"
                            onClicked: homeView.openWebConsole(homeView.dashboard.serverUrl)
                        }
                        SecondaryButton {
                            text: homeView && homeView.dashboard.isLoggedIn ? "Sign out" : "Sign in"
                            onClicked: {
                                if (homeView.dashboard.isLoggedIn) {
                                    bridge.logout();
                                    homeView.dashboard.isLoggedIn = false;
                                    homeView.dashboard.token = "";
                                    homeView.dashboard.username = "";
                                } else {
                                    homeView.peerTab = "server";
                                    homeView.page = "home";
                                }
                            }
                        }
                    }
                }
            }

            // ===================== Display =====================
            Card {
                visible: settings.section === "display"
                title: "New connections"
                subtitle: "Used when you connect to a computer by its address. You can change them during a session from the menu at the top of the screen."

                SettingRow {
                    label: "Resolution"
                    Segmented {
                        options: [{ value: "1280x720", label: "720p" }, { value: "1920x1080", label: "1080p" }, { value: "2560x1440", label: "1440p" }, { value: "3840x2160", label: "4K" }]
                        value: settings.pref("res")
                        onPicked: (v) => settings.setPref("res", v)
                    }
                }
                SettingRow {
                    label: "Frame rate"
                    Segmented {
                        options: [{ value: 30, label: "30" }, { value: 60, label: "60" }, { value: 120, label: "120" }, { value: 144, label: "144" }]
                        value: settings.pref("fps")
                        onPicked: (v) => settings.setPref("fps", v)
                    }
                }
                SettingRow {
                    label: "Quality (bitrate)"
                    hint: "Higher looks sharper but needs a faster network."
                    Segmented {
                        options: [{ value: 5000, label: "5" }, { value: 10000, label: "10" }, { value: 15000, label: "15" }, { value: 20000, label: "20" }, { value: 30000, label: "30" }, { value: 50000, label: "50 Mbps" }]
                        value: settings.pref("bitrate")
                        onPicked: (v) => settings.setPref("bitrate", v)
                    }
                }
                SettingRow {
                    label: "Codec"
                    hint: "H.264 4:4:4 keeps text crisp but is encoded in software unless the other computer has an NVIDIA GPU."
                    Segmented {
                        options: [{ value: "h264", label: "H.264" }, { value: "h264_444", label: "H.264 4:4:4" }, { value: "h265", label: "H.265" }, { value: "av1", label: "AV1" }]
                        value: settings.pref("codec")
                        onPicked: (v) => settings.setPref("codec", v)
                    }
                }
                SettingRow {
                    label: "Video decoding"
                    hint: "Software is slower; use it if the picture stays black."
                    Segmented {
                        options: [{ value: "auto_gpu", label: "GPU" }, { value: "software", label: "Software" }]
                        value: settings.pref("renderBackend")
                        onPicked: (v) => settings.setPref("renderBackend", v)
                    }
                }
            }

            // ===================== About =====================
            Card {
                visible: settings.section === "about"
                title: "Lunaris " + bridge.appVersion()
                subtitle: "Low-latency remote desktop. Free software under the GNU GPL v3 or later."
                RowLayout {
                    spacing: 8
                    SecondaryButton { text: "Website"; onClicked: Qt.openUrlExternally("https://github.com/collyn/lunaris") }
                    SecondaryButton { text: "Report a problem"; onClicked: Qt.openUrlExternally("https://github.com/collyn/lunaris/issues") }
                    SecondaryButton { text: "Releases"; onClicked: Qt.openUrlExternally("https://github.com/collyn/lunaris/releases") }
                }
            }
        }
    }
}
