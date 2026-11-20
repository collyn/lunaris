import QtQuick
import QtTest
import "../../qml"

// main.qml itself, off screen, with the stand-ins of stubs/: the gear of
// the session menu opens this device's settings for the stream
// (SessionSettings.qml); each one applies right away through the bridge and
// is kept (store "view_prefs"), the decoder restarts the stream with it.
// Pictures in English and Vietnamese to out/.
Item {
    id: root
    width: 200
    height: 100

    readonly property string outDir: String(Qt.resolvedUrl("out")).replace(/^file:\/\//, "")

    TestCase {
        name: "SessionSettings"
        when: windowShown

        property var app: null
        property var bridge: null
        property var panel: null

        function init() {
            ViewPrefs.fromJson(JSON.stringify({ displayMode: "fit", audioEnabled: true, volume: 100, recordMicrophone: false,
                                                clipboardSync: true, fileTransfer: true, gamepad: true, systemKeys: "always" }));
            var component = Qt.createComponent("../../qml/main.qml");
            compare(component.status, Component.Ready, component.errorString());
            app = component.createObject(null);
            verify(app !== null);
            bridge = findChild(app, "bridge");
            verify(bridge !== null);
            bridge.deeplinkReceived("lunaris://connect");
            verify(app.isStreamMode);
            panel = findChild(app, "sessionSettings");
            verify(panel !== null);
        }

        function cleanup() {
            if (app) app.destroy();
            app = null;
            I18n.lang = "en";
        }

        function child(name) { var c = findChild(app, name); verify(c !== null, name); return c; }
        function flip(toggle) { toggle.checked = !toggle.checked; toggle.toggled(); }
        function saved() { return JSON.parse(bridge.stores.view_prefs); }

        function openPanel() {
            child("sessionSettingsButton").clicked();
            tryVerify(function () { return panel.opened; });
        }

        function test_the_gear_opens_and_closes_it() {
            verify(!panel.opened);
            openPanel();
            child("sessionSettingsButton").clicked();
            tryVerify(function () { return !panel.opened; });
        }

        function test_sound_applies_and_is_kept() {
            openPanel();
            flip(child("audioToggle"));
            compare(ViewPrefs.audioEnabled, false);
            compare(bridge.audio.on, false);
            compare(saved().audioEnabled, false);

            ViewPrefs.volume = 40;
            compare(bridge.audio.volume, 40);
            compare(saved().volume, 40);

            flip(child("microphoneToggle"));
            compare(bridge.microphone, true);
            compare(saved().recordMicrophone, true);
        }

        function test_clipboard_files_and_controllers() {
            openPanel();
            flip(child("clipboardToggle"));
            compare(bridge.share.clipboard, false);
            flip(child("fileTransferToggle"));
            compare(bridge.share.files, false);
            compare(saved().fileTransfer, false);
            flip(child("gamepadToggle"));
            compare(bridge.gamepad, false);
            compare(saved().gamepad, false);

            // The controller test is shown on demand.
            compare(findChild(app, "gamepadTester"), null);
            child("testControllersButton").clicked();
            verify(panel.testerShown);
            tryVerify(function () { return findChild(app, "gamepadTester") !== null; });
            panel.close();
            verify(!panel.testerShown, "hidden again when the panel closes");
        }

        function test_keys_picture_statistics_decoder() {
            openPanel();
            child("systemKeysCombo").picked("fullscreen");
            compare(ViewPrefs.systemKeys, "fullscreen");
            compare(saved().systemKeys, "fullscreen");

            child("displayModeCombo").picked("stretch");
            compare(ViewPrefs.displayMode, "stretch");
            compare(saved().displayMode, "stretch");

            verify(!app.showStats);
            flip(child("statsToggle"));
            verify(app.showStats);
            verify(child("statsOverlay").visible);

            // The decoder is the stream's: it starts again with it.
            var asked = bridge.configs.length;
            child("decoderCombo").picked("software");
            compare(bridge.configs.length, asked + 1);
            compare(bridge.configs[bridge.configs.length - 1].renderBackend, "software");
            compare(app.renderBackend, "software");
            compare(child("decoderCombo").value, "software");
        }

        function test_no_system_shortcuts_where_none_can_be_taken() {
            app.isStreamMode = false;
            bridge.systemKeys = "window";
            app.isStreamMode = true;
            compare(app.systemKeysKind, "window");
            openPanel();
            verify(!child("systemKeysCombo").visible);
            bridge.systemKeys = "grab";
        }

        function test_pictures_data() {
            return [{ tag: "en" }, { tag: "vi" }];
        }

        function test_pictures(data) {
            I18n.lang = data.tag;
            app.width = 1280;
            app.height = 800;
            openPanel();
            wait(300);
            grabImage(app.contentItem.parent || app.contentItem).save(root.outDir + "/session-settings-" + data.tag + ".png");
        }
    }
}
