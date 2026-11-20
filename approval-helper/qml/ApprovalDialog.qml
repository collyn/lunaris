import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Someone connected without the password: let them in? (the agent running
// as a daemon asks over its channel, see approval-helper/src/pending.rs and
// agent approval.rs). The same question, wording and behaviour as the
// Lunaris app's (client-qml/qml/main.qml): the window comes up,
// Accept/Decline, declined by the agent when nobody answers in time.
Popup {
    id: dialog
    // The ApprovalBridge, the window to bring up and the tray icon (its
    // notification when the window cannot be raised over other windows).
    required property var agentBridge
    required property var hostWindow
    property var tray: null

    property var question: null
    property int secondsLeft: 30

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(440, (parent ? parent.width : 440) - 32)
    modal: true
    focus: true
    closePolicy: Popup.NoAutoClose
    padding: 20

    // The texts of client-qml/qml/I18n.qml; Vietnamese when the system is.
    readonly property bool vi: Qt.locale().name.indexOf("vi") === 0
    readonly property var viText: ({
        "Someone wants to transfer files": "Có người muốn truyền file",
        "Someone wants to control this computer": "Có người muốn điều khiển máy này",
        "They will be able to see, download and upload files on this computer.": "Họ sẽ xem, tải xuống và tải lên file trên máy này.",
        "They will see your screen and can use the keyboard and mouse. Only accept people you know.": "Họ sẽ thấy màn hình và dùng được bàn phím, chuột của bạn. Chỉ chấp nhận người bạn biết.",
        "Declined automatically in %1 s": "Tự động từ chối sau %1 giây",
        "Decline": "Từ chối",
        "Accept": "Chấp nhận"
    })
    function t(text) {
        if (vi && viText[text] !== undefined) return viText[text];
        return text;
    }

    readonly property bool files: question !== null && question.kind === "files"

    function answer(accept) {
        if (question) agentBridge.answerApproval(question.id, accept);
        question = null;
        close();
    }

    // The questions waiting (oldest first): show the first one.
    Timer {
        interval: 600
        running: true
        repeat: true
        onTriggered: {
            var list = [];
            try { list = JSON.parse(dialog.agentBridge.approvals()); } catch (e) { list = []; }
            if (list.length === 0) {
                if (dialog.opened) dialog.close();
                dialog.question = null;
                return;
            }
            var q = list[0];
            if (!dialog.question || dialog.question.id !== q.id) {
                dialog.question = q;
                dialog.secondsLeft = q.timeout_s || 30;
                var w = dialog.hostWindow;
                if (!w.visible) w.show();
                if (w.visibility === Window.Minimized) w.showNormal();
                w.raise();
                w.requestActivate();
                w.alert(0);
                if (dialog.tray && dialog.tray.supportsMessages)
                    dialog.tray.showMessage(dialog.files ? dialog.t("Someone wants to transfer files")
                                                         : dialog.t("Someone wants to control this computer"),
                                            q.requester, 1, (q.timeout_s || 30) * 1000);
                dialog.open();
            }
        }
    }

    Timer {
        interval: 1000
        repeat: true
        running: dialog.opened
        onTriggered: if (dialog.secondsLeft > 0) dialog.secondsLeft--
    }

    background: Rectangle {
        color: Theme.surface
        radius: Theme.radiusLg
        border.width: 1
        border.color: Theme.borderStrong
    }

    contentItem: ColumnLayout {
        spacing: 12
        RowLayout {
            spacing: 12
            Rectangle {
                Layout.preferredWidth: 40; Layout.preferredHeight: 40; radius: 10
                color: Theme.accentA(0.15)
                Image { anchors.centerIn: parent; source: "qrc:/icons/icon.png"; width: 22; height: 22; fillMode: Image.PreserveAspectFit; smooth: true }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                Text {
                    Layout.fillWidth: true
                    text: dialog.files ? dialog.t("Someone wants to transfer files")
                                       : dialog.t("Someone wants to control this computer")
                    color: Theme.textStrong
                    font.pixelSize: 16
                    font.weight: Font.DemiBold
                    wrapMode: Text.WordWrap
                }
                Text {
                    Layout.fillWidth: true
                    // (Who asks is what the other side says of itself.)
                    textFormat: Text.PlainText
                    text: dialog.question ? dialog.question.requester : ""
                    color: Theme.text
                    font.pixelSize: 13
                    maximumLineCount: 1
                    elide: Text.ElideRight
                }
            }
        }
        Text {
            Layout.fillWidth: true
            text: dialog.files
                ? dialog.t("They will be able to see, download and upload files on this computer.")
                : dialog.t("They will see your screen and can use the keyboard and mouse. Only accept people you know.")
            color: Theme.textMuted
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Text {
                Layout.fillWidth: true
                text: dialog.t("Declined automatically in %1 s").arg(dialog.secondsLeft)
                color: Theme.textMuted
                font.pixelSize: 11
                wrapMode: Text.WordWrap
            }
            Rectangle {
                implicitHeight: 34
                implicitWidth: declineLabel.implicitWidth + 28
                radius: Theme.radius
                color: declineMa.containsMouse ? Theme.surface2 : Theme.surface
                border.width: 1
                border.color: Theme.borderStrong
                Text { id: declineLabel; anchors.centerIn: parent; text: dialog.t("Decline"); color: Theme.textStrong; font.pixelSize: 13; font.weight: Font.DemiBold }
                MouseArea { id: declineMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: dialog.answer(false) }
            }
            Rectangle {
                implicitHeight: 34
                implicitWidth: acceptLabel.implicitWidth + 28
                radius: Theme.radius
                color: acceptMa.containsMouse ? Theme.accentHover : Theme.accent
                Text { id: acceptLabel; anchors.centerIn: parent; text: dialog.t("Accept"); color: Theme.textOnAccent; font.pixelSize: 13; font.weight: Font.DemiBold }
                MouseArea { id: acceptMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: dialog.answer(true) }
            }
        }
    }
}
