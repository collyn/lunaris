import QtQuick
import QtTest
import "../../qml"

// main.qml off screen: the question a lunaris:// link asks before it
// connects. A link to a server reached over http says it is not
// encrypted (and warns before "Always trust"); a trusted server's link for
// another account names both accounts. Pictures to out/.
Item {
    id: root
    width: 200
    height: 100

    readonly property string outDir: String(Qt.resolvedUrl("out")).replace(/^file:\/\//, "")

    TestCase {
        name: "LinkDialog"
        when: windowShown

        property var app: null
        property var bridge: null
        property var dialog: null

        function open(info) {
            if (app) app.destroy();
            var component = Qt.createComponent("../../qml/main.qml");
            compare(component.status, Component.Ready, component.errorString());
            app = component.createObject(null);
            verify(app !== null);
            bridge = findChild(app, "bridge");
            dialog = findChild(app, "linkDialog");
            verify(dialog !== null);
            bridge.pendingLink = JSON.stringify(info);
            verify(dialog.openIfPending());
            tryCompare(dialog, "opened", true);
        }

        function cleanup() {
            if (app) app.destroy();
            app = null;
            I18n.lang = "en";
        }

        function shot(what) {
            wait(250);
            grabImage(app.contentItem).save(root.outDir + "/link-" + what + ".png");
        }

        function test_encrypted_link() {
            open({ server: "srv.example", computer: "PC", encrypted: true, account: "", knownAccount: "" });
            verify(!findChild(dialog.contentItem, "linkNotEncrypted").visible);
            findChild(dialog.contentItem, "linkTrustBox").checked = true;
            verify(!findChild(dialog.contentItem, "linkTrustNotEncrypted").visible);
            compare(dialog.otherAccount, false);
        }

        function test_unencrypted_link_says_so() {
            open({ server: "10.0.0.2:8080", computer: "PC", encrypted: false, account: "", knownAccount: "" });
            var note = findChild(dialog.contentItem, "linkNotEncrypted");
            verify(note.visible);
            verify(note.text.indexOf("Not encrypted") === 0);
            var warn = findChild(dialog.contentItem, "linkTrustNotEncrypted");
            verify(!warn.visible);
            findChild(dialog.contentItem, "linkTrustBox").checked = true;
            verify(warn.visible);
            shot("not-encrypted-en");
            I18n.lang = "vi";
            verify(note.text.indexOf("Không mã hoá") === 0, note.text);
            verify(warn.text.indexOf("Máy chủ này không mã hoá") === 0, warn.text);
            shot("not-encrypted-vi");
            dialog.answer(true);
            compare(bridge.linkAnswers, [[true, true]]);
        }

        function test_other_account_names_both() {
            open({ server: "srv.example", computer: "Office PC", encrypted: true, account: "mallory", knownAccount: "alice" });
            verify(dialog.otherAccount);
            var box = findChild(dialog.contentItem, "linkTrustBox");
            compare(box.text, "Always trust this server for the account mallory");
            shot("other-account-en");
            I18n.lang = "vi";
            compare(box.text, "Luôn tin cậy máy chủ này cho tài khoản mallory");
            shot("other-account-vi");
            // Connect without trusting: the bridge starts it untrusted.
            dialog.answer(true);
            compare(bridge.linkAnswers, [[true, false]]);
        }

        function test_older_bridge_info_still_opens() {
            // Only server and computer (as before): shown like an encrypted one.
            open({ server: "srv.example", computer: "PC" });
            verify(!findChild(dialog.contentItem, "linkNotEncrypted").visible);
            dialog.answer(false);
            compare(bridge.linkAnswers, [[false, false]]);
        }
    }
}
