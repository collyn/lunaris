import QtQuick
import QtQuick.Controls
import QtTest
import "../../qml"
import "../../qml/Cards.js" as Cards

// The "Add computer" flow and the cards of computers of other types,
// rendered off screen with a stub bridge (StubBridge.qml): checks what the
// pages do and saves a picture of each state, in English and Vietnamese,
// to out/. See run.sh. No window is opened and nothing is connected to.
Item {
    id: window
    width: 1180
    height: 760

    // What the pages expect of main.qml's window.
    property bool closeToTray: true
    property bool autostartEnabled: false
    property bool showStats: false
    function startSystemMove() {}

    StubBridge { id: bridge }
    Item { id: fileManager; property var opened: []; property var calls: []; function openFor(server, token, hostId, name, sourceId) { opened = opened.concat([hostId]); calls = calls.concat([[server, token, hostId, name, sourceId || ""]]); } }

    readonly property string outDir: String(Qt.resolvedUrl("out")).replace(/^file:\/\//, "")

    Loader { id: homeLoader; anchors.fill: parent; active: false; sourceComponent: Home {} }

    SignalSpy { id: sessions; signalName: "startSessionRequested" }

    TestCase {
        id: test
        name: "Computers"
        when: windowShown

        property var home: null
        property string lang: "en"

        // `prepare(bridge)` sets the stub's data before the pages load (and
        // may replace the stores); `theme` is "dark" unless said.
        function start(language, prepare, theme) {
            homeLoader.active = false;
            test.lang = language;
            // (A singleton: back to how a first start has it.)
            ViewPrefs.computerView = "grid";
            ViewPrefs.computerType = "";
            bridge.startOver();
            bridge.stores = {
                app_settings: JSON.stringify({ theme: theme || "dark", language: language }),
                peers: JSON.stringify({ peers: [
                    { address: "192.168.1.20:8091", name: "Office PC", password: "saved", favorite: true, os: "windows", lastUsed: 30 },
                    { address: "source:den-pc-1a2b", name: "Den PC", type: "rustdesk", sourceId: "den-pc-1a2b", favorite: false, lastUsed: 20 },
                    { address: "10.0.0.7:8091", name: "", password: "", favorite: false, os: "", lastUsed: 10 }
                ] })
            };
            if (prepare) prepare(bridge);
            homeLoader.active = true;
            test.home = homeLoader.item;
            sessions.target = test.home;
            sessions.clear();
            wait(50);
        }

        function shot(what) {
            // Popups fade in; the picture is of the whole window, overlay included.
            wait(350);
            var image = grabImage(window.Window.window.contentItem);
            var path = window.outDir + "/desktop-" + test.lang + "-" + what + ".png";
            image.save(path);
        }

        function cardOf(address) {
            var grid = findChild(test.home, "peerGrid");
            var items = grid.contentItem.children;
            for (var i = 0; i < items.length; i++) {
                var card = findChild(items[i], "peerCard");
                if (card && card.peer.address === address) return card;
            }
            return null;
        }

        // The card of the list with this key (Cards.js: "host:<address>",
        // "group:<group>", "source::<id>").
        function cardByKey(key) {
            var grid = findChild(test.home, "peerGrid");
            var items = grid.contentItem.children;
            for (var i = 0; i < items.length; i++) {
                var card = findChild(items[i], "peerCard");
                if (card && card.cardData.key === key) return card;
            }
            return null;
        }

        // Every item under `item` with this object name.
        function all(item, name, found) {
            found = found || [];
            if (item.objectName === name) found.push(item);
            var children = item.children || [];
            for (var i = 0; i < children.length; i++) all(children[i], name, found);
            return found;
        }

        function labels(card) { return card.methods.map(function (m) { return m.label; }); }

        function settings() { return JSON.parse(bridge.stores.app_settings); }

        function peer(address) {
            var peers = JSON.parse(bridge.stores.peers).peers;
            for (var i = 0; i < peers.length; i++) if (peers[i].address === address) return peers[i];
            return null;
        }

        function field(dialog, key) { return findChild(dialog.contentItem, "field-" + key); }

        function closeAll(dialog) {
            if (dialog) dialog.close();
            wait(250);
        }

        function test_screens_data() {
            return [{ tag: "en" }, { tag: "vi" }];
        }

        function test_screens(data) {
            start(data.tag);
            var home = test.home;

            // ---- Home: a Lunaris card and a card of another type ----
            var sourceCard = cardOf("source:den-pc-1a2b");
            verify(sourceCard !== null, "the RustDesk-type computer has a card");
            verify(sourceCard.isSource);
            verify(!cardOf("192.168.1.20:8091").isSource);
            // Only Lunaris computers have a port to check.
            wait(100);
            verify(bridge.probed.indexOf("192.168.1.20:8091") !== -1);
            verify(bridge.probed.indexOf("source:den-pc-1a2b") === -1, "a source card is not probed");
            shot("home-list");

            // One click connects, through the app's helper ("bridge:"), with no password.
            mouseClick(sourceCard, sourceCard.width / 2, 40);
            compare(sessions.count, 1);
            compare(sessions.signalArguments[0][0], "bridge:");
            compare(sessions.signalArguments[0][1], "");
            compare(sessions.signalArguments[0][2], "direct");
            compare(sessions.signalArguments[0][3], "Den PC");
            compare(sessions.signalArguments[0][15], "den-pc-1a2b");
            sessions.clear();
            home.setStatus("", false);
            // Connecting moved the card to the front: the list was made anew.
            wait(50);
            sourceCard = cardOf("source:den-pc-1a2b");

            // Its files: the same file manager, through the app's helper,
            // naming the computer as the source; no password is asked for.
            var filesBefore = fileManager.calls.length;
            findChild(sourceCard, "peerFilesButton").clicked();
            compare(fileManager.calls.length, filesBefore + 1);
            compare(fileManager.calls[filesBefore], ["bridge:", "", "direct", "Den PC", "den-pc-1a2b"]);
            compare(sessions.count, 0, "opening the files does not start a session");

            // Its menu: Files, but no Rename and no Forget password; Edit instead.
            var more = findChild(sourceCard, "peerMoreButton");
            more.clicked();
            shot("home-card-menu");
            var menu = findChild(sourceCard, "peerMenu");
            menu.close();
            wait(250);

            // ---- Add computer ----
            var dialog = findChild(home, "addComputerDialog");
            findChild(home, "addComputerButton").clicked();
            verify(dialog.opened);
            compare(dialog.typeOptions.length, 3);
            compare(dialog.typeId, "lunaris");
            findChild(dialog.contentItem, "addressField").text = "192.168.1.31";
            findChild(dialog.contentItem, "lunarisNameField").text = "Meeting room";
            shot("add-lunaris");
            findChild(dialog.contentItem, "submitButton").clicked();
            verify(!dialog.visible || !dialog.opened);
            wait(250);
            var added = peer("192.168.1.31:8091");
            verify(added !== null, "the Lunaris computer is saved");
            compare(added.name, "Meeting room");
            compare(sessions.count, 0, "adding does not connect");

            // The types are there to click: no list to open. Lunaris says
            // what it is; a click chooses.
            dialog.openAdd();
            verify(findChild(dialog.contentItem, "typeCombo") === null, "no drop-down");
            var lunarisOption = findChild(dialog.contentItem, "typeOption-lunaris");
            var rustdeskOption = findChild(dialog.contentItem, "typeOption-rustdesk");
            verify(lunarisOption.visible && rustdeskOption.visible && findChild(dialog.contentItem, "typeOption-acme-view").visible);
            verify(lunarisOption.chosen && !rustdeskOption.chosen);
            // A tile has the short name; the whole one is what a screen reader says.
            compare(lunarisOption.text, "Lunaris");
            compare(rustdeskOption.text, "RustDesk");
            compare(rustdeskOption.fullName, I18n.t("RustDesk (by IP address)"));
            // What Lunaris is stands once, under the row (no paragraph above the form as well).
            compare(findChild(dialog.contentItem, "typeFullName").text, "Lunaris");
            compare(findChild(dialog.contentItem, "typeNote").text,
                I18n.t("A computer with the Lunaris app (remote control turned on). Its address and password are shown under \"This computer\" there."));
            shot("add-type-choice");

            // A connector type: the form is the connector's fields
            mouseClick(rustdeskOption);
            compare(dialog.typeId, "rustdesk");
            verify(rustdeskOption.chosen && !lunarisOption.chosen);
            wait(50);
            compare(field(dialog, "port").text, "21118", "the default is filled in");
            verify(field(dialog, "password").password, "a secret is not readable");
            shot("add-rustdesk-empty");

            // Required fields and the port are checked before anything is saved
            findChild(dialog.contentItem, "nameField").text = "Workshop";
            findChild(dialog.contentItem, "submitButton").clicked();
            verify(dialog.error !== "");
            verify(dialog.opened);
            field(dialog, "host").text = "192.168.1.60";
            field(dialog, "port").text = "70000";
            field(dialog, "password").text = "not-shown";
            findChild(dialog.contentItem, "submitButton").clicked();
            verify(dialog.error !== "");
            shot("add-validation-error");

            field(dialog, "port").text = "21118";
            dialog.error = "";
            shot("add-rustdesk");
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(250);
            verify(!dialog.opened);
            var workshop = peer("source:workshop-0001");
            verify(workshop !== null, "the computer has a card");
            compare(workshop.type, "rustdesk");
            compare(workshop.sourceId, "workshop-0001");
            verify(bridge.stores.peers.indexOf("not-shown") === -1, "the password is not in the list of computers");

            // Another connector, other fields: nothing in the dialog is RustDesk's
            dialog.openAdd();
            dialog.chooseType("acme-view");
            wait(50);
            compare(field(dialog, "gateway").text, "gw.example.net");
            compare(field(dialog, "gateway_port").text, "7400");
            verify(field(dialog, "token").password);
            verify(field(dialog, "host") === null);
            findChild(dialog.contentItem, "nameField").text = "Press room";
            field(dialog, "station").text = "press-04";
            field(dialog, "token").text = "abcdef";
            shot("add-other-type");
            closeAll(dialog);

            // ---- Edit: values that may be shown are there, secrets are not ----
            home.editSource(peer("source:den-pc-1a2b"));
            wait(50);
            verify(dialog.editing);
            compare(findChild(dialog.contentItem, "nameField").text, "Den PC");
            compare(field(dialog, "host").text, "192.168.1.50");
            compare(field(dialog, "password").text, "");
            shot("edit");
            findChild(dialog.contentItem, "nameField").text = "Den";
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(250);
            verify(!dialog.opened, "an empty password keeps the stored one");
            compare(peer("source:den-pc-1a2b").name, "Den");
            home.updatePeer("source:den-pc-1a2b", { name: "Den PC" });

            // ---- Stream settings of a source card: its own displays ----
            home.sessionPeer = "source:den-pc-1a2b";
            home.sessionSource = "den-pc-1a2b";
            home.sourceDisplaysSeen(JSON.stringify([{ id: "0", name: "Display 1", width: 1920, height: 1080 }, { id: "1", name: "Display 2", width: 2560, height: 1440 }]), "0");
            bridge.capabilitiesAsked = [];
            home.openPeerSettings("source:den-pc-1a2b");
            var settings = findChild(home, "peerSettings");
            verify(settings.visible);
            verify(settings.sourceOnly);
            compare(bridge.capabilitiesAsked.length, 0, "nothing is asked of a source card");
            compare(settings.displayOptions.length, 3);
            settings.category = "host";
            settings.displayId = "1";
            settings.save();
            compare(peer("source:den-pc-1a2b").settings.displayId, "1");
            home.requestConnect("source:den-pc-1a2b");
            compare(sessions.signalArguments[0][14], "1", "the chosen display goes with the session");
            sessions.clear();

            // Remove takes the card and what the helper keeps
            home.removeSourcePeer(peer("source:workshop-0001"));
            verify(peer("source:workshop-0001") === null);
            compare(bridge.ownSources.length, 1);

            // ---- Server tab ----
            home.peerTab = "server";
            var dashboard = home.dashboard;
            tryCompare(dashboard, "isAdmin", true);
            compare(dashboard.cards.length, 4);
            // Each card says where its computer is
            var addressOf = function (key) {
                var grid = findChild(dashboard, "serverGrid");
                var items = grid.contentItem.children;
                for (var i = 0; i < items.length; i++)
                    if (items[i].cardData && items[i].cardData.key === key) {
                        var label = findChild(items[i], "cardAddress");
                        return label && label.parent.visible ? label.text : "";
                    }
                return null;
            };
            compare(addressOf("host:" + bridge.hosts[0].id), "192.168.1.20");
            compare(addressOf("host:" + bridge.hosts[1].id), "192.168.1.40, 100.64.0.40");
            compare(addressOf("source:" + bridge.hosts[0].id + ":lab-server-9f31"), "192.168.1.30");
            compare(addressOf("source:" + bridge.hosts[1].id + ":press-7c02"), "", "nothing when it is not known");
            dashboard.searchText = "100.64";
            compare(dashboard.shownCards.map(function (c) { return c.title; }), ["Lab PC"]);
            dashboard.searchText = "";
            // Its stream settings say it too
            dashboard.openStreamSettings(bridge.hosts[1]);
            compare(dashboard.settingsModal.hostAddress, "192.168.1.40, 100.64.0.40");
            verify(findChild(dashboard.settingsModal, "hostAddress").visible);
            dashboard.settingsModal.close();
            shot("server-list");

            // A source card connects as a session to the computer it is reached through
            dashboard.connectSource(dashboard.sourcesList[0]);
            compare(sessions.count, 1);
            compare(sessions.signalArguments[0][0], "http://192.168.1.5:8090");
            compare(sessions.signalArguments[0][2], bridge.hosts[0].id);
            compare(sessions.signalArguments[0][3], "Lab server");
            compare(sessions.signalArguments[0][15], "lab-server-9f31");
            sessions.clear();
            home.setStatus("", false);

            var serverDialog = findChild(dashboard, "serverAddDialog");
            serverDialog.openDialog();
            compare(serverDialog.typeOptions.length, 3);
            shot("server-add-lunaris");
            serverDialog.chooseType("rustdesk");
            wait(50);
            compare(serverDialog.throughHost, bridge.hosts[0].id);
            // The one Lunaris computer that can show it is there to see, chosen.
            var throughOption = findChild(serverDialog.contentItem, "throughOption-" + bridge.hosts[0].id);
            verify(throughOption.visible && throughOption.chosen);
            compare(throughOption.text, "Office PC");
            verify(!findChild(serverDialog.contentItem, "throughCombo").visible);
            findChild(serverDialog.contentItem, "nameField").text = "Den PC";
            field(serverDialog, "host").text = "192.168.1.50";
            field(serverDialog, "password").text = "not-shown";
            shot("server-add-connector");

            // The server's own words are shown in the dialog
            bridge.nextWrite = { status: 409, body: JSON.stringify({ error: "That computer asks at its screen before each connection, so computers cannot be added to it from here. Add it in the Lunaris app on that computer." }) };
            findChild(serverDialog.contentItem, "submitButton").clicked();
            verify(serverDialog.opened);
            verify(serverDialog.error !== "");
            shot("server-error");
            bridge.nextWrite = { status: 403, body: "Admin access required" };
            findChild(serverDialog.contentItem, "submitButton").clicked();
            compare(serverDialog.error, I18n.t("Admin access required"));
            shot("server-error-403");

            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            verify(!serverDialog.opened);
            var post = bridge.calls[0];
            compare(post.method, "POST");
            compare(post.path, "/api/hosts/" + bridge.hosts[0].id + "/sources");
            var sent = JSON.parse(post.body);
            verify(sent.group === undefined, "nothing is said of a group that the form did not say");
            compare(sent.name, "Den PC");
            compare(sent.connector, "rustdesk");
            compare(sent.values.host, "192.168.1.50");
            compare(sent.values.port, "21118");
            compare(dashboard.sourcesList.length, 3);

            // Edit on the server: empty fields, and only what was typed is sent
            dashboard.editSource(dashboard.sourcesList[0]);
            wait(50);
            compare(field(serverDialog, "host").text, "");
            field(serverDialog, "host").text = "192.168.1.77";
            shot("server-edit");
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(bridge.calls[0].method, "PUT");
            compare(bridge.calls[0].path, "/api/hosts/" + bridge.hosts[0].id + "/sources/lab-server-9f31");
            var changed = JSON.parse(bridge.calls[0].body);
            verify(changed.group === undefined, "an edit that leaves the computer where it is says nothing of its group");
            compare(changed.values.host, "192.168.1.77");
            verify(changed.values.password === undefined);
            verify(changed.values.port === undefined);

            // Edit on a server that sends the stored values (to administrators):
            // they are filled in and sent back as they are; the password stays empty
            // and, left so, is kept.
            dashboard.editSource(Object.assign({}, dashboard.sourcesList[0], { values: { host: "192.168.1.30", port: "21119" } }));
            wait(50);
            verify(serverDialog.editValuesKnown);
            compare(field(serverDialog, "host").text, "192.168.1.30");
            compare(field(serverDialog, "port").text, "21119");
            compare(field(serverDialog, "password").text, "");
            compare(field(serverDialog, "password").placeholder, I18n.t("Leave empty to keep the password"));
            compare(field(serverDialog, "host").placeholder, "", "a field that is shown has no \"keep it\" hint");
            shot("server-edit-values");
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(bridge.calls[0].method, "PUT");
            var kept = JSON.parse(bridge.calls[0].body);
            compare(kept.values.host, "192.168.1.30");
            compare(kept.values.port, "21119");
            verify(kept.values.password === undefined);
            // An optional field that had a value and is emptied is sent empty (cleared).
            dashboard.editSource(Object.assign({}, dashboard.sourcesList[0], { values: { host: "192.168.1.30", port: "21119" } }));
            wait(50);
            verify(findChild(serverDialog.contentItem, "passwordsNote").visible);
            field(serverDialog, "port").text = "";
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            var cleared = JSON.parse(bridge.calls[0].body);
            compare(cleared.values.port, "");
            compare(cleared.values.host, "192.168.1.30");
            verify(cleared.values.password === undefined, "an empty password is still left out");
            // A required field that is shown may not be emptied.
            dashboard.editSource(Object.assign({}, dashboard.sourcesList[0], { values: { host: "192.168.1.30" } }));
            wait(50);
            field(serverDialog, "host").text = "";
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(100);
            compare(bridge.calls.length, 0);
            verify(serverDialog.error !== "");
            closeAll(serverDialog);

            // Remove asks first
            var removeDialog = findChild(dashboard, "removeSourceDialog");
            removeDialog.openFor(dashboard.sourcesList[0]);
            shot("server-remove");
            bridge.calls = [];
            removeDialog.confirm();
            wait(250);
            compare(bridge.calls[0].method, "DELETE");
            compare(bridge.calls[0].path, "/api/hosts/" + bridge.hosts[0].id + "/sources/lab-server-9f31");
            verify(!removeDialog.opened);

            // ---- A server from before this: Lunaris only, no source cards, no error ----
            bridge.oldServer = true;
            bridge.fetchHosts();
            wait(50);
            compare(dashboard.sourcesList.length, 0);
            compare(dashboard.cards.length, 2);
            serverDialog.openDialog();
            compare(serverDialog.typeOptions.length, 1);
            compare(serverDialog.error, "");
            shot("server-old-add");
            closeAll(serverDialog);
            shot("server-old-list");
        }

        function test_setup_data() {
            return [{ tag: "en" }, { tag: "vi" }];
        }

        // A type whose connector has a set-up (a pairing): the dialog shows
        // the connector's message and its PIN, waits, and the card comes
        // when the set-up is done. The app's own computers, then a server's.
        function test_setup(data) {
            start(data.tag);
            var home = test.home;
            bridge.extra = [bridge.sunshine];
            home.refreshOwnComputers();
            var note = bridge.sunshine.note;
            var message = bridge.firstSetup.message;

            // ---- The app's own computers ----
            var dialog = findChild(home, "addComputerDialog");
            findChild(home, "addComputerButton").clicked();
            compare(dialog.typeOptions.length, 4);
            // Lunaris says what it is; a connector without a note has its whole name only.
            verify(findChild(dialog.contentItem, "typeNote").visible);
            dialog.chooseType("rustdesk");
            wait(50);
            verify(!findChild(dialog.contentItem, "typeNote").visible);
            compare(findChild(dialog.contentItem, "typeFullName").text, I18n.t("RustDesk (by IP address)"));
            dialog.chooseType("sunshine");
            wait(50);
            // The connector's note under the Type, in the person's language.
            var typeNote = findChild(dialog.contentItem, "typeNote");
            verify(typeNote.visible);
            compare(typeNote.text, I18n.t(note));
            verify(data.tag === "en" ? typeNote.text === note : typeNote.text !== note, "the note is translated");
            compare(field(dialog, "port").text, "47989");
            // The PIN is the connector's: nothing in the form asks for one.
            verify(field(dialog, "pin") === null && field(dialog, "password") === null);
            findChild(dialog.contentItem, "nameField").text = "Living room";
            field(dialog, "host").text = "192.168.1.70";
            shot("add-sunshine");

            var before = JSON.parse(bridge.stores.peers).peers.length;
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(50);
            verify(dialog.opened, "the dialog stays while the set-up waits");
            verify(dialog.setupWaiting);
            compare(dialog.setupId, "su-1");
            var view = findChild(dialog.contentItem, "setupView");
            verify(view.visible);
            compare(findChild(view, "setupMessage").text, I18n.t(message));
            verify(data.tag === "en" ? findChild(view, "setupMessage").text === message : findChild(view, "setupMessage").text !== message);
            var code = findChild(view, "setupCode");
            compare(code.text, "4711");
            verify(code.visible && code.font.pixelSize >= 32, "the code is large");
            verify(findChild(view, "setupWaitingText").visible);
            verify(!findChild(dialog.contentItem, "submitButton").visible && !findChild(dialog.contentItem, "nameField").visible, "the form is out of the way");
            compare(JSON.parse(bridge.stores.peers).peers.length, before, "no card before the set-up is done");
            shot("add-setup-waiting");

            // Asked once a second; the connector's next step replaces the first.
            bridge.setupAnswer = { state: "pending", message: "Almost there", code: null };
            tryVerify(function () { return dialog.setupMessage === "Almost there"; }, 3000);
            verify(bridge.setupAsked >= 1);
            verify(!code.visible, "a step without a code shows none");

            // Failed: the reason, and a way back to the form as it was filled in.
            bridge.setupAnswer = { state: "failed", error: "Pairing was not completed in time" };
            tryVerify(function () { return dialog.setupError !== ""; }, 3000);
            compare(dialog.setupError, I18n.t("Pairing was not completed in time"));
            verify(findChild(view, "setupErrorText").visible && !findChild(view, "setupMessage").visible);
            shot("add-setup-failed");
            var asked = bridge.setupAsked;
            wait(1300);
            compare(bridge.setupAsked, asked, "a set-up that failed is not asked about any more");
            findChild(view, "setupBackButton").clicked();
            verify(!dialog.setupShown && dialog.opened);
            compare(findChild(dialog.contentItem, "nameField").text, "Living room");
            compare(field(dialog, "host").text, "192.168.1.70");
            compare(bridge.cancelled.length, 0, "nothing to stop after it failed");

            // Closing the dialog while it waits stops the set-up; no card.
            bridge.setupAnswer = bridge.firstSetup;
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(50);
            verify(dialog.setupWaiting);
            findChild(view, "setupCancelButton").clicked();
            wait(250);
            verify(!dialog.opened);
            compare(bridge.cancelled, ["su-1"]);
            compare(JSON.parse(bridge.stores.peers).peers.length, before);
            asked = bridge.setupAsked;
            wait(1300);
            compare(bridge.setupAsked, asked, "nothing is asked after the dialog closed");

            // Done: the dialog closes and the card is there; nothing is cancelled.
            findChild(home, "addComputerButton").clicked();
            verify(!dialog.setupShown, "a new dialog starts with the form");
            dialog.chooseType("sunshine");
            wait(50);
            findChild(dialog.contentItem, "nameField").text = "Living room";
            field(dialog, "host").text = "192.168.1.70";
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(50);
            verify(dialog.setupWaiting);
            bridge.setupAnswer = { state: "done", id: "living-room-0001" };
            tryVerify(function () { return !dialog.opened; }, 3000);
            var added = peer("source:living-room-0001");
            verify(added !== null, "the computer has a card when its set-up is done");
            compare(added.type, "sunshine");
            compare(added.name, "Living room");
            compare(bridge.cancelled.length, 1, "a set-up that is done is not cancelled");
            verify(bridge.stores.peers.indexOf("4711") === -1, "the PIN is kept nowhere");

            // Files are offered only where the connector has them: not on the
            // Sunshine card (button and menu item), still on the RustDesk one.
            wait(50);
            var sunshineCard = cardOf("source:living-room-0001");
            verify(sunshineCard !== null);
            verify(!findChild(sunshineCard, "peerFilesButton").visible, "no Files button without files");
            findChild(sunshineCard, "peerMoreButton").clicked();
            wait(300);
            verify(findChild(sunshineCard, "peerMenu").visible);
            verify(!findChild(sunshineCard, "peerFilesItem").visible, "no Files in the menu without files");
            shot("home-card-menu-no-files");
            findChild(sunshineCard, "peerMenu").close();
            wait(250);
            var rustdeskCard = cardOf("source:den-pc-1a2b");
            verify(findChild(rustdeskCard, "peerFilesButton").visible, "a connector with files keeps the button");
            findChild(rustdeskCard, "peerMoreButton").clicked();
            wait(300);
            verify(findChild(rustdeskCard, "peerFilesItem").visible);
            findChild(rustdeskCard, "peerMenu").close();
            wait(250);
            verify(findChild(cardOf("192.168.1.20:8091"), "peerFilesButton").visible, "a Lunaris computer has files");
            // A type that says nothing about what it can do (an older one) keeps it too.
            verify(home.sourceHasFiles({ type: "acme-view" }) && home.sourceHasFiles({ type: "not-installed" }));
            verify(!home.sourceHasFiles({ type: "sunshine" }));
            shot("home-list-no-files");

            // ---- Server tab: the same, over the server's REST ----
            bridge.setupAnswer = bridge.firstSetup;
            bridge.cancelled = [];
            home.peerTab = "server";
            var dashboard = home.dashboard;
            tryCompare(dashboard, "isAdmin", true);
            var hostId = bridge.hosts[0].id;
            var setupPath = "/api/hosts/" + hostId + "/sources/setup/su-1";
            var serverDialog = findChild(dashboard, "serverAddDialog");
            var serverView = findChild(serverDialog.contentItem, "setupView");
            function fill() {
                serverDialog.openDialog();
                wait(50);
                serverDialog.chooseType("sunshine");
                wait(50);
                findChild(serverDialog.contentItem, "nameField").text = "Living room";
                field(serverDialog, "host").text = "192.168.1.70";
            }
            fill();
            compare(findChild(serverDialog.contentItem, "typeNote").text, I18n.t(note));
            shot("server-add-sunshine");
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(50);
            compare(bridge.calls[0].method, "POST");
            compare(bridge.calls[0].path, "/api/hosts/" + hostId + "/sources");
            verify(serverDialog.opened && serverDialog.setupWaiting, "202: the dialog shows the set-up");
            compare(serverDialog.setupHost, hostId);
            compare(findChild(serverView, "setupMessage").text, I18n.t(message));
            compare(findChild(serverView, "setupCode").text, "4711");
            var listed = dashboard.sourcesList.length;
            shot("server-add-setup-waiting");

            // Asked once a second at the server, about that set-up on that computer.
            bridge.setupAnswer = { state: "pending", message: "Almost there", code: null };
            tryVerify(function () { return serverDialog.setupMessage === "Almost there"; }, 3000);
            var gets = bridge.calls.filter(function (c) { return c.method === "GET" && c.path === setupPath; });
            verify(gets.length >= 1);

            // Failed.
            bridge.setupAnswer = { state: "failed", error: "Pairing was not completed in time" };
            tryVerify(function () { return serverDialog.setupError !== ""; }, 3000);
            compare(serverDialog.setupError, I18n.t("Pairing was not completed in time"));
            shot("server-add-setup-failed");
            findChild(serverView, "setupCloseButton").clicked();
            wait(250);
            verify(!serverDialog.opened);
            compare(bridge.cancelled.length, 0);

            // The server does not know the set-up any more (404): said, not asked on.
            bridge.setupAnswer = bridge.firstSetup;
            fill();
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(50);
            bridge.setupStatus = 404;
            tryVerify(function () { return serverDialog.setupError !== ""; }, 3000);
            compare(serverDialog.setupError, I18n.t("That set-up is not there any more. Add the computer again."));
            bridge.setupStatus = 200;
            closeAll(serverDialog);

            // Closed while it waits: DELETE of that set-up.
            fill();
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(50);
            verify(serverDialog.setupWaiting);
            bridge.calls = [];
            findChild(serverView, "setupCancelButton").clicked();
            wait(250);
            verify(!serverDialog.opened);
            compare(bridge.cancelled, ["su-1"]);
            compare(bridge.calls[0].method, "DELETE");
            compare(bridge.calls[0].path, setupPath);
            compare(dashboard.sourcesList.length, listed);

            // Done: closed, and the card is listed.
            fill();
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(50);
            bridge.setupAnswer = { state: "done", id: "living-room-0001" };
            tryVerify(function () { return !serverDialog.opened; }, 3000);
            tryVerify(function () { return dashboard.sourcesList.length === listed + 1; }, 4000);
            compare(bridge.cancelled.length, 1);

            // The server's cards: Files only where the source's connector has files.
            function filesButtons(item, found) {
                if (item.objectName === "cardFilesButton" && item.visible) found.push(item);
                var children = item.children || [];
                for (var i = 0; i < children.length; i++) filesButtons(children[i], found);
                return found;
            }
            wait(100);
            // Online: the Lunaris computer, the RustDesk source (files) and the
            // Sunshine source (no files): two buttons for three cards.
            compare(dashboard.cards.filter(function (c) { return c.methods.length === 1 && c.methods[0].online; }).length, 3);
            compare(filesButtons(dashboard, []).length, 2);
            shot("server-list-no-files");
            // A server or computer from before this says nothing: the button is there as it was.
            bridge.serverSources = bridge.serverSources.map(function (s) { var c = Object.assign({}, s); delete c.capabilities; return c; });
            dashboard.refreshSources();
            tryVerify(function () { return filesButtons(dashboard, []).length === 3; }, 3000);
        }

        // ---- One card for each computer ----

        // The app's own computers of the group tests: an Office PC that is
        // also reached with RustDesk and Sunshine, a Den PC that is three
        // computers of other types, and two that are alone.
        function groupedHome(stub) {
            stub.extra = [stub.sunshine];
            stub.ownSources = [
                { id: "den-pc-1a2b", name: "Den PC", connector: "rustdesk", connector_name: stub.rustdesk.name, values: { host: "192.168.1.50", port: "21118" }, group: "g-0000000000de" },
                { id: "den-sun-2", name: "Den PC (Sunshine)", connector: "sunshine", connector_name: stub.sunshine.name, values: { host: "192.168.1.50" }, group: "g-0000000000de" },
                { id: "den-acme-3", name: "Den PC (Acme)", connector: "acme-view", connector_name: stub.acme.name, values: { station: "den", gateway: "gw.example.net", gateway_port: "7400" }, group: "g-0000000000de" },
                { id: "office-rd", name: "Office (RustDesk)", connector: "rustdesk", connector_name: stub.rustdesk.name, values: { host: "192.168.1.20" }, group: "g-00000000000f" },
                { id: "office-sun", name: "Office (Sunshine)", connector: "sunshine", connector_name: stub.sunshine.name, values: { host: "192.168.1.20" }, group: "g-00000000000f" },
                { id: "workshop-9", name: "Workshop", connector: "rustdesk", connector_name: stub.rustdesk.name, values: { host: "192.168.1.60" } }
            ];
            var source = function (id, name, type, used) { return { address: "source:" + id, name: name, type: type, sourceId: id, favorite: false, lastUsed: used }; };
            stub.stores = Object.assign({}, stub.stores, { peers: JSON.stringify({ peers: [
                { address: "192.168.1.20:8091", name: "Office PC", password: "saved", favorite: true, os: "windows", lastUsed: 60, group: "g-00000000000f" },
                source("den-pc-1a2b", "Den PC", "rustdesk", 50),
                source("den-sun-2", "Den PC (Sunshine)", "sunshine", 40),
                source("den-acme-3", "Den PC (Acme)", "acme-view", 30),
                source("office-rd", "Office (RustDesk)", "rustdesk", 20),
                source("office-sun", "Office (Sunshine)", "sunshine", 20),
                source("workshop-9", "Workshop", "rustdesk", 15),
                { address: "10.0.0.7:8091", name: "", password: "", favorite: false, os: "", lastUsed: 10 }
            ] }) });
        }

        function groupedServer(stub) {
            var h0 = stub.hosts[0], h1 = stub.hosts[1];
            var source = function (host, id, name, connector, over) {
                var c = connector === "rustdesk" ? stub.rustdesk : connector === "sunshine" ? stub.sunshine : stub.acme;
                return Object.assign({ host_id: host.id, host_name: host.name, id: id, name: name, connector: connector, connector_name: c.name, online: host.status === "Online", capabilities: c.capabilities }, over || {});
            };
            stub.extra = [stub.sunshine];
            stub.serverSources = [
                source(h0, "lab-server-9f31", "Lab server", "rustdesk"),
                source(h0, "office-rd", "Office (RustDesk)", "rustdesk", { group: "host:" + h0.id }),
                source(h0, "office-sun", "Office (Sunshine)", "sunshine", { group: "host:" + h0.id }),
                source(h0, "den-rd", "Den PC", "rustdesk", { group: "g-0000000000de" }),
                source(h0, "den-sun", "Den PC (Sunshine)", "sunshine", { group: "g-0000000000de" }),
                source(h1, "den-acme", "Den PC (Acme)", "acme-view", { group: "g-0000000000de" })
            ];
        }

        // ---- Finding a computer: Cards.js, the same cases as the web
        // client's tests (web/tests/computerCards.test.ts) ----
        function findCards() {
            var src = function (id, name, connector, over) {
                return Object.assign({ host_id: "h1", host_name: "Office PC", id: id, name: name, connector: connector,
                    connector_name: connector === "rustdesk" ? "RustDesk (by IP address)" : connector === "sunshine" ? "Sunshine / GameStream" : connector,
                    online: true }, over || {});
            };
            return Cards.buildCards(
                [{ id: "h1", name: "Máy văn phòng", status: "Online", address: "192.168.1.20:8091" }, { id: "h2", name: "Lab PC", status: "Offline" }],
                [src("rd-1", "Đức laptop", "rustdesk", { group: "host:h1" }),
                 src("sun-1", "Den PC", "sunshine", { host_name: "Gateway" }),
                 src("vnc-1", "Kiosk", "vnc", { connector_name: "VNC (screen sharing)" }),
                 src("odd-1", "Press room", "acme-view", { connector_name: "Acme Viewer" })]);
        }

        function test_find() {
            compare(Cards.foldText("Máy Đức ổ"), "may duc o");
            compare(Cards.foldText(undefined), "");
            var cards = findCards();
            var titles = function (q, type) { return Cards.filterCards(cards, q, type || "").map(function (c) { return c.title; }); };
            // Search: names, addresses and the names of sources, accents aside
            compare(titles("may"), ["Máy văn phòng"]);
            compare(titles("MÁY VĂN"), ["Máy văn phòng"]);
            compare(titles("duc"), ["Máy văn phòng"], "a source on the card is found");
            compare(titles("192.168.1.20"), ["Máy văn phòng"]);
            compare(titles("gateway"), ["Den PC"]);
            compare(titles("kiosk vnc"), ["Kiosk"], "every word must be found");
            compare(titles("kiosk rustdesk"), []);
            compare(titles("   ").length, 5);
            // A Lunaris computer's own addresses and a source's address are searched too
            var addressed = Cards.buildCards(
                [{ id: "h1", name: "Desk", status: "Online", addresses: ["192.168.7.10", "100.64.0.10"] }],
                [{ host_id: "h1", host_name: "Desk", id: "rd", name: "Den", connector: "rustdesk", connector_name: "RustDesk", online: true, address: "192.168.7.99:21118" }]);
            compare(Cards.filterCards(addressed, "100.64").map(function (c) { return c.title; }), ["Desk"]);
            compare(Cards.filterCards(addressed, "7.99").map(function (c) { return c.title; }), ["Den"]);
            // The address shown on a card for the way it connects with
            compare(Cards.methodAddress(addressed[0].methods[0]), "192.168.7.10, 100.64.0.10");
            compare(Cards.methodAddress(addressed[1].methods[0]), "192.168.7.99:21118");
            compare(Cards.methodAddress(cards[0].methods[0]), "192.168.1.20", "added by address: that address, without the usual port");
            compare(Cards.methodAddress({ host: { id: "x", addresses: [] } }), "");
            compare(Cards.methodAddress(cards[1].methods[0]), "", "a source whose address is not known");
            compare(Cards.methodAddress(undefined), "");
            // Types: known ones first, unknown ones kept
            compare(Cards.typesOf(cards).map(function (t) { return t.id; }), ["lunaris", "rustdesk", "sunshine", "acme-view", "vnc"]);
            compare(Cards.typesOf(cards).map(function (t) { return t.name; }), ["Lunaris", "RustDesk", "Sunshine", "Acme Viewer", "VNC"]);
            compare(Cards.cardTypes(cards[0]), ["lunaris", "rustdesk"]);
            compare(Cards.typesOf([]), []);
            // Filter: a grouped card matches if any of its ways is of the type
            compare(titles("", "rustdesk"), ["Máy văn phòng"]);
            compare(titles("", "lunaris"), ["Máy văn phòng", "Lab PC"]);
            compare(titles("", "acme-view"), ["Press room"]);
            compare(titles("may", "sunshine"), []);
            compare(titles("", "rdp"), []);
            compare(Cards.effectiveType(Cards.typesOf(cards), "rdp"), "", "a remembered type that is gone is All");
            compare(Cards.effectiveType(Cards.typesOf(cards), "vnc"), "vnc");
            // Logos: the new types have one, an unknown type has none (its letter)
            compare(Cards.typeLogo("rdp"), "logos/rdp.svg");
            compare(Cards.typeLogo("vnc"), "logos/vnc.svg");
            compare(Cards.typeLogo("acme-view"), "");
            // The view and the type are remembered with this device's view settings
            ViewPrefs.computerView = "list";
            ViewPrefs.computerType = "rustdesk";
            var saved = ViewPrefs.toJson();
            ViewPrefs.computerView = "grid";
            ViewPrefs.computerType = "";
            ViewPrefs.fromJson(saved);
            compare(ViewPrefs.computerView, "list");
            compare(ViewPrefs.computerType, "rustdesk");
            ViewPrefs.fromJson(JSON.stringify({ computerView: "table", computerType: "<b>" }));
            compare(ViewPrefs.computerView, "list", "nonsense changes nothing");
            compare(ViewPrefs.computerType, "rustdesk");
            ViewPrefs.computerView = "grid";
            ViewPrefs.computerType = "";
        }

        // More types for the finder's pictures: RDP and VNC (the logos of
        // our own), an unknown type and a Vietnamese name.
        function finderHome(stub) {
            groupedHome(stub);
            stub.ownSources = stub.ownSources.concat([
                { id: "acct-rdp", name: "Máy kế toán", connector: "rdp", connector_name: "Remote Desktop (RDP)", values: { host: "192.168.1.61" } },
                { id: "kiosk-vnc", name: "Kiosk sảnh", connector: "vnc", connector_name: "VNC", values: { host: "192.168.1.70" } }
            ]);
            var peers = JSON.parse(stub.stores.peers).peers.concat([
                { address: "source:acct-rdp", name: "Máy kế toán", type: "rdp", sourceId: "acct-rdp", favorite: true, lastUsed: 55 },
                { address: "source:kiosk-vnc", name: "Kiosk sảnh", type: "vnc", sourceId: "kiosk-vnc", favorite: false, lastUsed: 5 }
            ]);
            stub.stores = Object.assign({}, stub.stores, { peers: JSON.stringify({ peers: peers }) });
        }

        function chipIds(finder) { return all(finder, "typeChip", []).map(function (c) { return c.typeId; }); }
        function click(item) { mouseClick(item, item.width / 2, item.height / 2); }

        function test_finder_data() {
            return [{ tag: "en-dark", lang: "en", theme: "dark" }, { tag: "vi-light", lang: "vi", theme: "light" }];
        }

        function test_finder(data) {
            start(data.lang, function (stub) { finderHome(stub); groupedServer(stub); }, data.theme);
            var home = test.home;
            var suffix = "-" + data.theme;
            var finder = findChild(home, "peerFinder");
            var grid = findChild(home, "peerGrid");
            verify(finder.visible);
            compare(grid.count, 6);
            compare(chipIds(finder), ["", "lunaris", "rustdesk", "sunshine", "acme-view", "rdp", "vnc"]);
            compare(findChild(finder, "computerCount").text, I18n.t("%1 computers").arg(6));
            shot("finder-home-grid" + suffix);

            // The RDP and VNC chips show their logos; the unknown type its letter.
            var logos = all(finder, "typeLogoPicture", []).filter(function (l) { return l.visible; });
            verify(logos.some(function (l) { return String(l.source).indexOf("logos/rdp.svg") !== -1 && l.status === Image.Ready; }), "the RDP logo is shown");
            verify(logos.some(function (l) { return String(l.source).indexOf("logos/vnc.svg") !== -1 && l.status === Image.Ready; }), "the VNC logo is shown");
            ["lunaris", "rustdesk", "sunshine", "rdp", "vnc"].forEach(function (id) {
                verify(logos.some(function (l) { return String(l.source).indexOf("logos/" + id + ".svg") !== -1 && l.status === Image.Ready; }), "the " + id + " chip has its logo");
            });
            var acme = all(finder, "typeChip", []).filter(function (c) { return c.typeId === "acme-view"; })[0];
            var letters = all(acme, "typeLogoLetter", []).filter(function (l) { return l.visible; });
            compare(letters.length, 1, "a type without a logo has its letter");
            compare(letters[0].text, "A");

            // List: one row per computer, the same cards
            click(findChild(finder, "view-list"));
            compare(ViewPrefs.computerView, "list");
            tryCompare(grid, "cellHeight", 62);
            compare(grid.count, 6);
            wait(50);
            var rows = all(grid, "peerRow", []).filter(function (r) { return r.visible; });
            compare(rows.length, 6);
            shot("finder-home-list" + suffix);

            // A type: the grouped Office card has RustDesk among its ways.
            var rustdeskChip = all(finder, "typeChip", []).filter(function (c) { return c.typeId === "rustdesk"; })[0];
            click(rustdeskChip);
            compare(ViewPrefs.computerType, "rustdesk");
            compare(home.shownCards.map(function (c) { return c.key; }).sort(),
                    ["group:g-0000000000de", "host:192.168.1.20:8091", "source::workshop-9"]);
            compare(findChild(finder, "computerCount").text, I18n.t("%1 of %2 computers").arg(3).arg(6));
            shot("finder-home-filter" + suffix);

            // Search: accents aside ("ke toan" finds "Máy kế toán"), with the filter: none.
            var search = findChild(finder, "computerSearch");
            search.forceActiveFocus();
            keyClick(Qt.Key_K); keyClick(Qt.Key_E); keyClick(Qt.Key_Space); keyClick(Qt.Key_T); keyClick(Qt.Key_O);
            compare(home.searchText, "ke to");
            compare(grid.count, 0);
            var noMatch = findChild(home, "peerNoMatch");
            verify(noMatch.visible);
            shot("finder-home-nomatch" + suffix);
            ViewPrefs.computerType = "";
            compare(home.shownCards.map(function (c) { return c.title; }), ["Máy kế toán"]);
            // Esc clears the search
            search.forceActiveFocus();
            keyClick(Qt.Key_Escape);
            compare(home.searchText, "");
            compare(grid.count, 6);
            // The empty state's button shows every computer again
            home.searchText = "nothing like this";
            ViewPrefs.computerType = "vnc";
            compare(grid.count, 0);
            click(findChild(noMatch, "clearFinding"));
            compare(home.searchText, "");
            compare(ViewPrefs.computerType, "");
            compare(grid.count, 6);
            // A remembered type no card has is All.
            ViewPrefs.computerType = "gone-type";
            compare(home.peerType, "");
            compare(grid.count, 6);
            ViewPrefs.computerType = "";

            // Narrow window: the chips wrap, the rows keep Connect.
            window.width = 820;
            wait(100);
            shot("finder-home-list-narrow" + suffix);
            window.width = 1180;
            wait(50);

            // ---- Server tab: the same finder ----
            home.peerTab = "server";
            var dashboard = home.dashboard;
            tryCompare(dashboard, "isAdmin", true);
            tryVerify(function () { return dashboard.cards.length > 0; }, 3000);
            wait(100);
            var serverFinder = findChild(dashboard, "serverFinder");
            verify(serverFinder !== null && serverFinder.visible);
            compare(chipIds(serverFinder), ["", "lunaris", "rustdesk", "sunshine", "acme-view"]);
            var serverGrid = findChild(dashboard, "serverGrid");
            compare(serverGrid.count, dashboard.cards.length);
            var serverRows = all(serverGrid, "serverRow", []).filter(function (r) { return r.visible; });
            compare(serverRows.length, dashboard.cards.length);
            shot("finder-server-list" + suffix);
            dashboard.searchText = "den";
            compare(dashboard.shownCards.map(function (c) { return c.key; }), ["group:g-0000000000de"]);
            dashboard.searchText = "";
            ViewPrefs.computerType = "acme-view";
            compare(dashboard.shownCards.map(function (c) { return c.key; }), ["group:g-0000000000de"], "the Den card has Acme among its ways");
            shot("finder-server-filter" + suffix);
            ViewPrefs.computerType = "";
            ViewPrefs.computerView = "grid";
            wait(50);
            shot("finder-server-grid" + suffix);
            dashboard.searchText = "zzz";
            verify(findChild(dashboard, "serverNoMatch").visible);
            shot("finder-server-nomatch" + suffix);
            dashboard.searchText = "";
        }

        function test_groups_data() {
            return [{ tag: "en", theme: "dark" }, { tag: "vi", theme: "light" }];
        }

        function test_groups(data) {
            start(data.tag, function (stub) { groupedHome(stub); groupedServer(stub); }, data.theme);
            var home = test.home;
            var suffix = data.theme === "light" ? "-light" : "";

            // ---- Home: four cards for eight entries ----
            compare(findChild(home, "peerGrid").count, 4);
            var office = cardByKey("host:192.168.1.20:8091");
            var den = cardByKey("group:g-0000000000de");
            var workshop = cardByKey("source::workshop-9");
            var plain = cardByKey("host:10.0.0.7:8091");
            verify(office && den && workshop && plain);
            // A Lunaris computer that took in two computers of other types.
            verify(office.multi);
            compare(labels(office.cardData), ["Lunaris", "RustDesk", "Sunshine"]);
            compare(office.title, "Office PC");
            // Three computers of other types that are one: named after the first.
            compare(labels(den.cardData), ["RustDesk", "Acme Viewer", "Sunshine"]);
            compare(den.title, "Den PC");
            // The cards of those who never grouped anything are what they were.
            verify(!workshop.multi && workshop.isSource && !plain.multi && !plain.isSource);
            verify(!findChild(workshop, "peerChips").visible && !findChild(plain, "peerChips").visible);
            verify(findChild(office, "peerChips").visible && findChild(den, "peerChips").visible);
            compare(all(findChild(office, "peerChips"), "methodChip").length, 3);
            shot("home-groups" + suffix);

            // A computer of another type that does not answer (shut down) is
            // offline; one not checked yet is not said to be.
            compare(home.sourceOnline("workshop-9"), -1);
            verify(cardByKey("source::workshop-9").cardData.methods[0].online, "not checked yet: as before");
            bridge.sourceStatus = { "workshop-9": 0, "office-sun": 1 };
            home.refreshSourceStatus();
            wait(50);
            compare(home.sourceOnline("workshop-9"), 0);
            verify(!cardByKey("source::workshop-9").cardData.methods[0].online);
            verify(!cardByKey("source::office-sun") && cardByKey("host:192.168.1.20:8091").cardData.methods.some(function (m) { return m.key === "source::office-sun" && m.online; }));
            // Nothing changed: the cards are not made again.
            var before = home.tabCards;
            home.refreshSourceStatus();
            verify(home.tabCards === before);
            bridge.sourceStatus = {};
            home.refreshSourceStatus();
            wait(50);
            workshop = cardByKey("source::workshop-9");
            office = cardByKey("host:192.168.1.20:8091");

            // Nothing was chosen yet: the Lunaris computer itself; a click connects with it.
            compare(office.method.key, "lunaris");
            verify(!office.isSource);
            mouseClick(office, office.width / 2, 70);
            compare(sessions.count, 1);
            compare(sessions.signalArguments[0][0], "ws://192.168.1.20:8091");
            compare(sessions.signalArguments[0][15], "");
            sessions.clear();
            home.setStatus("", false);
            wait(50);

            // A click on a chip chooses that way: the card acts on it from then on.
            office = cardByKey("host:192.168.1.20:8091");
            var chips = all(findChild(office, "peerChips"), "methodChip");
            var sunshineChip = chips.filter(function (c) { return c.methodKey === "source::office-sun"; })[0];
            verify(sunshineChip !== undefined);
            mouseClick(sunshineChip);
            compare(sessions.count, 0, "choosing does not connect");
            compare(office.method.key, "source::office-sun");
            verify(office.isSource);
            compare(office.peer.sourceId, "office-sun");
            verify(!findChild(office, "peerFilesButton").visible, "Sunshine has no files");
            compare(settings().cardMethods["host:192.168.1.20:8091"], "source::office-sun", "kept with the app's settings");
            mouseClick(office, office.width / 2, 70);
            compare(sessions.count, 1);
            compare(sessions.signalArguments[0][0], "bridge:");
            compare(sessions.signalArguments[0][15], "office-sun");
            sessions.clear();
            home.setStatus("", false);
            wait(50);

            // The card's menu: each way, to connect with it at once.
            office = cardByKey("host:192.168.1.20:8091");
            findChild(office, "peerMoreButton").clicked();
            wait(300);
            var menu = findChild(office, "peerMenu");
            verify(menu.visible);
            var entries = [];
            for (var i = 0; i < menu.count; i++) if (menu.itemAt(i).objectName === "peerMethodItem") entries.push(menu.itemAt(i));
            compare(entries.length, 3);
            compare(entries[1].text, I18n.t("Connect with %1").arg("RustDesk"));
            shot("home-group-menu" + suffix);
            entries[1].triggered();
            menu.close();
            wait(250);
            compare(sessions.count, 1);
            compare(sessions.signalArguments[0][15], "office-rd");
            compare(settings().cardMethods["host:192.168.1.20:8091"], "source::office-rd", "the way used last is remembered");
            sessions.clear();
            home.setStatus("", false);

            // The app starts again: the card connects with the way used last.
            var stores = bridge.stores;
            homeLoader.active = false;
            homeLoader.active = true;
            test.home = homeLoader.item;
            home = test.home;
            sessions.target = home;
            sessions.clear();
            bridge.stores = stores;
            wait(100);
            office = cardByKey("host:192.168.1.20:8091");
            compare(office.method.key, "source::office-rd");
            verify(findChild(office, "peerFilesButton").visible, "RustDesk has files");

            // ---- One more way to connect to a card ----
            var dialog = findChild(home, "addComputerDialog");
            // To a computer of another type that is alone: it gets a group,
            // and the new one is added with it.
            workshop = cardByKey("source::workshop-9");
            bridge.groupsSaid = [];
            home.addMethodTo(workshop.cardData);
            wait(50);
            verify(dialog.opened && dialog.addingMethod);
            compare(findChild(dialog.contentItem, "dialogTitle").text, I18n.t("Add a connection method to %1").arg("Workshop"));
            verify(findChild(dialog.contentItem, "methodNote").visible);
            verify(findChild(dialog.contentItem, "typeOption-lunaris") === null, "a Lunaris computer is a card of its own");
            compare(dialog.typeOptions.length, 3);
            compare(findChild(dialog.contentItem, "nameField").text, "Workshop");
            dialog.chooseType("acme-view");
            wait(50);
            field(dialog, "station").text = "workshop";
            field(dialog, "token").text = "abc";
            shot("add-method" + suffix);
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(250);
            verify(!dialog.opened);
            compare(bridge.groupsSaid.length, 2);
            compare(bridge.groupsSaid[0].op, "update");
            compare(bridge.groupsSaid[0].id, "workshop-9");
            verify(/^g-[0-9a-f]{12}$/.test(bridge.groupsSaid[0].group));
            compare(bridge.groupsSaid[1].op, "add");
            compare(bridge.groupsSaid[1].group, bridge.groupsSaid[0].group);
            var workshopGroup = bridge.groupsSaid[0].group;
            var workshopCard = cardByKey("group:" + workshopGroup);
            verify(workshopCard !== null && cardByKey("source::workshop-9") === null, "one card with two ways");
            // (Both are called "Workshop": in the order of their ids.)
            compare(labels(workshopCard.cardData), ["Acme Viewer", "RustDesk"]);
            compare(workshopCard.title, "Workshop");
            compare(findChild(home, "peerGrid").count, 4);

            // To a Lunaris computer, with a type that has a set-up (a PIN):
            // the computer gets a group, the set-up is shown and asked
            // about, and the new way is on its card when it is done.
            plain = cardByKey("host:10.0.0.7:8091");
            bridge.groupsSaid = [];
            home.addMethodTo(plain.cardData);
            wait(50);
            dialog.chooseType("sunshine");
            wait(50);
            compare(findChild(dialog.contentItem, "typeNote").text, I18n.t(bridge.sunshine.note));
            field(dialog, "host").text = "10.0.0.7";
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(50);
            verify(dialog.opened && dialog.setupWaiting, "the set-up is shown");
            compare(findChild(dialog.contentItem, "setupCode").text, "4711");
            var peerGroup = peer("10.0.0.7:8091").group;
            verify(/^g-[0-9a-f]{12}$/.test(peerGroup), "the Lunaris computer has a group now");
            compare(bridge.groupsSaid.length, 1);
            compare(bridge.groupsSaid[0].group, peerGroup);
            shot("add-method-setup" + suffix);
            bridge.setupAnswer = { state: "done", id: "living-room-0001" };
            tryVerify(function () { return !dialog.opened; }, 3000);
            wait(50);
            plain = cardByKey("host:10.0.0.7:8091");
            verify(plain.multi);
            compare(labels(plain.cardData), ["Lunaris", "Sunshine"]);
            compare(bridge.cancelled.length, 0);

            // Added in the ordinary way at the address of a computer that is
            // there: the helper puts the two together, and so does the list.
            findChild(home, "addComputerButton").clicked();
            dialog.chooseType("rustdesk");
            wait(50);
            findChild(dialog.contentItem, "nameField").text = "Living room (RustDesk)";
            field(dialog, "host").text = "10.0.0.7";
            field(dialog, "password").text = "pw";
            bridge.groupsSaid = [];
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(bridge.groupsSaid[0].group, "*", "nothing is said: the helper groups by address");
            plain = cardByKey("host:10.0.0.7:8091");
            compare(labels(plain.cardData), ["Lunaris", "Sunshine", "RustDesk"]);

            // ---- Edit: which computer it is ----
            home.editSource(peer("source:den-acme-3"));
            wait(50);
            verify(findChild(dialog.contentItem, "sameView").visible);
            compare(dialog.sameCurrent, "group:g-0000000000de");
            verify(findChild(dialog.contentItem, "sameOption-group:g-0000000000de").chosen);
            verify(findChild(dialog.contentItem, "sameOption-host:192.168.1.20:8091") !== null);
            shot("edit-same" + suffix);
            // Left where it is: nothing is said of its group.
            bridge.groupsSaid = [];
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(bridge.groupsSaid[0].group, "*");
            compare(labels(cardByKey("group:g-0000000000de").cardData).length, 3);
            // A computer of its own again.
            home.editSource(peer("source:den-acme-3"));
            wait(50);
            mouseClick(findChild(dialog.contentItem, "sameOption-own"));
            verify(findChild(dialog.contentItem, "sameOption-own").chosen);
            bridge.groupsSaid = [];
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(bridge.groupsSaid[0].group, "");
            compare(labels(cardByKey("group:g-0000000000de").cardData), ["RustDesk", "Sunshine"]);
            verify(cardByKey("source::den-acme-3") !== null, "a card of its own");
            // ... and the same computer as the Office PC.
            home.editSource(peer("source:den-acme-3"));
            wait(50);
            compare(dialog.sameCurrent, "");
            dialog.sameChosen = "host:192.168.1.20:8091";
            bridge.groupsSaid = [];
            findChild(dialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(bridge.groupsSaid[0].group, "g-00000000000f");
            compare(labels(cardByKey("host:192.168.1.20:8091").cardData), ["Lunaris", "Acme Viewer", "RustDesk", "Sunshine"]);

            // ---- Remove acts on the way that is chosen; the last one takes the card ----
            den = cardByKey("group:g-0000000000de");
            compare(den.method.key, "source::den-pc-1a2b");
            home.removeSourcePeer(den.peer);
            wait(50);
            den = cardByKey("group:g-0000000000de");
            compare(labels(den.cardData), ["Sunshine"]);
            verify(!den.multi, "one way left: a card like any other");
            home.removeSourcePeer(den.peer);
            wait(50);
            verify(cardByKey("group:g-0000000000de") === null);
            // A Lunaris computer that is removed leaves its other ways, as a card of theirs.
            home.removePeer("192.168.1.20:8091");
            wait(50);
            verify(cardByKey("host:192.168.1.20:8091") === null);
            compare(labels(cardByKey("group:g-00000000000f").cardData), ["Acme Viewer", "RustDesk", "Sunshine"]);

            // ---- Server tab ----
            home.peerTab = "server";
            var dashboard = home.dashboard;
            tryCompare(dashboard, "isAdmin", true);
            var h0 = bridge.hosts[0].id;
            tryCompare(dashboard.cards, "length", 4);
            var keys = dashboard.cards.map(function (c) { return c.key; });
            compare(keys, ["host:" + h0, "host:" + bridge.hosts[1].id, "group:g-0000000000de", "source:" + h0 + ":lab-server-9f31"]);
            var officeCard = dashboard.cards[0];
            var denCard = dashboard.cards[2];
            compare(labels(officeCard), ["Lunaris", "RustDesk", "Sunshine"]);
            compare(labels(denCard), ["RustDesk", "Acme Viewer", "Sunshine"]);
            compare(denCard.title, "Den PC");
            verify(!denCard.methods[1].online, "through a computer that is offline");
            wait(100);
            compare(all(dashboard, "cardChips", []).filter(function (c) { return c.visible; }).length, 2, "chips only where there is a choice");
            // One more way: a button on a card with one way, in the list of a card with several.
            compare(all(dashboard, "cardAddMethodButton", []).filter(function (b) { return b.visible; }).length, 2);
            compare(all(dashboard, "cardChooseButton", []).filter(function (b) { return b.visible; }).length, 2);
            shot("server-groups" + suffix);

            // The Lunaris computer itself until another way is chosen.
            compare(dashboard.methodOf(officeCard).key, "lunaris");
            dashboard.connectMethod(officeCard, dashboard.methodOf(officeCard));
            compare(sessions.count, 1);
            compare(sessions.signalArguments[0][2], h0);
            compare(sessions.signalArguments[0][15], "");
            sessions.clear();
            // A chip chooses; kept with the app's settings; Files follow the way.
            var serverChips = all(dashboard, "methodChip", []).filter(function (c) { return c.methodKey === "source:" + h0 + ":office-sun"; });
            compare(serverChips.length, 1);
            mouseClick(serverChips[0]);
            compare(sessions.count, 0);
            compare(dashboard.methodOf(officeCard).key, "source:" + h0 + ":office-sun");
            compare(settings().serverCardMethods["host:" + h0], "source:" + h0 + ":office-sun");
            wait(50);
            // Office: Sunshine (no files). Den: RustDesk (files). Lab server: RustDesk (files). Lab PC: offline.
            compare(all(dashboard, "cardFilesButton", []).filter(function (b) { return b.visible; }).length, 2);
            // A way that cannot be connected to now is not the one a card
            // falls on, and choosing it does not connect.
            dashboard.chooseMethod(denCard, denCard.methods[1].key);
            compare(dashboard.methodOf(denCard).key, denCard.methods[0].key, "the remembered one is offline: the first that is not");
            dashboard.connectMethod(denCard, denCard.methods[1]);
            compare(sessions.count, 0);

            // The list beside Connect: connect with another way at once.
            var chooseButtons = all(dashboard, "cardChooseButton", []).filter(function (b) { return b.visible; });
            var methodMenu = findChild(dashboard, "methodMenu");
            chooseButtons[0].clicked();
            wait(300);
            verify(methodMenu.visible);
            var items = [];
            for (var m = 0; m < methodMenu.count; m++) if (methodMenu.itemAt(m).objectName === "methodItem") items.push(methodMenu.itemAt(m));
            compare(items.length, 3);
            compare(items[1].text, I18n.t("Connect with %1").arg("RustDesk"));
            verify(findChild(methodMenu, "methodAddItem").visible);
            shot("server-chooser" + suffix);
            items[1].triggered();
            methodMenu.close();
            wait(250);
            compare(sessions.count, 1);
            compare(sessions.signalArguments[0][2], h0);
            compare(sessions.signalArguments[0][15], "office-rd");
            compare(settings().serverCardMethods["host:" + h0], "source:" + h0 + ":office-rd");
            sessions.clear();
            home.setStatus("", false);
            // In the list of the Den PC the way that is offline cannot be picked.
            methodMenu.openFor(denCard, chooseButtons[1]);
            wait(300);
            var denItems = [];
            for (m = 0; m < methodMenu.count; m++) if (methodMenu.itemAt(m).objectName === "methodItem") denItems.push(methodMenu.itemAt(m));
            verify(denItems[0].enabled && !denItems[1].enabled && denItems[2].enabled);
            methodMenu.close();
            wait(250);

            // Remove: of a card with several ways, that way only.
            var removeDialog = findChild(dashboard, "removeSourceDialog");
            removeDialog.openFor(officeCard.methods[1].source, officeCard, officeCard.methods[1]);
            compare(findChild(removeDialog.contentItem, "removeSourceText").text, I18n.t("Remove %1 from %2? The other ways to connect stay.").arg("RustDesk").arg("Office PC"));
            shot("server-remove-method" + suffix);
            removeDialog.close();
            wait(250);

            // One more way to a Lunaris computer: named in the form as "host:<id>".
            var serverDialog = findChild(dashboard, "serverAddDialog");
            serverDialog.openMethod(dashboard.cards[1]);
            wait(50);
            verify(serverDialog.addingMethod);
            compare(findChild(serverDialog.contentItem, "nameField").text, "Lab PC");
            verify(findChild(serverDialog.contentItem, "typeOption-lunaris") === null);
            serverDialog.chooseType("rustdesk");
            wait(50);
            field(serverDialog, "host").text = "10.0.0.7";
            field(serverDialog, "password").text = "pw";
            shot("server-add-method" + suffix);
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            verify(!serverDialog.opened);
            compare(bridge.calls[0].method, "POST");
            compare(JSON.parse(bridge.calls[0].body).group, "host:" + bridge.hosts[1].id);
            tryVerify(function () { return dashboard.cards[1].methods.length === 2; }, 3000);

            // To a computer of another type that is alone: it is given a
            // group first (PUT), then the new one is added with it (POST).
            var lab = dashboard.cardByKey("source:" + h0 + ":lab-server-9f31");
            serverDialog.openMethod(lab);
            wait(50);
            serverDialog.chooseType("sunshine");
            wait(50);
            field(serverDialog, "host").text = "192.168.1.90";
            bridge.calls = [];
            bridge.setupAnswer = bridge.firstSetup;
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(100);
            var writes = bridge.calls.filter(function (c) { return c.method === "PUT" || c.method === "POST"; });
            compare(writes.length, 2);
            compare(writes[0].method, "PUT");
            compare(writes[0].path, "/api/hosts/" + h0 + "/sources/lab-server-9f31");
            var given = JSON.parse(writes[0].body);
            verify(/^g-[0-9a-f]{12}$/.test(given.group));
            compare(given.name, "Lab server");
            compare(given.values, ({}));
            compare(writes[1].method, "POST");
            compare(JSON.parse(writes[1].body).group, given.group);
            // Its type has a set-up: the PIN, then the card has both ways.
            verify(serverDialog.opened && serverDialog.setupWaiting);
            compare(findChild(serverDialog.contentItem, "setupCode").text, "4711");
            shot("server-add-method-setup" + suffix);
            bridge.setupAnswer = { state: "done", id: "living-room-0001" };
            tryVerify(function () { return !serverDialog.opened; }, 3000);
            tryVerify(function () { return dashboard.cardByKey("group:" + given.group) !== null; }, 4000);
            compare(labels(dashboard.cardByKey("group:" + given.group)), ["RustDesk", "Sunshine"]);
            compare(bridge.cancelled.length, 0);

            // The server refuses (the computer is offline): its words, and nothing is added.
            serverDialog.openMethod(dashboard.cards[1]);
            wait(50);
            serverDialog.chooseType("rustdesk");
            wait(50);
            field(serverDialog, "host").text = "10.0.0.8";
            field(serverDialog, "password").text = "pw";
            bridge.nextWrite = { status: 409, body: JSON.stringify({ error: "That computer is offline" }) };
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(100);
            verify(serverDialog.opened);
            compare(serverDialog.error, I18n.t("That computer is offline"));
            closeAll(serverDialog);

            // Edit: which computer it is. Left alone: no "group" in what is sent.
            var denAcme = dashboard.sourcesList.filter(function (x) { return x.id === "den-acme"; })[0];
            dashboard.editSource(denAcme);
            wait(50);
            compare(serverDialog.sameCurrent, "group:g-0000000000de");
            verify(findChild(serverDialog.contentItem, "sameOption-host:" + h0) !== null);
            shot("server-edit-same" + suffix);
            mouseClick(findChild(serverDialog.contentItem, "sameOption-own"));
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(bridge.calls[0].method, "PUT");
            compare(JSON.parse(bridge.calls[0].body).group, "", "an empty group: a computer of its own");
            tryVerify(function () { return dashboard.cardByKey("source:" + bridge.hosts[1].id + ":den-acme") !== null; }, 3000);
            // The same computer as a Lunaris one.
            dashboard.editSource(dashboard.sourcesList.filter(function (x) { return x.id === "den-acme"; })[0]);
            wait(50);
            serverDialog.sameChosen = "host:" + h0;
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            compare(JSON.parse(bridge.calls[0].body).group, "host:" + h0);

            // Who is no administrator sees the ways and connects, and adds nothing.
            bridge.role = "user";
            dashboard.refreshRole();
            tryCompare(dashboard, "isAdmin", false);
            wait(100);
            compare(all(dashboard, "cardAddMethodButton", []).filter(function (b) { return b.visible; }).length, 0);
            verify(all(dashboard, "cardChips", []).filter(function (c) { return c.visible; }).length >= 2);
            methodMenu.openFor(dashboard.cards[0], all(dashboard, "cardChooseButton", []).filter(function (b) { return b.visible; })[0]);
            wait(300);
            verify(!findChild(methodMenu, "methodAddItem").visible);
            methodMenu.close();
            wait(250);
        }

        // ---- What another computer says is shown as it is, never as markup ----
        Text { id: plainRef; visible: false; textFormat: Text.PlainText }
        function shownLiterally(item, text) {
            verify(item !== null && item !== undefined, "an item for " + text);
            compare(item.textFormat, Text.PlainText);
            compare(item.text, text);
            // As wide as the same words drawn as plain text (markup that
            // was taken for markup would be drawn narrower, or as a picture).
            plainRef.font = item.font;
            plainRef.text = text;
            if (item.elide === Text.ElideNone && item.wrapMode === Text.NoWrap)
                compare(Math.round(item.implicitWidth), Math.round(plainRef.implicitWidth));
        }

        // ---- The dialog itself: the row of types, logos, rows of fields, choices ----

        // Which fields share a row: the same cases as the web's test
        // (web/tests/computerCards.test.ts, "which fields of a form share a row").
        function test_field_rows() {
            var f = function (key, kind, def) { return { key: key, kind: kind || "text", "default": def || "" }; };
            var rows = function (fields) { return Cards.layoutFields(fields).map(function (p) { return p.field.key + ":" + p.span; }).join(" "); };
            compare(rows([f("host"), f("port", "port", "21118"), f("password", "secret")]), "host:4 port:2 password:6");
            compare(rows([f("host"), f("port", "port", "47989"), f("app", "text", "Desktop"), f("resolution", "text", "1920x1080"), f("bitrate", "text", "20"), f("pointer", "choice", "picture")]),
                "host:4 port:2 app:2 resolution:2 bitrate:2 pointer:6");
            compare(rows([f("station"), f("gateway", "text", "gw.example.net"), f("gateway_port", "port", "7400"), f("token", "secret"), f("pin", "secret")]),
                "station:6 gateway:4 gateway_port:2 token:3 pin:3");
            compare(rows([f("a", "choice", "x"), f("p", "port", "1")]), "a:3 p:3");
            // Fields left alone in a row share one, two by two, in order; options
            // to click with long names (more than 20 characters together) do not.
            var o = function (key, labels, def) {
                var field = f(key, "choice", def);
                field.options = labels.map(function (l, n) { return { value: "v" + n, label: l }; });
                return field;
            };
            var rdp = [f("host"), f("port", "port", "3389"), f("username"), f("password", "secret"), f("domain"),
                       o("security", ["NLA (recommended)", "TLS (sign in on the computer's screen)", "Either"], "nla"), f("resolution", "text", "auto")];
            compare(rows(rdp), "host:4 port:2 username:3 password:3 domain:3 security:3 resolution:6");
            compare(rows([f("host"), f("port", "port", "5900"), f("password", "secret"), o("view_only", ["Control", "View only"], "off")]),
                "host:4 port:2 password:3 view_only:3");
            compare(rows([f("a"), o("c", ["In the picture (host's pointer)", "Drawn here (no lag; arrow only)"]), f("b")]), "a:6 c:6 b:6");
            compare(rows([f("a"), f("b"), f("c")]), "a:3 b:3 c:6");
            // A drop-down: many options, or a long one; options to click otherwise.
            verify(Cards.choiceAsList(rdp[5]));
            verify(!Cards.choiceAsList(o("p", ["In the picture (host's pointer)", "Drawn here (no lag; arrow only)"])));
            verify(!Cards.choiceAsList(o("m", ["Control", "View only"])));
            verify(Cards.choiceAsList(o("m", ["a", "b", "c", "d"])));
            verify(!Cards.choiceAsList(o("m", [])));
            verify(!Cards.choiceAsList(f("t")));
            compare(rows([]), "");
            var several = function (n) { var all = []; for (var i = 0; i < n; i++) all.push(f("s" + i, "text", "v")); return all; };
            compare(rows(several(1)), "s0:6");
            compare(rows(several(2)), "s0:3 s1:3");
            compare(rows(several(4)), "s0:3 s1:3 s2:3 s3:3");
            compare(rows(several(5)), "s0:2 s1:2 s2:2 s3:3 s4:3");
            compare(rows(several(7)), "s0:2 s1:2 s2:2 s3:3 s4:3 s5:3 s6:3");
            compare(rows([f("a", "text", "1"), f("b", "text", "2"), f("p", "port", "3")]), "a:6 b:4 p:2");
            compare(rows([f("p", "port", "1"), f("q", "port", "2")]), "p:3 q:3");
            compare(Cards.layoutFields([f("a", "text", "1"), f("b", "text", "2"), f("c", "text", "3"), f("d"), f("e", "port", "5")]).map(function (p) { return p.field.key; }),
                ["a", "b", "c", "d", "e"]);
            // The short name of a type, and which types have a logo.
            compare(Cards.typeShortName("rustdesk", "RustDesk (by IP address)"), "RustDesk");
            compare(Cards.typeShortName("sunshine", "Sunshine / GameStream"), "Sunshine");
            compare(Cards.typeShortName("acme-view", "Acme Viewer (beta)"), "Acme Viewer");
            compare(Cards.typeShortName("template", ""), "template");
            compare(Cards.typeShortName("constructor", "Odd"), "Odd");
            compare(Cards.typeLogo("constructor"), "");
            compare(Cards.typeLogo("lunaris"), "logos/lunaris.svg");
            compare(Cards.methodType({ source: { connector: "sunshine" } }), "sunshine");
            compare(Cards.methodType({ host: {} }), "lunaris");
        }

        function test_dialog_data() {
            return [{ tag: "en", theme: "dark" }, { tag: "vi", theme: "light" }];
        }

        function nearly(a, b) { return Math.abs(a - b) <= 1.5; }

        function tilesOf(dialog) {
            return dialog.typeOptions.map(function (o) { return findChild(dialog.contentItem, "typeOption-" + o.value); });
        }

        function test_dialog(data) {
            start(data.tag, function (stub) { groupedHome(stub); groupedServer(stub); }, data.theme);
            var home = test.home;
            var suffix = data.theme === "light" ? "-light" : "";
            var dialog = findChild(home, "addComputerDialog");
            findChild(home, "addComputerButton").clicked();
            wait(100);

            // Wider than it was, and never wider than the window: the types beside the form.
            verify(dialog.width >= 760 && dialog.width <= window.width - 40);
            verify(dialog.wide);
            verify(dialog.twoPane);

            // The types in a list at the left: the same left, the same width, top to bottom.
            var tiles = tilesOf(dialog);
            compare(tiles.length, 4);
            for (var i = 1; i < tiles.length; i++) {
                verify(nearly(tiles[i].x, tiles[0].x), "one column");
                verify(nearly(tiles[i].width, tiles[0].width), "equal widths");
                verify(tiles[i].y > tiles[i - 1].y + tiles[i - 1].height - 1);
            }
            verify(tiles[0].height <= 44, "compact rows");
            verify(findChild(dialog.contentItem, "typeChips").visible === false);
            compare(tiles.map(function (t) { return t.text; }), ["Lunaris", "RustDesk", "Acme Viewer", "Sunshine"]);
            compare(tiles[3].fullName, "Sunshine / GameStream");
            compare(tiles[3].Accessible.name, "Sunshine / GameStream");
            // The logos: Lunaris', RustDesk's and Sunshine's are pictures; a type without one keeps its letter.
            var logoOf = function (tile) { return findChild(tile, "tileLogo"); };
            verify(logoOf(tiles[0]).shown, "Lunaris' logo");
            verify(logoOf(tiles[1]).shown, "RustDesk's logo (an SVG)");
            verify(logoOf(tiles[3]).shown, "Sunshine's logo (an SVG)");
            verify(!logoOf(tiles[2]).shown);
            var letter = findChild(logoOf(tiles[2]), "typeLogoLetter");
            verify(letter.visible);
            compare(letter.text, "A");
            verify(!findChild(logoOf(tiles[1]), "typeLogoLetter").visible);
            // A picture that cannot be shown (a package without the SVG plugin): the letter, never an empty square.
            logoOf(tiles[1]).file = "logos/not-there.svg";
            tryCompare(logoOf(tiles[1]), "shown", false);
            verify(findChild(logoOf(tiles[1]), "typeLogoLetter").visible);
            compare(findChild(logoOf(tiles[1]), "typeLogoLetter").text, "R");
            logoOf(tiles[1]).file = Cards.typeLogo("rustdesk");
            tryCompare(logoOf(tiles[1]), "shown", true);

            // The note stands once, under the row, and follows the choice.
            var about = findChild(dialog.contentItem, "typeNote");
            var fullName = findChild(dialog.contentItem, "typeFullName");
            compare(fullName.text, "Lunaris");
            verify(about.visible && about.text.indexOf("Lunaris") !== -1);
            // ... at the right of the list, at the top of the form.
            var aboutAt = findChild(dialog.contentItem, "typeAbout").mapToItem(dialog.contentItem, 0, 0);
            var tileAt = tiles[0].mapToItem(dialog.contentItem, 0, 0);
            verify(aboutAt.x > tileAt.x + tiles[0].width);
            verify(nearly(aboutAt.y, findChild(dialog.contentItem, "typeRail").mapToItem(dialog.contentItem, 0, 0).y));
            findChild(dialog.contentItem, "addressField").text = "192.168.1.31";
            // The buttons are in sight, under the form.
            var buttons = findChild(dialog.contentItem, "buttonRow");
            var scroller = findChild(dialog.contentItem, "formScroller");
            verify(scroller.contentHeight <= scroller.height + 1, "nothing to scroll for Lunaris");
            shot("add-tiles-lunaris" + suffix);

            mouseClick(tiles[1]);
            wait(100);
            compare(dialog.typeId, "rustdesk");
            compare(fullName.text, I18n.t("RustDesk (by IP address)"));
            verify(!about.visible);
            // The address and its port share a row; the password has its own.
            var item = function (d, key) { return findChild(d.contentItem, "fieldItem-" + key); };
            verify(nearly(item(dialog, "host").y, item(dialog, "port").y), "host and port in one row");
            verify(item(dialog, "host").width > item(dialog, "port").width * 1.6);
            verify(item(dialog, "password").y > item(dialog, "host").y + 20);
            verify(nearly(item(dialog, "password").width, findChild(dialog.contentItem, "fieldGrid").width));
            verify(scroller.contentHeight <= scroller.height + 1, "nothing to scroll for RustDesk");
            findChild(dialog.contentItem, "nameField").text = "Workshop 2";
            field(dialog, "host").text = "192.168.1.61";
            shot("add-tiles-rustdesk" + suffix);

            dialog.chooseType("sunshine");
            wait(100);
            compare(fullName.text, "Sunshine / GameStream");
            compare(about.text, I18n.t(bridge.sunshine.note));
            verify(nearly(item(dialog, "host").y, item(dialog, "port").y));
            verify(nearly(item(dialog, "app").y, item(dialog, "resolution").y) && nearly(item(dialog, "app").y, item(dialog, "bitrate").y), "three short fields in one row");
            verify(nearly(item(dialog, "app").width, item(dialog, "resolution").width) && nearly(item(dialog, "app").width, item(dialog, "bitrate").width));
            verify(item(dialog, "app").y > item(dialog, "host").y + 20);
            verify(item(dialog, "pointer").y > item(dialog, "app").y + 20);
            verify(nearly(item(dialog, "pointer").width, findChild(dialog.contentItem, "fieldGrid").width));
            // The pointer is a choice: two options to click, the usual one chosen, no box to type in.
            var picture = findChild(dialog.contentItem, "field-pointer-picture");
            var own = findChild(dialog.contentItem, "field-pointer-own");
            verify(picture !== null && own !== null && picture.visible && own.visible);
            verify(!field(dialog, "pointer").visible, "no text box for a choice");
            compare(picture.text, I18n.t("In the picture (host's pointer)"));
            compare(own.text, I18n.t("Drawn here (no lag; arrow only)"));
            verify(data.tag === "en" ? own.text.indexOf("Drawn") === 0 : own.text.indexOf("Drawn") === -1, "the options are translated");
            verify(picture.chosen && !own.chosen);
            verify(own.width >= own.contentItem.childrenRect.width - 1);
            mouseClick(own);
            verify(own.chosen && !picture.chosen);
            findChild(dialog.contentItem, "nameField").text = "Living room";
            field(dialog, "host").text = "192.168.1.70";
            wait(50);
            // The buttons stay in sight, whether or not this form scrolls.
            var bottom = buttons.mapToItem(dialog.contentItem, 0, buttons.height).y;
            verify(bottom <= dialog.contentItem.height + 1);
            verify(dialog.y + dialog.height <= window.height);
            shot("add-tiles-sunshine" + suffix);
            // What was chosen goes with the form, as text like any other field.
            var given = null;
            var took = function (connectorId, hostId, sourceId, name, values) { given = values; };
            dialog.sourceSubmitted.connect(took);
            findChild(dialog.contentItem, "submitButton").clicked();
            dialog.sourceSubmitted.disconnect(took);
            verify(given !== null);
            compare(given.pointer, "own");
            compare(given.resolution, "1920x1080");
            compare(given.host, "192.168.1.70");
            // Its type has a set-up: the PIN step fits the dialog as it is now.
            verify(dialog.setupShown);
            verify(!buttons.visible);
            shot("add-tiles-setup" + suffix);
            closeAll(dialog);

            // Many types: a longer list, each row as wide as the others.
            var more = [];
            for (var n = 0; n < 5; n++) more.push(Object.assign({}, bridge.acme, { id: "acme-" + n, name: "Acme Viewer " + n + " (test)" }));
            bridge.extra = [bridge.sunshine].concat(more);
            home.refreshOwnComputers();
            dialog.openAdd();
            wait(100);
            tiles = tilesOf(dialog);
            compare(tiles.length, 9);
            tiles.forEach(function (t, k) {
                verify(nearly(t.width, tiles[0].width) && nearly(t.x, tiles[0].x));
                if (k > 0) verify(t.y > tiles[k - 1].y);
                verify(t.x >= 0 && t.x + t.width <= findChild(dialog.contentItem, "typeOptions").width + 1, "no tile leaves the list");
            });
            compare(tiles[4].text, "Acme Viewer 0");
            shot("add-tiles-many" + suffix);
            closeAll(dialog);
            bridge.extra = [bridge.sunshine];
            home.refreshOwnComputers();

            // Edit on Home: the stored choice is the one chosen.
            bridge.ownSources = bridge.ownSources.map(function (s) {
                return s.id === "den-sun-2" ? Object.assign({}, s, { values: { host: "192.168.1.50", pointer: "own" } }) : s;
            });
            home.refreshOwnComputers();
            home.editSource({ address: "source:den-sun-2", name: "Den PC (Sunshine)", type: "sunshine", sourceId: "den-sun-2" });
            wait(100);
            verify(dialog.editing);
            verify(findChild(dialog.contentItem, "field-pointer-own").chosen);
            verify(!findChild(dialog.contentItem, "field-pointer-picture").chosen);
            shot("edit-tiles-sunshine" + suffix);
            closeAll(dialog);

            // A narrow window: every field has its own row, the types are chips in rows above the form.
            var widthBefore = window.width;
            window.width = 420;
            dialog.openAdd();
            wait(100);
            verify(dialog.width <= 420 - 40 + 1);
            verify(!dialog.wide);
            verify(!dialog.twoPane);
            verify(!findChild(dialog.contentItem, "typeRail").visible);
            dialog.chooseType("rustdesk");
            wait(100);
            verify(item(dialog, "port").y > item(dialog, "host").y + 20, "one field to a row");
            tiles = tilesOf(dialog);
            compare(tiles.length, 4);
            var chips = findChild(dialog.contentItem, "typeChips");
            verify(chips.visible);
            tiles.forEach(function (t) {
                verify(t.height <= 40, "a chip is low");
                verify(t.width >= 120 - 1, "and not narrower than 120");
                verify(t.x + t.width <= chips.width + 1);
            });
            verify(nearly(tiles[1].y, tiles[0].y) && tiles[1].x > tiles[0].x, "chips side by side");
            verify(nearly(tiles[1].width, tiles[0].width));
            shot("add-tiles-narrow" + suffix);
            closeAll(dialog);
            window.width = widthBefore;
            wait(100);

            // ---- Server tab ----
            home.peerTab = "server";
            var dashboard = home.dashboard;
            tryCompare(dashboard, "isAdmin", true);
            var serverDialog = findChild(dashboard, "serverAddDialog");
            serverDialog.openDialog();
            wait(150);
            tiles = tilesOf(serverDialog);
            compare(tiles.length, 4);
            verify(serverDialog.twoPane);
            for (i = 1; i < tiles.length; i++) verify(nearly(tiles[i].x, tiles[0].x) && tiles[i].y > tiles[i - 1].y && nearly(tiles[i].width, tiles[0].width));
            compare(findChild(serverDialog.contentItem, "typeNote").text,
                I18n.t("The server connects to the Lunaris app on that computer (remote control turned on). Its address and password are shown under \"This computer\" there."));
            shot("server-add-tiles-lunaris" + suffix);
            serverDialog.chooseType("rustdesk");
            wait(100);
            // The name and the one computer to go through share a row.
            verify(findChild(serverDialog.contentItem, "nameGrid").paired);
            var through = findChild(serverDialog.contentItem, "throughOption-" + bridge.hosts[0].id);
            verify(through !== null && through.visible);
            verify(nearly(through.mapToItem(serverDialog.contentItem, 0, 0).y,
                          findChild(serverDialog.contentItem, "nameField").mapToItem(serverDialog.contentItem, 0, 0).y));
            shot("server-add-tiles-rustdesk" + suffix);
            serverDialog.chooseType("sunshine");
            wait(100);
            verify(findChild(serverDialog.contentItem, "field-pointer-picture").chosen);
            findChild(serverDialog.contentItem, "nameField").text = "Living room";
            field(serverDialog, "host").text = "192.168.1.70";
            mouseClick(findChild(serverDialog.contentItem, "field-pointer-own"));
            wait(50);
            bottom = findChild(serverDialog.contentItem, "buttonRow").mapToItem(serverDialog.contentItem, 0, 36).y;
            verify(bottom <= serverDialog.contentItem.height + 1);
            shot("server-add-tiles-sunshine" + suffix);
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            var posted = bridge.calls.filter(function (c) { return c.method === "POST"; })[0];
            verify(posted !== undefined);
            compare(JSON.parse(posted.body).values.pointer, "own");
            verify(serverDialog.setupShown);
            shot("server-add-tiles-setup" + suffix);
            closeAll(serverDialog);

            // Edit on the server: nothing is chosen (the server never says what is stored), and nothing is sent for it.
            var sun = dashboard.sourcesList.filter(function (s) { return s.id === "den-sun"; })[0];
            dashboard.editSource(sun);
            wait(150);
            verify(!findChild(serverDialog.contentItem, "field-pointer-picture").chosen);
            verify(!findChild(serverDialog.contentItem, "field-pointer-own").chosen);
            bridge.calls = [];
            findChild(serverDialog.contentItem, "submitButton").clicked();
            wait(250);
            var put = bridge.calls.filter(function (c) { return c.method === "PUT"; })[0];
            verify(put !== undefined);
            verify(JSON.parse(put.body).values.pointer === undefined);
            closeAll(serverDialog);
        }

        // On a usual window (1280 x 800) no form of a type scrolls: the types
        // beside the form, lone fields two to a row, RDP's security a drop-down.
        function test_dialog_fits_data() {
            return [{ tag: "en", theme: "dark" }, { tag: "vi", theme: "light" }];
        }

        function test_dialog_fits(data) {
            var sizeBefore = [window.width, window.height];
            window.width = 1280;
            window.height = 800;
            start(data.tag, function (stub) { stub.extra = [stub.sunshine, stub.rdp, stub.vnc]; }, data.theme);
            var suffix = data.theme === "light" ? "-light" : "";
            var dialog = findChild(test.home, "addComputerDialog");
            findChild(test.home, "addComputerButton").clicked();
            wait(100);
            verify(dialog.twoPane);
            compare(dialog.width, 820);
            compare(tilesOf(dialog).length, 6);
            var scroller = findChild(dialog.contentItem, "formScroller");
            var item = function (key) { return findChild(dialog.contentItem, "fieldItem-" + key); };
            var fits = function (what) {
                verify(scroller.contentHeight <= scroller.height + 1, "nothing to scroll for " + what);
                var rail = findChild(dialog.contentItem, "railScroller");
                verify(rail.contentHeight <= rail.height + 1, "the list of types does not scroll");
                var buttons = findChild(dialog.contentItem, "buttonRow");
                verify(buttons.mapToItem(dialog.contentItem, 0, buttons.height).y <= dialog.contentItem.height + 1);
                verify(dialog.y + dialog.height <= window.height);
            };
            fits("Lunaris");

            dialog.chooseType("rdp");
            wait(100);
            fits("RDP");
            // Host and port; user name and password; domain and security; the resolution alone.
            verify(nearly(item("host").y, item("port").y));
            verify(nearly(item("username").y, item("password").y) && nearly(item("username").width, item("password").width));
            verify(nearly(item("domain").y, item("security").y) && nearly(item("domain").width, item("security").width));
            verify(item("username").y > item("host").y + 20 && item("domain").y > item("username").y + 20);
            verify(item("resolution").y > item("domain").y + 20);
            verify(nearly(item("resolution").width, findChild(dialog.contentItem, "fieldGrid").width));
            // Security: a drop-down with the usual one, translated; no options to click.
            var combo = findChild(dialog.contentItem, "fieldCombo-security");
            verify(combo.visible);
            verify(findChild(dialog.contentItem, "choice-security").visible);
            compare(combo.displayText, I18n.t("NLA (recommended)"));
            verify(findChild(dialog.contentItem, "field-security-nla") === null);
            compare(field(dialog, "resolution").text, "auto");
            findChild(dialog.contentItem, "nameField").text = "Office desk";
            field(dialog, "host").text = "192.168.1.80";
            field(dialog, "username").text = "anna";
            combo.currentIndex = 1;
            combo.activated(1);
            shot("add-fits-rdp" + suffix);
            var given = null;
            var took = function (connectorId, hostId, sourceId, name, values) { given = values; };
            dialog.sourceSubmitted.connect(took);
            findChild(dialog.contentItem, "submitButton").clicked();
            dialog.sourceSubmitted.disconnect(took);
            verify(given !== null);
            compare(given.security, "tls");
            compare(given.username, "anna");
            closeAll(dialog);

            dialog.openAdd();
            dialog.chooseType("vnc");
            wait(100);
            fits("VNC");
            // The password and the mode share a row; the mode is two options to click, side by side.
            verify(nearly(item("password").y, item("view_only").y));
            var control = findChild(dialog.contentItem, "field-view_only-off");
            var viewOnly = findChild(dialog.contentItem, "field-view_only-on");
            verify(control.visible && viewOnly.visible && control.chosen);
            verify(nearly(control.y, viewOnly.y) && viewOnly.x > control.x);
            verify(!findChild(dialog.contentItem, "fieldCombo-view_only").visible);
            shot("add-fits-vnc" + suffix);

            dialog.chooseType("sunshine");
            wait(100);
            fits("Sunshine");
            shot("add-fits-sunshine" + suffix);
            dialog.chooseType("rustdesk");
            wait(100);
            fits("RustDesk");
            dialog.chooseType("lunaris");
            wait(100);
            fits("Lunaris");
            closeAll(dialog);

            window.width = sizeBefore[0];
            window.height = sizeBefore[1];
            wait(50);
        }

        function test_plain_text() {
            var evil = "<b>x</b><img src=\"http://127.0.0.1:9/x.png\">";
            start("en", function (stub) {
                var noted = Object.assign({}, stub.sunshine, { id: "evil-view", name: evil, note: evil });
                stub.extra = [stub.sunshine, noted];
                stub.ownSources = [
                    { id: "evil-1", name: evil, connector: "evil-view", connector_name: evil, values: { host: evil }, group: "g-0000000000e1" },
                    { id: "evil-2", name: evil + "2", connector: "rustdesk", connector_name: stub.rustdesk.name, values: { host: "192.168.1.50" }, group: "g-0000000000e1" },
                    { id: "evil-3", name: evil, connector: "evil-view", connector_name: evil, values: { host: evil } }
                ];
                stub.stores = Object.assign({}, stub.stores, { peers: JSON.stringify({ peers: [
                    { address: "source:evil-1", name: evil, type: "evil-view", sourceId: "evil-1", favorite: false, lastUsed: 3 },
                    { address: "source:evil-2", name: evil + "2", type: "rustdesk", sourceId: "evil-2", favorite: false, lastUsed: 2 },
                    { address: "source:evil-3", name: evil, type: "evil-view", sourceId: "evil-3", favorite: false, lastUsed: 1 }
                ] }) });
                stub.serverSources = [
                    { host_id: stub.hosts[0].id, host_name: evil, id: "evil-s1", name: evil, connector: "evil-view", connector_name: evil, online: true, group: "g-0000000000e2" },
                    { host_id: stub.hosts[0].id, host_name: evil, id: "evil-s2", name: evil + "2", connector: "rustdesk", connector_name: stub.rustdesk.name, online: true, group: "g-0000000000e2" }
                ];
                stub.hosts = [Object.assign({}, stub.hosts[0], { name: evil }), stub.hosts[1]];
            });
            var home = test.home;

            // Home: a card's name, its type's badge and its chips.
            var single = cardByKey("source::evil-3");
            var group = cardByKey("group:g-0000000000e1");
            verify(single && group);
            shownLiterally(findChild(single, "peerCardName"), evil);
            shownLiterally(findChild(group, "peerCardName"), evil);
            var chipTexts = [];
            all(findChild(group, "peerChips"), "methodChip").forEach(function (chip) {
                var texts = chip.contentItem.children;
                for (var i = 0; i < texts.length; i++) if (texts[i].text !== undefined) chipTexts.push(texts[i]);
            });
            compare(chipTexts.length, 2);
            chipTexts.forEach(function (t) { compare(t.textFormat, Text.PlainText); });
            verify(chipTexts.some(function (t) { return t.text === evil; }));
            // The line at the foot of the window (what a host said of a session).
            home.setStatus(evil, true);
            shot("plain-text-home");
            home.setStatus("", false);

            // The dialog: a type's name and note, the title, the set-up's message and its error.
            var dialog = findChild(home, "addComputerDialog");
            findChild(home, "addComputerButton").clicked();
            var option = findChild(dialog.contentItem, "typeOption-evil-view");
            compare(option.text, evil);
            compare(option.fullName, evil);
            var optionTexts = [];
            (function collect(item) {
                if (item.text !== undefined && item.textFormat !== undefined) optionTexts.push(item);
                var children = item.children || [];
                for (var i = 0; i < children.length; i++) collect(children[i]);
            })(option.contentItem);
            verify(optionTexts.length >= 2);
            optionTexts.forEach(function (t) { compare(t.textFormat, Text.PlainText); });
            dialog.chooseType("evil-view");
            wait(50);
            shownLiterally(findChild(dialog.contentItem, "typeNote"), evil);
            compare(findChild(dialog.contentItem, "typeFullName").textFormat, Text.PlainText);
            compare(findChild(dialog.contentItem, "typeFullName").text, evil);
            shot("plain-text-add");
            dialog.setupShow("su-x", "", evil, "12<b>34");
            shownLiterally(findChild(dialog.contentItem, "setupMessage"), evil);
            shownLiterally(findChild(dialog.contentItem, "setupCode"), "12<b>34");
            shot("plain-text-setup");
            dialog.setupFailed(evil);
            shownLiterally(findChild(dialog.contentItem, "setupErrorText"), evil);
            dialog.setupFinished();
            dialog.error = evil;
            shownLiterally(findChild(dialog.contentItem, "errorText"), evil);
            closeAll(dialog);
            home.editSource(peer("source:evil-3"));
            wait(50);
            shownLiterally(findChild(dialog.contentItem, "dialogTitle"), I18n.t("Edit %1").arg(evil));
            closeAll(dialog);
            home.addMethodTo(single.cardData);
            wait(50);
            shownLiterally(findChild(dialog.contentItem, "dialogTitle"), I18n.t("Add a connection method to %1").arg(evil));
            shownLiterally(findChild(dialog.contentItem, "methodNote"), I18n.t("It is shown as one more way to connect to %1.").arg(evil));
            closeAll(dialog);

            // The Server tab: the cards' names, and the question before one is removed.
            home.peerTab = "server";
            var dashboard = home.dashboard;
            tryCompare(dashboard, "isAdmin", true);
            tryCompare(dashboard.cards, "length", 3);
            wait(100);
            var titles = all(dashboard, "cardTitle", []);
            verify(titles.length >= 3);
            titles.forEach(function (t) { compare(t.textFormat, Text.PlainText); });
            verify(titles.filter(function (t) { return t.text === evil; }).length >= 2);
            shot("plain-text-server");
            var removeDialog = findChild(dashboard, "removeSourceDialog");
            var card = dashboard.cardByKey("group:g-0000000000e2");
            removeDialog.openFor(card.methods[0].source, card, card.methods[0]);
            var asked = findChild(removeDialog.contentItem, "removeSourceText");
            compare(asked.textFormat, Text.PlainText);
            verify(asked.text.indexOf(evil) !== -1);
            removeDialog.close();
            wait(250);
        }

        // Settings > Security: "Wayland screen capture" (Linux only), in both languages.
        function test_wayland_capture_setting_data() {
            return [{ tag: "en", lang: "en" }, { tag: "vi", lang: "vi" }];
        }
        function test_wayland_capture_setting(data) {
            start(data.lang);
            var home = test.home;
            home.page = "settings";
            var settingsPage = findChild(home, "appSettings");
            settingsPage.section = "security";
            wait(50);
            var card = findChild(settingsPage, "waylandCaptureCard");
            verify(card !== null);
            compare(card.visible, Qt.platform.os === "linux");
            if (Qt.platform.os !== "linux") return;
            compare(card.title, data.lang === "vi" ? "Chụp màn hình trên Wayland" : "Wayland screen capture");
            var auto = findChild(card, "waylandCapture_auto");
            var portal = findChild(card, "waylandCapture_portal");
            verify(auto.checked && !portal.checked, "auto by default");
            if (data.lang === "vi") compare(portal.text, "Hỏi quyền qua portal");
            shot("settings-wayland-capture");
            portal.clicked();
            compare(bridge.waylandCaptureSet, ["portal"]);
            tryCompare(portal, "checked", true);
            verify(!auto.checked);
            compare(home.hostError, "");
            auto.clicked();
            compare(bridge.waylandCaptureSet, ["portal", "auto"]);
            tryCompare(auto, "checked", true);
        }
    }
}
