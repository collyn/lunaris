import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The Updates card of the window shown without a link (Launcher.qml): this version, "Check now", the automatic
// check, and a newer release with its notes, Update and the progress.
// `status` is the parsed bridge.updateStatus() (updater/src/service.rs).
ColumnLayout {
    id: panel
    property var status: ({})
    readonly property string state_: status.state || "idle"
    readonly property bool busy: ["checking", "downloading", "verifying", "installing", "restart"].indexOf(state_) >= 0
    readonly property bool hasRelease: (status.latest || "") !== ""
    spacing: 10

    function stateText() {
        var v = status.latest || "";
        switch (state_) {
        case "checking": return I18n.t("Checking for updates…");
        case "upToDate": return I18n.t("Lunaris is up to date.");
        case "downloading": return I18n.t("Downloading Lunaris %1… %2%").arg(v).arg(Math.round((status.progress || 0) * 100));
        case "verifying": return I18n.t("Checking the signature…");
        case "installing": return I18n.t("Installing Lunaris %1…").arg(v);
        case "restart": return I18n.t("Restarting Lunaris…");
        case "available": return I18n.t("Lunaris %1 is available").arg(v);
        case "error": return I18n.tError(status.error || "");
        }
        return "";
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 16
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Text {
                objectName: "updatesVersion"
                text: "Lunaris " + (panel.status.current || bridge.appVersion())
                color: Theme.text
                font.pixelSize: 13
                font.weight: Font.Medium
            }
            Text {
                objectName: "updatesState"
                Layout.fillWidth: true
                visible: text !== ""
                textFormat: Text.PlainText
                text: panel.stateText()
                color: panel.state_ === "error" ? Theme.danger : (panel.state_ === "available" ? Theme.accentFg : Theme.textSubtle)
                font.pixelSize: 12
                wrapMode: Text.WordWrap
            }
        }
        SecondaryButton {
            objectName: "updatesCheckNow"
            text: I18n.t("Check now")
            enabled: !panel.busy
            onClicked: bridge.checkForUpdates()
        }
    }

    CheckRow {
        objectName: "updatesAutoCheck"
        Layout.fillWidth: true
        text: I18n.t("Check for updates automatically")
        checked: panel.status.autoCheck !== false
        onToggled: bridge.setUpdateAutoCheck(checked)
    }

    ProgressBar {
        objectName: "updatesProgress"
        Layout.fillWidth: true
        visible: panel.state_ === "downloading"
        from: 0
        to: 1
        value: panel.status.progress || 0
    }

    Text {
        objectName: "updatesManual"
        Layout.fillWidth: true
        visible: panel.hasRelease && panel.status.canInstall !== true && (panel.status.manualMessage || "") !== ""
        textFormat: Text.PlainText
        text: I18n.t(panel.status.manualMessage || "")
        color: Theme.textMuted
        font.pixelSize: 12
        wrapMode: Text.WordWrap
    }

    RowLayout {
        visible: panel.hasRelease && !panel.busy
        spacing: 8
        PrimaryButton {
            objectName: "updatesInstall"
            visible: panel.status.canInstall === true
            text: panel.state_ === "error" ? I18n.t("Try again") : I18n.t("Update to %1").arg(panel.status.latest || "")
            onClicked: bridge.updateInstall()
        }
        SecondaryButton {
            objectName: "updatesReleasePage"
            text: panel.status.canInstall === true ? I18n.t("Release page") : I18n.t("Download")
            onClicked: Qt.openUrlExternally(panel.status.releaseUrl)
        }
    }

    Text {
        objectName: "updatesNotes"
        Layout.fillWidth: true
        visible: panel.hasRelease && (panel.status.notes || "") !== ""
        textFormat: Text.PlainText
        text: panel.status.notes || ""
        color: Theme.textMuted
        font.pixelSize: 12
        wrapMode: Text.WordWrap
        maximumLineCount: 12
        elide: Text.ElideRight
    }
}
