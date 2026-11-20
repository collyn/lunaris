import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The slim notice above the window shown without a link: a newer Lunaris release, the progress of its
// download and install, or why the update failed. `status` is the parsed
// bridge.updateStatus() (updater/src/service.rs); Update, Download and the
// restart go through the bridge.
Rectangle {
    id: banner
    property var status: ({})
    property bool dismissed: false
    property bool expanded: false

    readonly property string state_: status.state || "idle"
    readonly property bool busy: ["downloading", "verifying", "installing", "restart"].indexOf(state_) >= 0
    readonly property bool failed: state_ === "error" && status.errorKind !== "check"
    readonly property bool shown: (status.latest || "") !== "" && !dismissed
                                  && (state_ === "available" || busy || failed)

    visible: shown
    implicitHeight: col.implicitHeight + 14
    color: failed ? Qt.rgba(Theme.danger.r, Theme.danger.g, Theme.danger.b, 0.12) : Theme.accentA(0.12)
    border.color: failed ? Theme.danger : Theme.accentA(0.35)
    border.width: 1

    function message() {
        var v = status.latest || "";
        switch (state_) {
        case "downloading": return I18n.t("Downloading Lunaris %1… %2%").arg(v).arg(Math.round((status.progress || 0) * 100));
        case "verifying": return I18n.t("Checking the signature…");
        case "installing": return I18n.t("Installing Lunaris %1…").arg(v);
        case "restart": return I18n.t("Restarting Lunaris…");
        case "error": return I18n.tError(status.error || "");
        }
        return I18n.t("Lunaris %1 is available").arg(v);
    }

    ColumnLayout {
        id: col
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: 16
        anchors.rightMargin: 10
        spacing: 6

        RowLayout {
            Layout.fillWidth: true
            spacing: 10
            Text {
                objectName: "updateBannerText"
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: banner.message()
                color: banner.failed ? Theme.danger : Theme.textStrong
                font.pixelSize: 13
                font.weight: Font.DemiBold
                wrapMode: Text.WordWrap
            }
            SecondaryButton {
                objectName: "updateBannerNotes"
                visible: !banner.busy && (banner.status.notes || "") !== ""
                text: banner.expanded ? I18n.t("Hide notes") : I18n.t("What's new")
                implicitHeight: 28
                onClicked: banner.expanded = !banner.expanded
            }
            PrimaryButton {
                objectName: "updateBannerInstall"
                visible: !banner.busy && banner.status.canInstall === true
                text: banner.failed ? I18n.t("Try again") : I18n.t("Update")
                implicitHeight: 28
                onClicked: bridge.updateInstall()
            }
            SecondaryButton {
                objectName: "updateBannerDownload"
                visible: !banner.busy && banner.status.canInstall !== true
                text: I18n.t("Download")
                implicitHeight: 28
                onClicked: Qt.openUrlExternally(banner.status.releaseUrl)
            }
            Text {
                visible: !banner.busy
                text: "✕"
                color: Theme.textMuted
                font.pixelSize: 14
                Accessible.name: I18n.t("Close")
                MouseArea {
                    anchors.fill: parent
                    anchors.margins: -6
                    cursorShape: Qt.PointingHandCursor
                    onClicked: banner.dismissed = true
                }
            }
        }
        Text {
            objectName: "updateBannerManual"
            Layout.fillWidth: true
            visible: banner.state_ === "available" && banner.status.canInstall !== true && (banner.status.manualMessage || "") !== ""
            textFormat: Text.PlainText
            text: I18n.t(banner.status.manualMessage || "")
            color: Theme.textMuted
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }
        ProgressBar {
            objectName: "updateBannerProgress"
            Layout.fillWidth: true
            visible: banner.state_ === "downloading"
            from: 0
            to: 1
            value: banner.status.progress || 0
        }
        Text {
            objectName: "updateBannerNotesText"
            Layout.fillWidth: true
            visible: banner.expanded && !banner.busy
            textFormat: Text.PlainText
            text: banner.status.notes || ""
            color: Theme.text
            font.pixelSize: 12
            wrapMode: Text.WordWrap
            maximumLineCount: 10
            elide: Text.ElideRight
        }
    }
}
