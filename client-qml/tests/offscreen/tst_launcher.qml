import QtQuick
import QtTest
import "../../qml"

// main.qml itself, off screen, with the stand-ins of stubs/: started
// without a link it shows the window that points to the server's web page
// (Launcher.qml): the trusted servers (open, remove, add), language and
// theme; a link that arrives starts the stream, and a stream that ends goes
// back to it with the reason. Pictures in English and Vietnamese to out/.
Item {
    id: root
    width: 200
    height: 100

    readonly property string outDir: String(Qt.resolvedUrl("out")).replace(/^file:\/\//, "")

    TestCase {
        name: "Launcher"
        when: windowShown

        property var app: null
        property var bridge: null
        property var launcher: null
        property var opened: []

        function open(language, trusted) {
            if (app) app.destroy();
            var component = Qt.createComponent("../../qml/main.qml");
            compare(component.status, Component.Ready, component.errorString());
            app = component.createObject(null);
            verify(app !== null);
            bridge = findChild(app, "bridge");
            verify(bridge !== null);
            bridge.trusted = trusted;
            bridge.stores = { app_settings: JSON.stringify({ theme: "dark", language: language, closeToTray: true }) };
            launcher = findChild(app, "launcher");
            verify(launcher !== null);
            launcher.loadPrefs();
            launcher.refresh();
            opened = [];
            launcher.openUrl = function (url) { opened = opened.concat([url]); };
            wait(50);
        }

        function cleanup() {
            if (app) app.destroy();
            app = null;
            I18n.lang = "en";
            Theme.mode = "system";
        }

        function shot(language, what) {
            wait(250);
            grabImage(app.contentItem).save(root.outDir + "/launcher-" + language + "-" + what + ".png");
        }

        function rows() {
            var list = [];
            var all = findChild(launcher, "serverList").parent.children;
            for (var i = 0; i < all.length; i++) if (all[i].objectName === "serverRow") list.push(all[i]);
            return list;
        }

        function test_shown_without_a_link_data() {
            return [{ tag: "en" }, { tag: "vi" }];
        }

        function test_shown_without_a_link(data) {
            open(data.tag, [{ server: "https://lunaris.example.com:443", web: "https://lunaris.example.com" },
                            { server: "http://office.example:8080", web: "http://office.example:8080" }]);
            verify(!app.isStreamMode);
            verify(launcher.visible);
            compare(I18n.lang, data.tag);
            compare(Theme.mode, "dark");
            compare(rows().length, 2);
            verify(!findChild(launcher, "noServers").visible);
            shot(data.tag, "servers");

            // Open one in the browser.
            findChild(rows()[1], "openServer").clicked();
            compare(opened, ["http://office.example:8080"]);

            // Take one off the list.
            findChild(rows()[0], "removeServer").clicked();
            compare(bridge.trusted.length, 1);
            compare(rows().length, 1);

            // Add one: it is listed and its page opens.
            var field = findChild(launcher, "addServerField");
            field.text = "lunaris.home.example";
            findChild(launcher, "addServerButton").clicked();
            compare(opened[opened.length - 1], "https://lunaris.home.example");
            compare(rows().length, 2);
            compare(field.text, "");

            // What is not an address is told.
            field.text = "not an address";
            findChild(launcher, "addServerButton").clicked();
            compare(launcher.addError, I18n.t("That is not the address of a web page."));
            compare(rows().length, 2);
        }

        function test_no_server_yet() {
            open("en", []);
            verify(findChild(launcher, "noServers").visible);
            compare(rows().length, 0);
            shot("en", "empty");
        }

        function test_language_and_theme_are_kept() {
            open("en", []);
            launcher.setPref("language", "vi");
            launcher.setPref("theme", "light");
            compare(I18n.lang, "vi");
            compare(Theme.mode, "light");
            var saved = JSON.parse(bridge.stores.app_settings);
            compare(saved.language, "vi");
            compare(saved.theme, "light");
            // What earlier versions kept there stays.
            compare(saved.closeToTray, true);
        }

        function test_a_link_starts_the_stream_and_its_end_comes_back() {
            open("en", []);
            bridge.deeplinkReceived("lunaris://connect");
            verify(app.isStreamMode);
            compare(bridge.started, 1);
            verify(!launcher.visible);

            // The host: the computer it shows could not be connected.
            bridge.sourceStatus("failed", "Wrong password for the RustDesk computer");
            verify(!app.isStreamMode, "back on the window without a link");
            compare(bridge.stopped, 1);
            compare(launcher.message, I18n.t("Could not connect to the other computer: %1").arg(I18n.t("Wrong password for the RustDesk computer")));
            verify(findChild(launcher, "launcherMessage").visible);
            shot("en", "stream-failed");

            // The next link clears it; the stream's own error is told too.
            bridge.deeplinkReceived("lunaris://connect");
            compare(launcher.message, "");
            bridge.streamFailed("Connection failed: timed out");
            verify(!app.isStreamMode);
            compare(launcher.message, "Connection failed: timed out");

            // A link whose ticket was refused, with no stream running.
            bridge.linkFailed("This link has expired or was already used. Open the computer again from the web page.");
            verify(launcher.message !== "");

            // Leaving a stream from the menu comes back here as well.
            bridge.deeplinkReceived("lunaris://connect");
            verify(app.isStreamMode);
            var stops = bridge.stopped;
            findChild(app, "menuBar").exitTriggered();
            verify(!app.isStreamMode);
            compare(bridge.stopped, stops + 1);
            verify(launcher.visible);
        }
    }
}
