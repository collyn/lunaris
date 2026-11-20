import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Shapes
import "Cards.js" as Cards

Rectangle {
    id: dashboardRoot
    anchors.fill: parent
    color: Theme.bg

    // Exposed properties/signals
    property string token: ""
    property string username: ""
    property string serverUrl: ""

    signal startSessionRequested(string server, string token, string hostId, string hostName, int appId, string res, int fps, string codec, int bitrate, int queueLimit, bool disableCuda, string renderBackend, string inputProtocol, string encoder, string displayId, string sourceId)

    // Internal UI state properties
    property bool isLoggedIn: false
    property string authError: ""
    property bool authLoading: false
    // Login screen shows "connect directly by IP" instead of the server sign-in.
    property bool directMode: false
    // Shown inside the Home "Server" tab: Home has its own address box and
    // brand, and the window isn't dragged from here.
    property bool embedded: false
    // main.qml passes the host's encoders and displays to the settings dialog.
    property alias settingsModal: settingsModal
    clip: embedded

    // Direct connection (no server): the agent's address ("host" or
    // "host:port", default port 8091) and its access password.
    function connectDirect(address, password) {
        var addr = address.trim().replace(/^wss?:\/\//, "").replace(/^https?:\/\//, "").replace(/\/.*$/, "");
        if (addr.length === 0 || password.trim().length === 0) {
            dashboardRoot.authError = "Enter the computer's address and its access password";
            return;
        }
        // Add the default port unless one is given ("[v6]:port" or "host:port").
        var hasPort = addr.charAt(0) === "[" ? addr.indexOf("]:") !== -1 : addr.indexOf(":") !== -1;
        if (!hasPort) addr = addr + ":8091";
        dashboardRoot.authError = "";
        bridge.saveDirectTarget(addr, password.trim());
        dashboardRoot.startSessionRequested(
            "ws://" + addr, password.trim(), "direct", addr, 0,
            dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec,
            dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda,
            dashboardRoot.configRenderBackend, dashboardRoot.configInputProtocol,
            dashboardRoot.configEncoder, dashboardRoot.configDisplay, "");
    }


    // Remote control of `host` in the browser: the web console signed in as
    // this app's user (session in the URL fragment, never sent to the
    // server) connecting straight to the computer.
    function openHostInBrowser(host) {
        Qt.openUrlExternally(String(dashboardRoot.serverUrl).replace(/\/+$/, "")
            + "/#session=" + encodeURIComponent(dashboardRoot.token)
            + "&user=" + encodeURIComponent(dashboardRoot.username)
            + "&connect=" + encodeURIComponent(host.id)
            + "&name=" + encodeURIComponent(host.name));
    }

    // Connect straight to a computer's desktop (app 0, the only one an
    // agent offers) with the current stream settings.
    function connectHost(host) {
        dashboardRoot.sessionHost = host.id;
        dashboardRoot.sessionSource = dashboardRoot.sourceFor(host);
        dashboardRoot.startSessionRequested(
            dashboardRoot.serverUrl, dashboardRoot.token, host.id, host.name, 0,
            dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec,
            dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda,
            dashboardRoot.configRenderBackend, dashboardRoot.configInputProtocol,
            dashboardRoot.configEncoder, dashboardRoot.displayFor(host.id), dashboardRoot.sourceFor(host));
    }

    // ---- Computers of other types, reached through a Lunaris computer ----
    // (its sources: GET /api/sources). Each is a card of its own.
    // [{ host_id, host_name, id, name, connector, connector_name, online }]
    property var sourcesList: []
    // What can be added through which computer (GET /api/connectors):
    // [{ host_id, host_name, connectors: [{ id, name, fields }] }]
    property var connectorHosts: []
    // The types to offer in "Add computer": every connector some computer offers.
    readonly property var connectorTypes: {
        var types = [];
        var seen = {};
        for (var i = 0; i < connectorHosts.length; i++) {
            var offered = connectorHosts[i].connectors || [];
            for (var k = 0; k < offered.length; k++) {
                if (seen[offered[k].id]) continue;
                seen[offered[k].id] = true;
                types.push(offered[k]);
            }
        }
        return types;
    }
    // The grid: one card for each computer (Cards.js). The Lunaris
    // computers, each with the sources that are that computer, then the
    // other sources, those of one group on one card. A card has its ways
    // to connect: [{ key, label, online, host | source }].
    readonly property var cards: Cards.buildCards(hostsList, sourcesList, function (s) {
        return Cards.typeShortName(s.connector, I18n.t(String(s.connector_name || s.connector || "")));
    }, "Lunaris")
    // Finding a computer (ComputerFinder.qml; Cards.js): the search, the
    // types among the cards, the one filtered by (ViewPrefs, shared with
    // Home's list) and the cards shown, in the grid or as a list.
    property string searchText: ""
    readonly property var cardTypes: Cards.typesOf(cards, function (id, source) {
        return id === "lunaris" ? "Lunaris" : Cards.typeShortName(id, I18n.t(String(source ? (source.connector_name || id) : id)));
    })
    readonly property string cardType: Cards.effectiveType(cardTypes, ViewPrefs.computerType)
    readonly property var shownCards: Cards.filterCards(cards, searchText, cardType)
    readonly property bool listView: ViewPrefs.computerView === "list"
    function clearFinding() {
        dashboardRoot.searchText = "";
        ViewPrefs.computerType = "";
    }
    // The way used last for a card: { "<card key>": "<method key>" }. The
    // owner (Home) keeps it between runs: it sets this when it starts and
    // saves it on `cardMethodChosen`.
    property var cardMethods: ({})
    signal cardMethodChosen(string card, string method)
    function chooseMethod(card, methodKey) {
        if (dashboardRoot.cardMethods[card.key] === methodKey) return;
        dashboardRoot.cardMethods = Cards.remembered(dashboardRoot.cardMethods, card.key, methodKey);
        dashboardRoot.cardMethodChosen(card.key, methodKey);
    }
    // The way a card connects with now.
    function methodOf(card) { return Cards.currentMethod(card.methods, dashboardRoot.cardMethods[card.key]); }
    // What the pages take for a way to connect: the Lunaris computer as it
    // is listed, or the source with `isSource`.
    function methodItem(method) {
        return method.source ? Object.assign({ isSource: true }, method.source) : method.host;
    }
    function cardByKey(key) {
        for (var i = 0; i < cards.length; i++) if (cards[i].key === key) return cards[i];
        return null;
    }
    // Connects with one way of a card, and remembers it for the card.
    function connectMethod(card, method) {
        if (!method || !method.online) return;
        chooseMethod(card, method.key);
        if (method.source) connectSource(method.source);
        else connectHost(method.host);
    }
    // Whether a way to connect can be added to a card: it has a group or
    // can be given one (a computer that is only an address yet cannot).
    function takesMethods(card) {
        return card.host ? card.group !== "" : true;
    }
    // "Edit" of a source: the cards it can be said to be the same computer
    // as, and the one it is on now ("" when it is a card of its own).
    function sameOptionsFor(source) {
        var options = [];
        var now = "";
        var key = Cards.sourceKey(source);
        for (var i = 0; i < cards.length; i++) {
            var card = cards[i];
            var mine = false;
            for (var k = 0; k < card.methods.length; k++) if (card.methods[k].key === key) mine = true;
            if (mine && card.methods.length === 1) continue;
            if (!takesMethods(card)) continue;
            if (mine) now = card.key;
            options.push({ value: card.key, label: card.title, group: card.group });
        }
        return { options: options, now: now };
    }
    function editSource(source) {
        var same = sameOptionsFor(source);
        // What the server sends of its fields (administrators, servers from
        // this version on); left out, the form starts empty and keeps what
        // is left empty.
        var values = source.values !== null && typeof source.values === "object" ? source.values : undefined;
        addHostDialog.openEdit(Object.assign({ isSource: true }, source), values, same.options, same.now);
    }
    // The group that was said with a form (AddComputerDialog), as the
    // server takes it. "@<card key>" is a card that has no group yet (one
    // source of its own): it is given a new one first. then(group, error).
    function withGroup(said, then) {
        if (String(said).charAt(0) !== "@") { then(said, ""); return; }
        var card = cardByKey(String(said).slice(1));
        if (!card || card.methods.length !== 1 || !card.methods[0].source) {
            then("*", I18n.t("That computer is not there any more"));
            return;
        }
        var first = card.methods[0].source;
        var group = Cards.newGroup();
        sourcesApi.call("PUT", "/api/hosts/" + encodeURIComponent(first.host_id) + "/sources/" + encodeURIComponent(first.id),
            { name: first.name, values: ({}), group: group }, function (ok, data, err, status, text) {
                if (ok) then(group, "");
                else then("*", dashboardRoot.apiError(err, status, text, data));
            });
    }
    // The displays the sources listed in their sessions, by the computer
    // they are reached through: { "<host id>": { "<source id>": [...] } }.
    property var seenSourceDisplays: ({})
    // The computer and the source of the session started last from here.
    property string sessionHost: ""
    property string sessionSource: ""

    ApiClient { id: sourcesApi }
    function refreshSources() {
        // A server from before this answers 404: then there are no such
        // cards and only the Lunaris type to add, and nothing to report.
        sourcesApi.call("GET", "/api/sources", null, function (ok, data) {
            dashboardRoot.sourcesList = ok && data instanceof Array ? data : [];
        });
        sourcesApi.call("GET", "/api/connectors", null, function (ok, data) {
            dashboardRoot.connectorHosts = ok && data instanceof Array ? data : [];
        });
    }

    // The message for a failed call: the server's own text when it sent one
    // ({"error": ...}, or the plain text of a 403), else ApiClient's.
    function apiError(err, status, text, data) {
        if (data && data.error) return I18n.tError(String(data.error));
        var plain = String(text || "").trim();
        if (status === 403 && plain !== "" && plain.length < 200 && plain.charAt(0) !== "{" && plain.charAt(0) !== "<")
            return I18n.t(plain);
        return err;
    }

    function configKey(card) { return card.isSource ? card.host_id + "/source/" + card.id : card.id; }

    // The display chosen in the stream settings is one of the computer they
    // were opened for (see sourceFor).
    function displayFor(key) {
        if (dashboardRoot.activeConfigHost && dashboardRoot.activeConfigHost.id === key) return dashboardRoot.configDisplay;
        // As before for the Lunaris computers among themselves; a source's
        // display is never another computer's.
        return dashboardRoot.activeConfigHost && dashboardRoot.activeConfigHost.isSource ? "default" : dashboardRoot.configDisplay;
    }

    // A session to a computer of another type: a normal session to the
    // Lunaris computer it is reached through, naming it as the source.
    function connectSource(source) {
        dashboardRoot.sessionHost = source.host_id;
        dashboardRoot.sessionSource = source.id;
        var key = configKey(source);
        dashboardRoot.startSessionRequested(
            dashboardRoot.serverUrl, dashboardRoot.token, source.host_id, source.name, 0,
            dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec,
            dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda,
            dashboardRoot.configRenderBackend, dashboardRoot.configInputProtocol,
            dashboardRoot.configEncoder,
            dashboardRoot.activeConfigHost && dashboardRoot.activeConfigHost.id === key ? dashboardRoot.configDisplay : "default",
            source.id);
    }

    // From Home: the displays of the source the session from here shows.
    function sourceDisplaysSeen(list) {
        if (dashboardRoot.sessionHost === "" || dashboardRoot.sessionSource === "") return;
        var all = Object.assign({}, dashboardRoot.seenSourceDisplays);
        var ofHost = Object.assign({}, all[dashboardRoot.sessionHost] || {});
        ofHost[dashboardRoot.sessionSource] = list;
        all[dashboardRoot.sessionHost] = ofHost;
        dashboardRoot.seenSourceDisplays = all;
    }

    // Stream settings of a card (a Lunaris computer or a source).
    function openStreamSettings(card) {
        var key = configKey(card);
        // Only reset to defaults when switching to a different computer
        if (dashboardRoot.activeConfigHost === null || dashboardRoot.activeConfigHost.id !== key) {
            dashboardRoot.configRes = "1920x1080";
            dashboardRoot.configFps = StreamOptions.displayFps(bridge.screenRefreshRate());
            dashboardRoot.configCodec = "auto";
            dashboardRoot.configBitrate = 15000;
            dashboardRoot.configQueueLimit = 256;
            dashboardRoot.configDisableCuda = false;
            dashboardRoot.configRenderBackend = "auto_gpu";
            dashboardRoot.configInputProtocol = "webrtc";
            dashboardRoot.configEncoder = "auto";
            dashboardRoot.configDisplay = "default";
            dashboardRoot.configSource = "";
        }
        dashboardRoot.activeConfigHost = card.isSource ? { id: key, name: card.name, isSource: true } : card;
        settingsModal.hostName = card.name;
        settingsModal.hostAddress = Cards.methodAddress(card.isSource ? { source: card } : { host: card });
        // A source card is that computer and no other; its displays are its own.
        settingsModal.sourceOnly = !!card.isSource;
        settingsModal.sourceDisplays = dashboardRoot.seenSourceDisplays[card.isSource ? card.host_id : card.id] || ({});
        settingsModal.setCurrentSettings(dashboardRoot.configRes, dashboardRoot.configFps, dashboardRoot.configCodec, dashboardRoot.configBitrate, dashboardRoot.configQueueLimit, dashboardRoot.configDisableCuda, dashboardRoot.configInputProtocol, dashboardRoot.configEncoder, dashboardRoot.configDisplay, dashboardRoot.configRenderBackend, card.isSource ? card.id : dashboardRoot.configSource);
        // Fetch encoder/display capabilities from agent
        if (!card.isSource && dashboardRoot.serverUrl && dashboardRoot.token && card.id) {
            bridge.fetchCapabilities(dashboardRoot.serverUrl, dashboardRoot.token, card.id);
        }
        settingsModal.open();
    }

    // The source chosen in the stream settings belongs to the computer they
    // were opened for: another computer shows its own screen.
    function sourceFor(host) {
        return (host && dashboardRoot.activeConfigHost && dashboardRoot.activeConfigHost.id === host.id)
            ? dashboardRoot.configSource : "";
    }

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
    // The screen's refresh rate until another one is picked (see StreamOptions.displayFps).
    property int configFps: StreamOptions.displayFps(bridge.screenRefreshRate())
    property string configCodec: "auto"
    property int configBitrate: 15000
    property int configQueueLimit: 256
    property bool configDisableCuda: false
    property string configRenderBackend: configDisableCuda ? "software" : "auto_gpu"
    property string configInputProtocol: "webrtc"
    property string configEncoder: "auto"
    property string configDisplay: "default"
    // The source of `activeConfigHost` to show; "" = its own screen.
    property string configSource: ""
    property var pendingDeleteHost: null



    // Agent configuration token
    property string agentToken: ""
    property bool agentTokenLoading: false
    property string agentTokenError: ""

    // Local Agent Control state
    property string localAgentHostname: ""
    property bool localAgentRunning: false

    // Signed-in account's role ("admin" shows the administration sections)
    // and the section shown: devices | users | groups | turn.
    property string role: ""
    readonly property bool isAdmin: role === "admin"
    property string section: "devices"
    onIsLoggedInChanged: {
        section = "devices";
        role = "";
        if (isLoggedIn) refreshRole();
    }
    function refreshRole() {
        roleApi.call("GET", "/api/auth/me", null, function (ok, data) {
            dashboardRoot.role = ok && data && data.role ? data.role : "";
            if (!dashboardRoot.isAdmin) dashboardRoot.section = "devices";
        });
    }
    ApiClient { id: roleApi }

    component SectionTab: AbstractButton {
        id: stab
        property string iconName: ""
        property bool current: false
        hoverEnabled: true
        implicitHeight: 34
        implicitWidth: stRow.implicitWidth + 24
        background: Rectangle {
            radius: Theme.radiusSm
            color: stab.current ? Theme.surface2 : (stab.hovered ? Theme.ink(0.04) : "transparent")
            border.color: stab.current ? Theme.border : "transparent"
        }
        contentItem: Item {
            Row {
                id: stRow
                anchors.centerIn: parent
                spacing: 8
                Icon { name: stab.iconName; size: 15; color: stab.current ? Theme.accentFg : Theme.textMuted; anchors.verticalCenter: parent.verticalCenter }
                Text { text: stab.text; color: stab.current ? Theme.textStrong : Theme.textMuted; font.pixelSize: 13; font.weight: Font.Medium; anchors.verticalCenter: parent.verticalCenter }
            }
        }
    }



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
            } else if (!mfaDialog.visible) {
                dashboardRoot.authError = errorMsg;
            }
        }

        // The password was right; the account wants a code from its app.
        function onMfaRequired(srv, user) {
            dashboardRoot.authLoading = false;
            dashboardRoot.authError = "";
            mfaDialog.begin(user);
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
                dashboardRoot.refreshSources();
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
                // Inside Home the address box keeps the focus.
                if (!dashboardRoot.embedded) serverInput.forceActiveFocus();
            }

            // Central glass card (near the top inside Home, where space is short)
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                y: dashboardRoot.embedded ? 16 : Math.max(0, (parent.height - height) / 2)
                width: 420
                height: (dashboardRoot.directMode ? 420 : 480) + (dashboardRoot.authError !== "" ? 24 : 0)
                radius: 20
                color: Theme.surfaceA(0.8)
                border.color: Theme.ink(0.08)
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
                        text: dashboardRoot.directMode ? "Connect directly" : (dashboardRoot.embedded ? "Sign in to a Lunaris server" : "Welcome back")
                        font.pixelSize: 22
                        font.weight: Font.DemiBold
                        color: Theme.textStrong
                        anchors.horizontalCenter: parent.horizontalCenter
                    }

                    Text {
                        text: dashboardRoot.directMode ? "To a computer on your network or VPN, no server needed" : (dashboardRoot.embedded ? "See and connect to the computers registered on your server" : "Access your remote desktop network")
                        font.pixelSize: 13
                        color: Theme.textMuted
                        anchors.horizontalCenter: parent.horizontalCenter
                    }

                    Item { width: 1; height: 10 } // Spacer

                    // Direct connection inputs
                    Column {
                        visible: dashboardRoot.directMode
                        width: parent.width
                        spacing: 12

                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: I18n.t("Computer address"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: directAddressInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: directAddressInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: bridge.savedDirectAddress()
                                    verticalAlignment: Text.AlignVCenter
                                    selectByMouse: true
                                    activeFocusOnTab: true
                                    KeyNavigation.tab: directPasswordInput
                                    Keys.onReturnPressed: directButton.clicked()
                                    Keys.onEnterPressed: directButton.clicked()
                                    Text {
                                        anchors.fill: parent; verticalAlignment: Text.AlignVCenter
                                        visible: parent.text.length === 0
                                        text: "192.168.1.20 or 192.168.1.20:8091"
                                        color: Theme.textSubtle; font.pixelSize: 13
                                    }
                                }
                            }
                        }

                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: I18n.t("Access password (shown in the agent window)"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: directPasswordInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: directPasswordInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: bridge.savedDirectPassword()
                                    echoMode: TextInput.Password
                                    verticalAlignment: Text.AlignVCenter
                                    selectByMouse: true
                                    activeFocusOnTab: true
                                    KeyNavigation.tab: directAddressInput
                                    Keys.onReturnPressed: directButton.clicked()
                                    Keys.onEnterPressed: directButton.clicked()
                                }
                            }
                        }
                    }

                    // Inputs
                    Column {
                        visible: !dashboardRoot.directMode
                        width: parent.width
                        spacing: 12

                        // Server Address
                        Column {
                            width: parent.width
                            spacing: 6
                            Text { text: I18n.t("Signaling Server Host"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: serverInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: serverInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
                                    text: "http://127.0.0.1:8090"
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
                            Text { text: I18n.t("Username"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: usernameInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: usernameInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
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
                            Text { text: I18n.t("Password"); color: Theme.textMuted; font.pixelSize: 12; font.weight: Font.Medium }
                            Rectangle {
                                width: parent.width; height: 38; color: Theme.surface; radius: Theme.radiusSm; border.color: passwordInput.activeFocus ? Theme.accent : Theme.borderStrong
                                border.width: 1
                                TextInput {
                                    id: passwordInput
                                    anchors.fill: parent; anchors.margins: 10
                                    color: Theme.textStrong; font.pixelSize: 13
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
                        textFormat: Text.PlainText
                        text: dashboardRoot.authError
                        color: "#ef4444"
                        font.pixelSize: 11
                        wrapMode: Text.Wrap
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        visible: text.length > 0
                    }

                    // Direct connect button
                    Button {
                        id: directButton
                        visible: dashboardRoot.directMode
                        width: parent.width
                        onClicked: dashboardRoot.connectDirect(directAddressInput.text, directPasswordInput.text)
                        background: Rectangle {
                            implicitHeight: 40
                            radius: 8
                            color: directButton.hovered ? Theme.accentHover : Theme.accent
                        }
                        contentItem: Item {
                            anchors.fill: parent
                            Text {
                                text: I18n.t("Connect")
                                color: Theme.textOnAccent
                                font.weight: Font.DemiBold
                                font.pixelSize: 13
                                anchors.centerIn: parent
                            }
                        }
                    }

                    // Submit Button
                    Button {
                        id: submitButton
                        visible: !dashboardRoot.directMode
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
                            color: submitButton.hovered ? Theme.accentHover : Theme.accent
                            opacity: dashboardRoot.authLoading ? 0.6 : 1.0
                        }

                        contentItem: Item {
                            anchors.fill: parent
                            Text {
                                text: dashboardRoot.authLoading ? "Signing in..." : "Sign in"
                                color: Theme.textOnAccent
                                font.weight: Font.DemiBold
                                font.pixelSize: 13
                                anchors.centerIn: parent
                            }
                        }
                    }

                    // Switch between server sign-in and direct connection
                    Text {
                        visible: !dashboardRoot.embedded
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: dashboardRoot.directMode ? "Sign in to a Lunaris server instead" : "Connect directly by IP address"
                        color: modeSwitchMa.containsMouse ? Theme.accentFg : Theme.textMuted
                        font.pixelSize: 12
                        font.underline: modeSwitchMa.containsMouse
                        MouseArea {
                            id: modeSwitchMa
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                dashboardRoot.authError = "";
                                dashboardRoot.directMode = !dashboardRoot.directMode;
                                if (dashboardRoot.directMode) directAddressInput.forceActiveFocus(); else serverInput.forceActiveFocus();
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
                color: Theme.bg
                border.color: Theme.ink(0.03)
                border.width: 1

                // Allow dragging the window by holding and dragging the navbar
                DragHandler {
                    target: null
                    enabled: !dashboardRoot.embedded
                    onActiveChanged: {
                        if (active) {
                            window.startSystemMove();
                        }
                    }
                }

                // Brand Title & Tech Pill
                Row {
                    id: brandRow
                    visible: !dashboardRoot.embedded
                    anchors.left: parent.left
                    anchors.leftMargin: 24
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 12

                    Image {
                        source: "icon.png"
                        width: 26; height: 26
                        sourceSize.width: 52; sourceSize.height: 52
                        smooth: true
                        anchors.verticalCenter: parent.verticalCenter
                    }

                    Text {
                        text: I18n.t("Lunaris")
                        font.pixelSize: 15
                        font.weight: Font.DemiBold
                        color: Theme.textStrong
                        anchors.verticalCenter: parent.verticalCenter
                    }

                    Rectangle {
                        width: 52; height: 18; radius: 9
                        color: Theme.surface2
                        border.color: Theme.border
                        anchors.verticalCenter: parent.verticalCenter
                        Text {
                            text: Qt.application.version !== "" ? "v" + Qt.application.version : ""
                            color: Theme.textMuted
                            font.pixelSize: 10
                            font.bold: true
                            anchors.centerIn: parent
                        }
                    }
                }

                // Sections (administrators only): computers, users, groups, TURN
                Row {
                    visible: dashboardRoot.isAdmin
                    anchors.left: brandRow.visible ? brandRow.right : parent.left
                    anchors.leftMargin: 24
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 4
                    SectionTab { text: I18n.t("Computers"); iconName: "monitor"; current: dashboardRoot.section === "devices"; onClicked: dashboardRoot.section = "devices" }
                    SectionTab { text: I18n.t("Users"); iconName: "user"; current: dashboardRoot.section === "users"; onClicked: dashboardRoot.section = "users" }
                    SectionTab { text: I18n.t("Groups"); iconName: "grid"; current: dashboardRoot.section === "groups"; onClicked: dashboardRoot.section = "groups" }
                    SectionTab { text: I18n.t("TURN servers"); iconName: "globe"; current: dashboardRoot.section === "turn"; onClicked: dashboardRoot.section = "turn" }
                }

                // Right Panel
                Row {
                    anchors.right: parent.right
                    anchors.rightMargin: 24
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 20

                    // Theme toggle: system -> light -> dark
                    Button {
                        id: themeBtn
                        visible: !dashboardRoot.embedded
                        anchors.verticalCenter: parent.verticalCenter
                        hoverEnabled: true
                        onClicked: Theme.mode = Theme.mode === "system" ? "light" : (Theme.mode === "light" ? "dark" : "system")
                        ToolTip.visible: hovered
                        ToolTip.text: "Theme: " + Theme.mode
                        background: Rectangle {
                            implicitWidth: 32; implicitHeight: 32; radius: Theme.radiusSm
                            color: themeBtn.hovered ? Theme.surface2 : "transparent"
                            border.color: Theme.border; border.width: 1
                        }
                        contentItem: Text {
                            text: Theme.mode === "light" ? "\u2600" : (Theme.mode === "dark" ? "\u263E" : "A")
                            color: Theme.textMuted
                            font.pixelSize: 14
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                    }

                    // Signed-in user and server
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 1
                        Text {
                            textFormat: Text.PlainText
                            text: dashboardRoot.username
                            font.pixelSize: 13
                            font.weight: Font.DemiBold
                            color: Theme.textStrong
                            anchors.right: parent.right
                        }
                        Text {
                            textFormat: Text.PlainText
                            text: dashboardRoot.serverUrl
                            font.pixelSize: 11
                            color: Theme.textSubtle
                            anchors.right: parent.right
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
                            border.color: logoutBtn.hovered ? "#ef4444" : Theme.ink(0.08)
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
                                    color: "transparent"; border.color: logoutBtn.hovered ? "#ef4444" : Theme.text; border.width: 1.5
                                }
                                Rectangle {
                                    x: 10; y: 3.5; width: 2; height: 9; color: Theme.bg
                                }
                                Rectangle {
                                    x: 6; y: 7.25; width: 8; height: 1.5; color: logoutBtn.hovered ? "#ef4444" : Theme.text
                                }
                                Rectangle {
                                    x: 10; y: 4.5; width: 1.5; height: 4.5; rotation: 45; color: logoutBtn.hovered ? "#ef4444" : Theme.text; antialiasing: true
                                }
                                Rectangle {
                                    x: 10; y: 7; width: 1.5; height: 4.5; rotation: -45; color: logoutBtn.hovered ? "#ef4444" : Theme.text; antialiasing: true
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
                anchors.topMargin: 64 + (dashboardRoot.embedded ? 20 : 32) // below the navbar
                anchors.bottom: parent.bottom
                anchors.margins: dashboardRoot.embedded ? 20 : 40

                Loader {
                    anchors.fill: parent
                    sourceComponent: dashboardRoot.section === "users" ? adminUsersComponent
                        : dashboardRoot.section === "groups" ? adminGroupsComponent
                        : dashboardRoot.section === "turn" ? adminTurnComponent
                        : dashboardRoot.selectedHost !== null ? appDirectoryComponent
                        : hostsGridComponent
                }
            }
        }
    }

    Component { id: adminUsersComponent; AdminUsers { anchors.fill: parent; currentUser: dashboardRoot.username } }
    Component { id: adminGroupsComponent; AdminGroups { anchors.fill: parent } }
    Component { id: adminTurnComponent; AdminTurnServers { anchors.fill: parent } }

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
                        text: I18n.t("Device Directory")
                        font.pixelSize: 24
                        font.bold: true
                        color: Theme.textStrong
                    }
                    Text {
                        text: I18n.t("Manage and connect to active remote agent streams")
                        font.pixelSize: 13
                        color: Theme.textSubtle
                    }
                }

                Row {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 12

                    // Add a computer by its address (the server connects to it)
                    PrimaryButton {
                        visible: dashboardRoot.isAdmin
                        text: I18n.t("Add computer")
                        anchors.verticalCenter: parent.verticalCenter
                        onClicked: addHostDialog.openDialog()
                    }

                    // Sync Devices Button
                    Button {
                        id: syncBtn
                        onClicked: {
                            dashboardRoot.hostsLoading = true;
                            bridge.fetchHosts();
                        }
                        background: Rectangle {
                            implicitWidth: 120; implicitHeight: 38; radius: 8
                            color: syncBtn.hovered ? Theme.ink(0.05) : "transparent"
                            border.color: syncBtn.hovered ? Theme.accentFg : Theme.ink(0.15)
                            border.width: 1
                        }
                        contentItem: Text {
                            text: I18n.t("Sync Devices")
                            color: Theme.text
                            font.bold: true
                            font.pixelSize: 13
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                    }


                }
            }

            // Search, the type of computer, how many, grid or list.
            ComputerFinder {
                id: serverFinder
                objectName: "serverFinder"
                visible: dashboardRoot.cards.length > 0
                anchors.top: sectionHeader.bottom
                anchors.topMargin: visible ? 24 : 0
                anchors.left: parent.left
                anchors.right: parent.right
                height: visible ? implicitHeight : 0
                searchText: dashboardRoot.searchText
                types: dashboardRoot.cardTypes
                typeFilter: dashboardRoot.cardType
                total: dashboardRoot.cards.length
                shown: dashboardRoot.shownCards.length
                view: ViewPrefs.computerView
                onSearchEdited: (text) => dashboardRoot.searchText = text
                onTypeChosen: (id) => ViewPrefs.computerType = id
                onViewChosen: (view) => ViewPrefs.computerView = view
            }

            // Hosts Grid (or, as a list, a grid of one column of rows)
            ScrollView {
                anchors.top: serverFinder.bottom
                anchors.bottom: parent.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.topMargin: serverFinder.visible ? 10 : 32
                clip: true

                GridView {
                    id: hostsGrid
                    objectName: "serverGrid"
                    anchors.fill: parent
                    topMargin: 8
                    cellWidth: dashboardRoot.listView ? Math.max(0, width - 12) : 330
                    cellHeight: dashboardRoot.listView ? 66 : 190
                    model: dashboardRoot.shownCards.length > 0 ? dashboardRoot.shownCards : 0

                    delegate: Item {
                        id: hostDelegate
                        width: dashboardRoot.listView ? hostsGrid.cellWidth : 290
                        height: dashboardRoot.listView ? 58 : 180

                        objectName: "serverCard"
                        property bool isHostHovered: cardMouseArea.containsMouse || webBtn.hovered || gearBtn.hovered || editBtn.hovered || deleteBtn.hovered || connectBtn.hovered || chooseBtn.hovered || addMethodBtn.hovered
                        // The computer (Cards.js) and the way to connect that
                        // is chosen for it: everything on the card acts on it.
                        readonly property var cardData: modelData
                        readonly property bool multi: cardData.methods.length > 1
                        readonly property var method: dashboardRoot.methodOf(cardData)
                        // That way as the pages take it: the Lunaris computer,
                        // or a source (a computer of another type, shown
                        // through a Lunaris computer: online when that is).
                        readonly property var item: dashboardRoot.methodItem(method)
                        readonly property bool isSource: !!method.source
                        // Where it is (its IP addresses, or a source's host): "" when not known.
                        readonly property string address: Cards.methodAddress(method)
                        readonly property string status: isSource ? (item.online ? "Online" : "Offline") : item.status
                        function connect() { dashboardRoot.connectMethod(cardData, method); }
                        // Which of the card's buttons it has (the grid's card and the list's row alike).
                        readonly property bool hasWeb: status === "Online" && !isSource
                        readonly property bool hasFiles: (status === "Online" || status === "Busy")
                            && !(isSource && item.capabilities && item.capabilities.files === false)
                        readonly property bool hasSettings: status === "Online" || status === "Busy"
                        readonly property bool hasEdit: isSource && dashboardRoot.isAdmin
                        readonly property bool hasAddMethod: dashboardRoot.isAdmin && !multi && dashboardRoot.takesMethods(cardData)
                        readonly property bool hasRemove: !isSource || dashboardRoot.isAdmin

                        // Static background click handler to avoid hover-translation flicker
                        MouseArea {
                            id: cardMouseArea
                            width: dashboardRoot.listView ? hostDelegate.width : 270
                            height: dashboardRoot.listView ? hostDelegate.height : 160
                            hoverEnabled: true
                            onClicked: {
                                if (hostDelegate.status === "Online") hostDelegate.connect();
                            }
                        }

                        // Device card (translates up 4px smoothly on hover based on parent static MouseArea)
                        Rectangle {
                            id: card
                            visible: !dashboardRoot.listView
                            width: 310
                            height: 170
                            radius: 14
                            color: Theme.surface
                            border.color: isHostHovered ? Theme.borderStrong : Theme.border
                            border.width: 1

                            Behavior on border.color { ColorAnimation { duration: 150 } }

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
                                        objectName: "cardTitle"
                                        textFormat: Text.PlainText
                                        text: hostDelegate.cardData.title
                                        font.pixelSize: 18
                                        font.bold: true
                                        color: Theme.textStrong
                                        elide: Text.ElideRight
                                        width: parent.width - (typeBadge.visible ? typeBadge.width + 8 : 0)
                                    }
                                    Text {
                                        visible: !hostDelegate.isSource && !hostDelegate.multi
                                        textFormat: Text.PlainText
                                        text: hostDelegate.isSource ? "" : "ID: " + String(hostDelegate.item.id).substring(0, 8) + "..."
                                        font.pixelSize: 11
                                        font.family: "monospace"
                                        color: Theme.textSubtle
                                    }
                                    // Which Lunaris computer shows it
                                    Text {
                                        visible: hostDelegate.isSource && !hostDelegate.multi
                                        width: parent.width
                                        textFormat: Text.PlainText
                                        text: hostDelegate.isSource ? I18n.t("via %1").arg(hostDelegate.item.host_name) : ""
                                        font.pixelSize: 11
                                        color: Theme.textSubtle
                                        elide: Text.ElideRight
                                    }
                                    // The ways to connect, when there is more
                                    // than one: a click chooses the one the
                                    // card acts on (one row; all of them are
                                    // in the list beside Connect).
                                    MethodChips {
                                        objectName: "cardChips"
                                        visible: hostDelegate.multi
                                        width: parent.width
                                        height: 20
                                        clip: true
                                        oneRow: true
                                        z: 10
                                        methods: hostDelegate.multi ? hostDelegate.cardData.methods : []
                                        current: hostDelegate.method.key
                                        onChosen: (key) => dashboardRoot.chooseMethod(hostDelegate.cardData, key)
                                    }
                                }

                                TypeBadge {
                                    id: typeBadge
                                    visible: hostDelegate.isSource && !hostDelegate.multi && shortName !== ""
                                    anchors.right: parent.right
                                    anchors.top: parent.top
                                    anchors.topMargin: 3
                                    typeName: hostDelegate.isSource ? String(hostDelegate.item.connector_name || hostDelegate.item.connector || "") : ""
                                    maxWidth: 120
                                }

                                // Divider top
                                Rectangle {
                                    width: parent.width; height: 1
                                    anchors.top: hostInfo.bottom
                                    anchors.topMargin: 8
                                    color: Theme.ink(0.03)
                                }

                                // Detail rows inside standard border-bracket body
                                Column {
                                    anchors.left: parent.left
                                    anchors.right: parent.right
                                    anchors.bottom: cardFooter.top
                                    anchors.bottomMargin: 10
                                    spacing: 6

                                    // (A source chosen on a card with several ways
                                    // says its address beside "via" below.)
                                    Row {
                                        visible: !hostDelegate.isSource || (hostDelegate.address !== "" && !hostDelegate.multi)
                                        width: parent.width
                                        Text { text: I18n.t("IP Address"); font.pixelSize: 11; color: Theme.textSubtle; width: 80 }
                                        Text {
                                            objectName: "cardAddress"
                                            width: parent.width - 80
                                            textFormat: Text.PlainText
                                            text: hostDelegate.address !== "" ? hostDelegate.address
                                                : (hostDelegate.isSource ? "" : I18n.t("Connected via Agent"))
                                            font.pixelSize: 11; font.bold: true; font.family: "monospace"; color: Theme.text
                                            elide: Text.ElideRight
                                        }
                                    }

                                    // (In a card with several ways the one chosen
                                    // is a source: through which computer.)
                                    Row {
                                        visible: hostDelegate.isSource && hostDelegate.multi
                                        width: parent.width
                                        Text {
                                            width: parent.width
                                            textFormat: Text.PlainText
                                            text: hostDelegate.isSource ? I18n.t("via %1").arg(hostDelegate.item.host_name)
                                                + (hostDelegate.address !== "" ? "  ·  " + hostDelegate.address : "") : ""
                                            font.pixelSize: 11
                                            color: Theme.textSubtle
                                            elide: Text.ElideRight
                                        }
                                    }

                                    Text {
                                        visible: !hostDelegate.isSource && !!hostDelegate.item.link_error
                                        width: parent.width
                                        textFormat: Text.PlainText
                                        text: !hostDelegate.isSource && hostDelegate.item.link_error ? I18n.t(hostDelegate.item.link_error) : ""
                                        font.pixelSize: 11
                                        color: Theme.danger
                                        elide: Text.ElideRight
                                    }

                                    Row {
                                        width: parent.width
                                        Text { text: I18n.t("Status"); font.pixelSize: 11; color: Theme.textSubtle; width: 80 }
                                        Row {
                                            spacing: 6
                                            Rectangle {
                                                width: 8; height: 8; radius: 4
                                                anchors.verticalCenter: parent.verticalCenter
                                                color: hostDelegate.status === "Online" ? "#22c55e" : (hostDelegate.status === "Busy" ? "#f59e0b" : "#ef4444")
                                            }
                                            Text {
                                                text: I18n.t(hostDelegate.status)
                                                font.pixelSize: 11
                                                font.bold: true
                                                color: hostDelegate.status === "Online" ? "#22c55e" : (hostDelegate.status === "Busy" ? "#f59e0b" : "#ef4444")
                                            }
                                        }
                                    }
                                }

                                // Primary action (matches the web client's "Connect")
                                Button {
                                    id: connectBtn
                                    visible: hostDelegate.status === "Online"
                                    anchors.left: parent.left
                                    anchors.bottom: parent.bottom
                                    z: 10
                                    onClicked: hostDelegate.connect()
                                    background: Rectangle {
                                        implicitWidth: Math.max(84, connectLabel.implicitWidth + 32); implicitHeight: 32; radius: Theme.radiusSm
                                        color: connectBtn.hovered ? Theme.accentHover : Theme.accent
                                    }
                                    contentItem: Text {
                                        id: connectLabel
                                        text: I18n.t("Connect")
                                        color: Theme.textOnAccent
                                        font.pixelSize: 13
                                        font.weight: Font.DemiBold
                                        horizontalAlignment: Text.AlignHCenter
                                        verticalAlignment: Text.AlignVCenter
                                    }
                                }

                                // The other ways to connect, and adding one:
                                // a list that opens beside Connect.
                                Button {
                                    id: chooseBtn
                                    objectName: "cardChooseButton"
                                    visible: hostDelegate.multi
                                    anchors.left: connectBtn.visible ? connectBtn.right : parent.left
                                    anchors.leftMargin: connectBtn.visible ? 1 : 0
                                    anchors.bottom: parent.bottom
                                    z: 10
                                    hoverEnabled: true
                                    ToolTip.visible: hovered
                                    ToolTip.delay: 400
                                    ToolTip.text: I18n.t("Choose how to connect")
                                    Accessible.name: I18n.t("Choose how to connect")
                                    onClicked: methodMenu.openFor(hostDelegate.cardData, chooseBtn)
                                    background: Rectangle {
                                        implicitWidth: 24; implicitHeight: 32; radius: Theme.radiusSm
                                        color: chooseBtn.hovered ? Theme.accentHover : Theme.accent
                                    }
                                    contentItem: Item {
                                        implicitWidth: 12; implicitHeight: 16
                                        Icon { anchors.centerIn: parent; name: "chevronDown"; size: 14; color: Theme.textOnAccent }
                                    }
                                }

                                // Card actions footer (bottom right, sits on top of visual card)
                                Row {
                                    id: cardFooter
                                    anchors.right: parent.right
                                    anchors.bottom: parent.bottom
                                    // (One more button for administrators: a little closer.)
                                    spacing: addMethodBtn.visible ? 6 : 8
                                    z: 10

                                    // Remote control in the browser (web console, signed in)
                                    Button {
                                        id: webBtn
                                        visible: hostDelegate.hasWeb
                                        hoverEnabled: true
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: I18n.t("Open in the browser")
                                        Accessible.name: "Open in the browser"
                                        onClicked: dashboardRoot.openHostInBrowser(hostDelegate.item)
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: webBtn.hovered ? Theme.accentA(0.1) : "transparent"
                                            border.color: webBtn.hovered ? Theme.accentFg : Theme.border
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Icon { anchors.centerIn: parent; name: "globe"; size: 17; color: webBtn.hovered ? Theme.accentFg : Theme.text }
                                        }
                                    }

                                    // Files of the computer (own connection, FileManager.qml)
                                    Button {
                                        id: filesBtn
                                        objectName: "cardFilesButton"
                                        // Not for a computer of another type whose connector says it has no files.
                                        visible: hostDelegate.hasFiles
                                        hoverEnabled: true
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: I18n.t("Files")
                                        Accessible.name: "Files"
                                        // A source: its files, through the computer it is reached through.
                                        onClicked: hostDelegate.isSource
                                            ? fileManager.openFor(dashboardRoot.serverUrl, dashboardRoot.token, hostDelegate.item.host_id, hostDelegate.item.name, hostDelegate.item.id)
                                            : fileManager.openFor(dashboardRoot.serverUrl, dashboardRoot.token, hostDelegate.item.id, hostDelegate.item.name)
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: filesBtn.hovered ? Theme.accentA(0.1) : "transparent"
                                            border.color: filesBtn.hovered ? Theme.accentFg : Theme.border
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Icon { anchors.centerIn: parent; name: "folder"; size: 17; color: filesBtn.hovered ? Theme.accentFg : Theme.text }
                                        }
                                    }

                                    // Settings Gear Icon (Sleek custom vector gear)
                                    Button {
                                        id: gearBtn
                                        visible: hostDelegate.hasSettings
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: I18n.t("Stream settings")
                                        Accessible.name: "Stream settings"
                                        onClicked: dashboardRoot.openStreamSettings(hostDelegate.item)
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: gearBtn.hovered ? Theme.accentA(0.1) : "transparent"
                                            border.color: gearBtn.hovered ? Theme.accentFg : Theme.border
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Icon { anchors.centerIn: parent; name: "settings"; size: 17; color: gearBtn.hovered ? Theme.accentFg : Theme.text }
                                        }
                                    }

                                    // Name and fields of a computer of another type (administrators)
                                    Button {
                                        id: editBtn
                                        visible: hostDelegate.hasEdit
                                        hoverEnabled: true
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: I18n.t("Edit")
                                        Accessible.name: "Edit"
                                        onClicked: dashboardRoot.editSource(hostDelegate.method.source)
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: editBtn.hovered ? Theme.accentA(0.1) : "transparent"
                                            border.color: editBtn.hovered ? Theme.accentFg : Theme.border
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Icon { anchors.centerIn: parent; name: "edit"; size: 17; color: editBtn.hovered ? Theme.accentFg : Theme.text }
                                        }
                                    }

                                    // One more way to connect to this computer
                                    // (administrators). A card with several
                                    // has it in the list beside Connect.
                                    Button {
                                        id: addMethodBtn
                                        objectName: "cardAddMethodButton"
                                        visible: hostDelegate.hasAddMethod
                                        hoverEnabled: true
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: I18n.t("Add a connection method")
                                        Accessible.name: I18n.t("Add a connection method")
                                        onClicked: addHostDialog.openMethod(hostDelegate.cardData)
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: addMethodBtn.hovered ? Theme.accentA(0.1) : "transparent"
                                            border.color: addMethodBtn.hovered ? Theme.accentFg : Theme.border
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Icon { anchors.centerIn: parent; name: "plus"; size: 17; color: addMethodBtn.hovered ? Theme.accentFg : Theme.text }
                                        }
                                    }

                                    Button {
                                        id: deleteBtn
                                        visible: hostDelegate.hasRemove
                                        hoverEnabled: true
                                        ToolTip.visible: hovered
                                        ToolTip.delay: 400
                                        ToolTip.text: I18n.t("Remove")
                                        Accessible.name: "Remove"
                                        onClicked: {
                                            if (hostDelegate.isSource) { removeSourceDialog.openFor(hostDelegate.item, hostDelegate.cardData, hostDelegate.method); return; }
                                            dashboardRoot.pendingDeleteHost = hostDelegate.item;
                                        }
                                        background: Rectangle {
                                            implicitWidth: 32; implicitHeight: 32; radius: 6
                                            color: deleteBtn.hovered ? Qt.rgba(239/255, 68/255, 68/255, 0.1) : "transparent"
                                            border.color: deleteBtn.hovered ? "#ef4444" : Theme.border
                                            border.width: 1
                                        }
                                        contentItem: Item {
                                            implicitWidth: 16; implicitHeight: 16
                                            Icon { anchors.centerIn: parent; name: "trash"; size: 17; color: deleteBtn.hovered ? "#ef4444" : Theme.text }
                                        }
                                    }

                                }
                            }
                        }

                        // The list's row: the same computer, the same buttons
                        // (made only for the list).
                        Loader {
                            anchors.fill: parent
                            active: dashboardRoot.listView
                            sourceComponent: Rectangle {
                                id: rowCard
                                objectName: "serverRow"
                                radius: 10
                                color: Theme.surface
                                border.color: hostDelegate.isHostHovered || rowHover.hovered ? Theme.borderStrong : Theme.border
                                HoverHandler { id: rowHover }
                                readonly property color statusColor: hostDelegate.status === "Online" ? "#22c55e" : (hostDelegate.status === "Busy" ? "#f59e0b" : "#ef4444")

                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 12
                                    anchors.rightMargin: 10
                                    spacing: 8
                                    TypeLogo {
                                        typeId: Cards.methodType(hostDelegate.method)
                                        name: String(hostDelegate.method.label || "")
                                        size: 34
                                    }
                                    Column {
                                        Layout.fillWidth: true
                                        Layout.minimumWidth: 60
                                        spacing: 2
                                        Text {
                                            objectName: "cardTitle"
                                            width: parent.width
                                            textFormat: Text.PlainText
                                            text: hostDelegate.cardData.title
                                            font.pixelSize: 14
                                            font.weight: Font.DemiBold
                                            color: Theme.textStrong
                                            elide: Text.ElideRight
                                        }
                                        Row {
                                            width: parent.width
                                            spacing: 6
                                            Rectangle { width: 7; height: 7; radius: 3.5; color: rowCard.statusColor; anchors.verticalCenter: parent.verticalCenter }
                                            Text {
                                                width: parent.width - 13
                                                textFormat: Text.PlainText
                                                objectName: "rowDetail"
                                                text: I18n.t(hostDelegate.status) + "  ·  " + (hostDelegate.isSource
                                                    ? I18n.t("via %1").arg(hostDelegate.item.host_name)
                                                      + (hostDelegate.address !== "" ? "  ·  " + hostDelegate.address : "")
                                                    : (hostDelegate.item.link_error ? I18n.t(hostDelegate.item.link_error)
                                                       : (hostDelegate.address || I18n.t("Connected via Agent"))))
                                                font.pixelSize: 12
                                                color: !hostDelegate.isSource && hostDelegate.item.link_error ? Theme.danger : Theme.textMuted
                                                elide: Text.ElideRight
                                            }
                                        }
                                    }
                                    MethodChips {
                                        objectName: "rowChips"
                                        visible: hostDelegate.multi && rowCard.width > 620
                                        Layout.preferredWidth: Math.min(300, rowCard.width * 0.34)
                                        Layout.preferredHeight: 20
                                        Layout.alignment: Qt.AlignVCenter
                                        clip: true
                                        oneRow: true
                                        methods: hostDelegate.multi ? hostDelegate.cardData.methods : []
                                        current: hostDelegate.method.key
                                        onChosen: (key) => dashboardRoot.chooseMethod(hostDelegate.cardData, key)
                                    }
                                    IconButton {
                                        visible: hostDelegate.hasWeb && rowCard.width > 640
                                        iconName: "globe"
                                        tip: I18n.t("Open in the browser")
                                        onClicked: webBtn.clicked()
                                    }
                                    IconButton {
                                        visible: hostDelegate.hasFiles && rowCard.width > 480
                                        iconName: "folder"
                                        tip: I18n.t("Files")
                                        onClicked: filesBtn.clicked()
                                    }
                                    IconButton {
                                        visible: hostDelegate.hasSettings && rowCard.width > 480
                                        iconName: "settings"
                                        tip: I18n.t("Stream settings")
                                        onClicked: gearBtn.clicked()
                                    }
                                    IconButton {
                                        visible: hostDelegate.hasEdit && rowCard.width > 640
                                        iconName: "edit"
                                        tip: I18n.t("Edit")
                                        onClicked: editBtn.clicked()
                                    }
                                    IconButton {
                                        visible: hostDelegate.hasAddMethod && rowCard.width > 640
                                        iconName: "plus"
                                        tip: I18n.t("Add a connection method")
                                        onClicked: addMethodBtn.clicked()
                                    }
                                    IconButton {
                                        visible: hostDelegate.hasRemove && rowCard.width > 560
                                        iconName: "trash"
                                        tip: I18n.t("Remove")
                                        onClicked: deleteBtn.clicked()
                                    }
                                    // (The same room in every row: the buttons line up.)
                                    Item {
                                        Layout.preferredWidth: Math.max(rowConnectRow.width, 124)
                                        Layout.preferredHeight: 30
                                        Row {
                                            id: rowConnectRow
                                            anchors.right: parent.right
                                            spacing: 1
                                            PrimaryButton {
                                                objectName: "rowConnect"
                                                visible: hostDelegate.status === "Online"
                                                text: I18n.t("Connect")
                                                implicitHeight: 30
                                                leftPadding: 14
                                                rightPadding: 14
                                                onClicked: hostDelegate.connect()
                                            }
                                            Button {
                                                id: rowChooseBtn
                                                objectName: "rowChooseButton"
                                                visible: hostDelegate.multi
                                                hoverEnabled: true
                                                ToolTip.visible: hovered
                                                ToolTip.delay: 400
                                                ToolTip.text: I18n.t("Choose how to connect")
                                                Accessible.name: I18n.t("Choose how to connect")
                                                onClicked: methodMenu.openFor(hostDelegate.cardData, rowChooseBtn)
                                                background: Rectangle {
                                                    implicitWidth: 24; implicitHeight: 30; radius: Theme.radiusSm
                                                    color: rowChooseBtn.hovered ? Theme.accentHover : Theme.accent
                                                }
                                                contentItem: Item {
                                                    implicitWidth: 12; implicitHeight: 16
                                                    Icon { anchors.centerIn: parent; name: "chevronDown"; size: 14; color: Theme.textOnAccent }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Computers there are, but the search and the filter find none.
            NoMatch {
                objectName: "serverNoMatch"
                anchors.centerIn: parent
                visible: dashboardRoot.cards.length > 0 && dashboardRoot.shownCards.length === 0
                width: Math.min(parent.width - 48, 420)
                onCleared: dashboardRoot.clearFinding()
            }

            // Empty state view
            Column {
                anchors.centerIn: parent
                visible: dashboardRoot.cards.length === 0 && !dashboardRoot.hostsLoading
                spacing: 16
                Text { text: I18n.t("No agents connected yet"); font.pixelSize: 16; font.bold: true; color: Theme.textMuted; anchors.horizontalCenter: parent.horizontalCenter }
                Text { text: I18n.t("Agents will appear here automatically once registered"); font.pixelSize: 12; color: Theme.textSubtle; anchors.horizontalCenter: parent.horizontalCenter }
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
                        color: backBtn.hovered ? Theme.accentA(0.1) : "transparent"
                        border.color: backBtn.hovered ? Theme.accentFg : "transparent"
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
                            
                            property color arrowColor: backBtn.hovered ? Theme.accentFg : Theme.text
                            
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
                        Text { text: I18n.t("Device Directory"); font.pixelSize: 12; color: Theme.textSubtle }
                        Text { text: "/"; font.pixelSize: 12; color: Theme.textSubtle }
                        Text { textFormat: Text.PlainText; text: dashboardRoot.selectedHost.name; font.pixelSize: 12; font.bold: true; color: Theme.accentFg }
                    }
                }
            }

            Text {
                id: appsTitle
                text: I18n.t("Applications Directory")
                font.pixelSize: 22
                font.bold: true
                color: Theme.textStrong
                anchors.top: backHeader.bottom
                anchors.topMargin: 12
            }

            Text {
                id: appsSubtitle
                text: "Select and stream apps from " + dashboardRoot.selectedHost.name
                font.pixelSize: 12
                color: Theme.textSubtle
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
                                    dashboardRoot.sourceFor(dashboardRoot.selectedHost)
                                );
                            }
                        }

                        Rectangle {
                            id: appCard
                            width: 180
                            height: 250
                            radius: Theme.radiusLg
                            clip: true
                            color: isCardHovered ? Theme.surface2 : Theme.surface
                            Behavior on color { ColorAnimation { duration: 120 } }

                            // Border drawn on top so cover images never hide it
                            Rectangle {
                                anchors.fill: parent
                                radius: parent.radius
                                color: "transparent"
                                border.color: isCardHovered ? Theme.accentFg : Theme.border
                                border.width: 1
                                z: 20
                                Behavior on border.color { ColorAnimation { duration: 120 } }
                            }

                            // Base64 cover image if exists
                            Image {
                                anchors.fill: parent
                                fillMode: Image.PreserveAspectCrop
                                visible: !!modelData.icon_base64
                                source: modelData.icon_base64 ? "data:image/png;base64," + modelData.icon_base64 : ""
                            }

                            // Glassmorphic overlay for cover images on hover
                            Rectangle {
                                anchors.fill: parent
                                color: Theme.surfaceA(0.75)
                                visible: !!modelData.icon_base64
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
                                    border.color: Theme.ink(0.22)
                                    border.width: 3
                                    anchors.centerIn: parent
                                }

                                // Monitor Stand
                                Rectangle {
                                    id: monitorStand
                                    width: 14; height: 12
                                    color: Theme.ink(0.22)
                                    anchors.top: monitorScreen.bottom
                                    anchors.horizontalCenter: monitorScreen.horizontalCenter
                                }

                                // Monitor Base
                                Rectangle {
                                    id: monitorBase
                                    width: 36; height: 3; radius: 1
                                    color: Theme.ink(0.22)
                                    anchors.top: monitorStand.bottom
                                    anchors.horizontalCenter: monitorScreen.horizontalCenter
                                }
                            }

                            // App Title Text (Aligned near the bottom)
                            Text {
                                id: titleText
                                textFormat: Text.PlainText
                                text: modelData.title
                                font.pixelSize: 14
                                font.weight: Font.DemiBold
                                color: Theme.textStrong
                                anchors.bottom: parent.bottom
                                anchors.bottomMargin: 24
                                anchors.horizontalCenter: parent.horizontalCenter
                                z: 4
                                visible: !modelData.icon_base64 || isCardHovered
                                horizontalAlignment: Text.AlignHCenter
                                width: parent.width - 30
                                wrapMode: Text.Wrap
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
                                    
                                    property color triangleColor: playMouse.containsMouse ? Theme.accentFg : Theme.textStrong
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
                                            dashboardRoot.sourceFor(dashboardRoot.selectedHost)
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
                Text { text: I18n.t("No games/apps available"); font.pixelSize: 14; color: Theme.textMuted; anchors.horizontalCenter: parent.horizontalCenter }
            }
        }
    }

    Rectangle {
        id: deleteHostOverlay
        anchors.fill: parent
        z: 1001
        visible: dashboardRoot.pendingDeleteHost !== null
        color: Theme.surfaceA(0.78)

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
            color: Theme.surface
            border.color: Theme.ink(0.10)
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
                    text: I18n.t("Delete Host")
                    color: Theme.textStrong
                    font.pixelSize: 18
                    font.bold: true
                }
                Text {
                    width: parent.width
                    wrapMode: Text.Wrap
                    color: Theme.text
                    font.pixelSize: 13
                    lineHeight: 1.25
                    textFormat: Text.PlainText
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
                    text: I18n.t("Cancel")
                    onClicked: dashboardRoot.pendingDeleteHost = null
                    background: Rectangle {
                        implicitWidth: 96; implicitHeight: 36; radius: 8
                        color: cancelDeleteBtn.hovered ? Theme.ink(0.05) : "transparent"
                        border.color: Theme.ink(0.16)
                        border.width: 1
                    }
                    contentItem: Text { text: cancelDeleteBtn.text; color: cancelDeleteBtn.hovered ? Theme.textStrong : Theme.text; font.bold: true; font.pixelSize: 13; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                }

                Button {
                    id: confirmDeleteBtn
                    text: I18n.t("Delete Host")
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
                    contentItem: Text { text: confirmDeleteBtn.text; color: Theme.textStrong; font.bold: true; font.pixelSize: 13; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                }
            }
        }
    }

    // Settings Modal
    // A page over this view (the Server tab), not a dialog that a small
    // window cuts off.
    Settings {
        id: settingsModal
        anchors.fill: parent
        z: 1000

        onApplySettings: (res, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol, encoder, displayId, sourceId) => {
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
            dashboardRoot.configSource = sourceId;
        }
    }

    // "Add computer" and "Edit" (administrators). Type Lunaris: by address,
    // the server connects to the Lunaris app on it (without its password,
    // the person there accepts each connection). Another type: through one
    // of the server's Lunaris computers, which keeps its fields.
    ApiClient { id: addHostApi }
    Timer { id: addHostRefresh; interval: 2500; onTriggered: bridge.fetchHosts() }
    AddComputerDialog {
        id: addHostDialog
        objectName: "serverAddDialog"
        serverMode: true
        connectors: dashboardRoot.connectorTypes
        through: dashboardRoot.connectorHosts
        function openDialog() {
            dashboardRoot.refreshSources();
            openAdd();
        }
        // One more way to connect to the computer of a card.
        function openMethod(card) {
            dashboardRoot.refreshSources();
            openAddMethod(card);
        }
        // The Lunaris computer a type with a set-up is being added through.
        property string addingThrough: ""
        property bool setupAsking: false
        property int setupMisses: 0
        function added() {
            addHostDialog.close();
            bridge.fetchHosts();
            addHostRefresh.restart();
        }
        function done(ok, data, err, status, text) {
            addHostDialog.busy = false;
            if (!ok) { addHostDialog.error = dashboardRoot.apiError(err, status, text, data); return; }
            // 202: its type has a set-up (a pairing) that waits for the
            // person; the dialog shows what the connector wants shown.
            if (status === 202 && data && data.setup) {
                addHostDialog.setupAsking = false;
                addHostDialog.setupMisses = 0;
                addHostDialog.setupShow(data.setup, addHostDialog.addingThrough, data.message, data.code);
                return;
            }
            addHostDialog.added();
        }
        function setupPath(setup, hostId) {
            return "/api/hosts/" + encodeURIComponent(hostId) + "/sources/setup/" + encodeURIComponent(setup);
        }
        onLunarisSubmitted: (address, password, name) => {
            addHostDialog.busy = true;
            addHostApi.call("POST", "/api/hosts/address", { address: address, password: password || null }, addHostDialog.done);
        }
        onSourceSubmitted: (connectorId, hostId, sourceId, name, values, group) => {
            addHostDialog.busy = true;
            addHostDialog.addingThrough = hostId;
            // Which computer it is, when the form said ("*": nothing said,
            // and nothing is sent, as before).
            dashboardRoot.withGroup(group === undefined ? "*" : group, function (said, error) {
                if (error !== "") { addHostDialog.busy = false; addHostDialog.error = error; return; }
                var path = "/api/hosts/" + encodeURIComponent(hostId) + "/sources";
                var form = sourceId === "" ? { name: name, connector: connectorId, values: values } : { name: name, values: values };
                if (said !== "*") form.group = said;
                if (sourceId === "")
                    addHostApi.call("POST", path, form, addHostDialog.done);
                else
                    addHostApi.call("PUT", path + "/" + encodeURIComponent(sourceId), form, addHostDialog.done);
            });
        }
        // Once a second while the set-up waits: done closes the dialog (the
        // new card is listed), failed shows why.
        onSetupAsked: (setup, hostId) => {
            if (addHostDialog.setupAsking) return;
            addHostDialog.setupAsking = true;
            addHostApi.call("GET", addHostDialog.setupPath(setup, hostId), null, function (ok, data, err, status, text) {
                addHostDialog.setupAsking = false;
                // An answer about a set-up that is not shown any more.
                if (addHostDialog.setupId !== setup || !addHostDialog.setupWaiting) return;
                if (!ok) {
                    if (status === 404) addHostDialog.setupFailed(dashboardRoot.apiError(err, status, text, data));
                    // Not at the first hiccup: the computer may be back in a moment.
                    else if (++addHostDialog.setupMisses >= 5) addHostDialog.setupFailed(dashboardRoot.apiError(err, status, text, data));
                    return;
                }
                addHostDialog.setupMisses = 0;
                if (data && data.state === "done") {
                    addHostDialog.setupFinished();
                    addHostDialog.added();
                } else if (data && data.state === "failed") {
                    addHostDialog.setupFailed(I18n.tError(String(data.error || "")));
                } else if (data && data.state === "pending") {
                    addHostDialog.setupUpdate(data.message, data.code);
                }
            });
        }
        // Closed while it waited: stopped on that computer.
        onSetupCancelled: (setup, hostId) => addHostApi.call("DELETE", addHostDialog.setupPath(setup, hostId), null, function () {})
    }

    // The ways to connect to a card's computer, to connect with one at once
    // (it is then the one the card remembers), and adding one. One list
    // for all the cards: the cards are made anew whenever the list comes.
    ThemedMenu {
        id: methodMenu
        objectName: "methodMenu"
        property var card: null
        function openFor(card, button) {
            methodMenu.card = card;
            popup(button, 0, button.height + 2);
        }
        width: 250
        // (Put in by hand, so that they come before what follows them.)
        Instantiator {
            model: methodMenu.card ? methodMenu.card.methods : []
            onObjectAdded: (index, object) => methodMenu.insertItem(index, object)
            onObjectRemoved: (index, object) => methodMenu.removeItem(object)
            delegate: MenuItem {
                id: methodEntry
                required property var modelData
                objectName: "methodItem"
                property string methodKey: modelData.key
                enabled: modelData.online
                text: I18n.t("Connect with %1").arg(modelData.label)
                leftPadding: 36
                TypeLogo {
                    x: 10
                    anchors.verticalCenter: parent.verticalCenter
                    typeId: Cards.methodType(methodEntry.modelData)
                    name: String(methodEntry.modelData.label)
                    size: 18
                    opacity: methodEntry.enabled ? 1 : 0.5
                }
                contentItem: Text {
                    textFormat: Text.PlainText
                    text: methodEntry.text
                    color: methodEntry.enabled ? Theme.text : Theme.textSubtle
                    font.pixelSize: 13
                    elide: Text.ElideRight
                    verticalAlignment: Text.AlignVCenter
                }
                onTriggered: {
                    var card = dashboardRoot.cardByKey(methodMenu.card.key) || methodMenu.card;
                    for (var i = 0; i < card.methods.length; i++)
                        if (card.methods[i].key === modelData.key) { dashboardRoot.connectMethod(card, card.methods[i]); return; }
                }
            }
        }
        MenuSeparator { visible: addMethodItem.visible; height: visible ? implicitHeight : 0 }
        MenuItem {
            id: addMethodItem
            objectName: "methodAddItem"
            visible: dashboardRoot.isAdmin && methodMenu.card !== null && dashboardRoot.takesMethods(methodMenu.card)
            height: visible ? implicitHeight : 0
            text: I18n.t("Add a connection method…")
            onTriggered: addHostDialog.openMethod(dashboardRoot.cardByKey(methodMenu.card.key) || methodMenu.card)
        }
    }

    // Remove a computer of another type (administrators): asked first.
    DialogFrame {
        id: removeSourceDialog
        objectName: "removeSourceDialog"
        width: 440
        property var source: null
        // In a card with other ways to connect: the card's name and this
        // way's ("" when the card is this source and no more).
        property string cardTitle: ""
        property string methodLabel: ""
        property bool busy: false
        property string error: ""
        function openFor(source, card, method) {
            removeSourceDialog.source = source;
            removeSourceDialog.cardTitle = card && card.methods.length > 1 ? String(card.title) : "";
            removeSourceDialog.methodLabel = method ? String(method.label) : "";
            error = "";
            busy = false;
            open();
        }
        function confirm() {
            if (busy || !source) return;
            busy = true;
            error = "";
            addHostApi.call("DELETE", "/api/hosts/" + encodeURIComponent(source.host_id) + "/sources/" + encodeURIComponent(source.id), null,
                function (ok, data, err, status, text) {
                    removeSourceDialog.busy = false;
                    if (!ok) { removeSourceDialog.error = dashboardRoot.apiError(err, status, text, data); return; }
                    removeSourceDialog.close();
                    dashboardRoot.refreshSources();
                });
        }
        contentItem: ColumnLayout {
            spacing: 12
            Text { text: I18n.t("Remove computer"); color: Theme.textStrong; font.pixelSize: 16; font.weight: Font.DemiBold }
            Text {
                objectName: "removeSourceText"
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: !removeSourceDialog.source ? ""
                    : removeSourceDialog.cardTitle !== ""
                        ? I18n.t("Remove %1 from %2? The other ways to connect stay.").arg(removeSourceDialog.methodLabel).arg(removeSourceDialog.cardTitle)
                        : I18n.t("Remove \"%1\"? It is removed from %2, which will no longer show it.").arg(removeSourceDialog.source.name).arg(removeSourceDialog.source.host_name)
                color: Theme.text
                font.pixelSize: 13
                wrapMode: Text.WordWrap
            }
            Text {
                visible: removeSourceDialog.error !== ""
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: removeSourceDialog.error
                color: Theme.danger
                font.pixelSize: 12
                wrapMode: Text.WordWrap
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { text: I18n.t("Cancel"); onClicked: removeSourceDialog.close() }
                SecondaryButton {
                    danger: true
                    text: removeSourceDialog.busy ? I18n.t("Removing…") : I18n.t("Remove")
                    onClicked: removeSourceDialog.confirm()
                }
            }
        }
    }

    // Two-factor sign-in: the code from the authenticator app (MfaDialog.qml).
    MfaDialog {
        id: mfaDialog
        onExpired: dashboardRoot.authError = I18n.t("This sign-in has expired. Sign in again.")
    }
}
