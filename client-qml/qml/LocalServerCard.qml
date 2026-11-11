import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// "Server on this computer": start/stop the app's own Lunaris server, its
// address, the first admin password, the web console and one-click sign-in.
Rectangle {
    id: card
    property var homeView: null
    readonly property var st: homeView ? homeView.serverStatus : ({})
    readonly property bool running: !!st.running
    readonly property bool signedInHere: homeView && homeView.dashboard.isLoggedIn
        && homeView.dashboard.serverUrl.indexOf("127.0.0.1:" + st.port) !== -1
    property bool adminPasswordVisible: false

    implicitHeight: body.implicitHeight + 40
    radius: Theme.radiusLg
    color: Theme.surface
    border.color: running ? Qt.rgba(34 / 255, 197 / 255, 94 / 255, 0.30) : Theme.border

    ColumnLayout {
        id: body
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 20
        spacing: 12

        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Icon { name: "server"; size: 20; color: card.running ? Theme.success : Theme.accentFg }
            Text {
                Layout.fillWidth: true
                text: I18n.t("Server on this computer")
                color: Theme.textStrong
                font.pixelSize: 16
                font.weight: Font.DemiBold
            }
            Rectangle {
                visible: card.running || (homeView && homeView.serverBusy)
                implicitWidth: stateText.implicitWidth + 16
                implicitHeight: 22
                radius: 11
                color: card.running ? Qt.rgba(34 / 255, 197 / 255, 94 / 255, 0.14) : Theme.surface2
                Text {
                    id: stateText
                    anchors.centerIn: parent
                    text: card.running ? I18n.t("Running") : I18n.t("Starting…")
                    color: card.running ? Theme.success : Theme.textMuted
                    font.pixelSize: 11
                    font.weight: Font.DemiBold
                }
            }
        }

        Text {
            Layout.fillWidth: true
            text: card.running
                ? I18n.t("Other computers sign in and register at the address below. Manage users, groups and computers in the web console.")
                : I18n.t("Run a Lunaris server here to manage users, groups and computers from a web page, and to connect computers on different networks.")
            color: Theme.textMuted
            font.pixelSize: 12
            wrapMode: Text.WordWrap
            lineHeight: 1.15
        }

        // Address for other computers
        ColumnLayout {
            visible: card.running
            Layout.fillWidth: true
            spacing: 2
            Text { text: I18n.t("Server address"); color: Theme.textMuted; font.pixelSize: 11; font.weight: Font.Medium }
            RowLayout {
                Layout.fillWidth: true
                spacing: 2
                Text {
                    Layout.fillWidth: true
                    text: card.st.lan_url || ""
                    color: Theme.textStrong
                    font.pixelSize: 15
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }
                IconButton {
                    iconName: "copy"
                    tip: I18n.t("Copy server address")
                    onClicked: { homeView.copyText(card.st.lan_url); homeView.setStatus(I18n.t("Server address copied"), false); }
                }
            }
        }

        // First admin account
        ColumnLayout {
            visible: card.running && (card.st.admin_password || "") !== ""
            Layout.fillWidth: true
            spacing: 2
            Text { text: I18n.t("Admin sign-in (change the password in the web console)"); color: Theme.textMuted; font.pixelSize: 11; font.weight: Font.Medium; Layout.fillWidth: true; wrapMode: Text.WordWrap }
            RowLayout {
                Layout.fillWidth: true
                spacing: 2
                Text {
                    Layout.fillWidth: true
                    text: "admin  ·  " + (card.adminPasswordVisible ? card.st.admin_password : "••••••••••••")
                    color: Theme.text
                    font.pixelSize: 13
                    font.family: card.adminPasswordVisible ? "monospace" : Qt.application.font.family
                    elide: Text.ElideRight
                }
                IconButton {
                    iconName: card.adminPasswordVisible ? "eyeOff" : "eye"
                    tip: card.adminPasswordVisible ? I18n.t("Hide password") : I18n.t("Show password")
                    onClicked: card.adminPasswordVisible = !card.adminPasswordVisible
                }
                IconButton {
                    iconName: "copy"
                    tip: I18n.t("Copy password")
                    onClicked: { homeView.copyText(card.st.admin_password); homeView.setStatus(I18n.t("Admin password copied"), false); }
                }
            }
        }

        Text {
            Layout.fillWidth: true
            visible: homeView && homeView.serverError !== ""
            text: homeView ? homeView.serverError : ""
            color: Theme.danger
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }

        Flow {
            Layout.fillWidth: true
            spacing: 8
            PrimaryButton {
                visible: !card.running
                enabled: !(homeView && homeView.serverBusy)
                text: homeView && homeView.serverBusy ? I18n.t("Starting…") : I18n.t("Start server")
                onClicked: homeView.setServer(true)
            }
            PrimaryButton {
                visible: card.running
                text: I18n.t("Open web console")
                onClicked: homeView.openWebConsole(card.st.local_url)
            }
            SecondaryButton {
                visible: card.running && !card.signedInHere && (card.st.admin_password || "") !== ""
                text: I18n.t("Show its computers here")
                onClicked: homeView.signInLocal()
            }
            SecondaryButton {
                visible: card.running && card.st.ours
                text: I18n.t("Stop")
                danger: true
                onClicked: homeView.setServer(false)
            }
        }

        Text {
            Layout.fillWidth: true
            visible: card.running && !card.st.ours
            text: I18n.t("This server was started outside the app (port %1).").arg(card.st.port)
            color: Theme.textSubtle
            font.pixelSize: 11
            wrapMode: Text.WordWrap
        }
    }
}
