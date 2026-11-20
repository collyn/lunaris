import QtQuick

// Stands in for the Rust `bridge` (StreamBridge) when the pages are
// rendered off screen: the functions and signals Home.qml, Dashboard.qml
// and the dialogs use, answering from canned data. Nothing leaves this
// process and nothing is stored.
QtObject {
    id: stub

    // ---- What the tests set ----
    // "admin" | "user"; the canned server's answers.
    property string role: "admin"
    // A server from before sources: 404 to /api/sources and /api/connectors.
    property bool oldServer: false
    // The answer to the next write of a source: { status, body }, or null (200).
    property var nextWrite: null
    // What the pages asked for, newest last: { method, path, body }.
    property var calls: []
    // The session the pages started last (startSessionRequested is theirs;
    // this is for fetchCapabilities and friends).
    property var probed: []
    property var capabilitiesAsked: []
    property var stores: ({})

    readonly property var rustdesk: ({
        id: "rustdesk", name: "RustDesk (by IP address)",
        capabilities: { audio: true, input: true, clipboard: true, cursor: true, files: true, setup: false },
        fields: [
            { key: "host", label: "IP address", kind: "text", "default": "", required: true },
            { key: "port", label: "Port", kind: "port", "default": "21118", required: false },
            { key: "password", label: "Password", kind: "secret", "default": "", required: true }
        ]
    })
    // A made-up connector with other fields, to show the form is built from data.
    // (It says nothing of what it can do, as an agent from before that does.)
    readonly property var acme: ({
        id: "acme-view", name: "Acme Viewer",
        fields: [
            { key: "station", label: "Station name", kind: "text", "default": "", required: true },
            { key: "gateway", label: "Gateway", kind: "text", "default": "gw.example.net", required: false },
            { key: "gateway_port", label: "Gateway port", kind: "port", "default": "7400", required: true },
            { key: "token", label: "Access token", kind: "secret", "default": "", required: true },
            { key: "pin", label: "PIN", kind: "secret", "default": "", required: false }
        ]
    })
    // A connector with a set-up (a pairing that shows a PIN) and a note,
    // as the Sunshine connector describes itself. Offered when a test puts
    // it in `extra`.
    readonly property var sunshine: ({
        id: "sunshine", name: "Sunshine / GameStream",
        capabilities: { audio: true, input: true, clipboard: false, cursor: false, files: false, setup: true },
        note: "Sunshine must already be installed and running on that computer. After you press Add, a PIN is shown here: enter it in Sunshine's web page (PIN).",
        fields: [
            { key: "host", label: "Address", kind: "text", "default": "", required: true },
            { key: "port", label: "Port (HTTP)", kind: "port", "default": "47989", required: false },
            { key: "app", label: "Application", kind: "text", "default": "Desktop", required: false },
            { key: "resolution", label: "Resolution", kind: "text", "default": "1920x1080", required: false },
            { key: "bitrate", label: "Bitrate (Mbit/s)", kind: "text", "default": "20", required: false },
            { key: "pointer", label: "Pointer", kind: "choice", "default": "picture", required: false, options: [
                { value: "picture", label: "In the picture (host's pointer)" },
                { value: "own", label: "Drawn here (no lag; arrow only)" }
            ] }
        ]
    })
    // The RDP and VNC connectors as the agent describes them (offered when
    // a test puts them in `extra`).
    readonly property var rdp: ({
        id: "rdp", name: "Remote Desktop (RDP)",
        note: "Remote Desktop must be enabled on that computer (Windows: Settings, System, Remote Desktop). When you press Add, its certificate is checked and remembered; Lunaris refuses the connection if it changes later.",
        fields: [
            { key: "host", label: "Address", kind: "text", "default": "", required: true },
            { key: "port", label: "Port", kind: "port", "default": "3389", required: false },
            { key: "username", label: "User name", kind: "text", "default": "", required: false },
            { key: "password", label: "Password", kind: "secret", "default": "", required: false },
            { key: "domain", label: "Domain", kind: "text", "default": "", required: false },
            { key: "security", label: "Security", kind: "choice", "default": "nla", required: false, options: [
                { value: "nla", label: "NLA (recommended)" },
                { value: "tls", label: "TLS (sign in on the computer's screen)" },
                { value: "any", label: "Either" }
            ] },
            { key: "resolution", label: "Resolution", kind: "text", "default": "auto", required: false }
        ]
    })
    readonly property var vnc: ({
        id: "vnc", name: "VNC",
        note: "A VNC server must be running on that computer, with \"None\" or \"VNC password\" security. VNC does not encrypt the picture or what you type: use it only on a network you trust, or through a VPN.",
        fields: [
            { key: "host", label: "Address", kind: "text", "default": "", required: true },
            { key: "port", label: "Port", kind: "port", "default": "5900", required: false },
            { key: "password", label: "Password", kind: "secret", "default": "", required: false },
            { key: "view_only", label: "Mode", kind: "choice", "default": "off", required: false, options: [
                { value: "off", label: "Control" },
                { value: "on", label: "View only" }
            ] }
        ]
    })
    property var extra: []
    readonly property var firstSetup: ({ state: "pending", message: "Open Sunshine's web page on that computer, choose PIN, and enter this PIN.", code: "4711" })
    // How the set-up "su-1" stands: what asking about it is answered.
    property var setupAnswer: firstSetup
    // The status the server answers asking with (200, or 404 ...).
    property int setupStatus: 200
    // The set-ups that were cancelled, and how often one was asked about.
    property var cancelled: []
    property int setupAsked: 0
    readonly property var firstOwnSources: [
        { id: "den-pc-1a2b", name: "Den PC", connector: "rustdesk", connector_name: "RustDesk (by IP address)", values: { host: "192.168.1.50", port: "21118" } }
    ]
    property var ownSources: firstOwnSources
    readonly property var firstHosts: [
        { id: "0F3A9C2E-77B1-4C1D-9A55-1B2C3D4E5F60", name: "Office PC", status: "Online", address: "192.168.1.20:8091" },
        { id: "5D21B7AA-0C4E-4F7B-8E11-99A0B1C2D3E4", name: "Lab PC", status: "Offline", ip_address: "10.0.0.7", addresses: ["192.168.1.40", "100.64.0.40"] }
    ]
    property var hosts: firstHosts
    readonly property var firstServerSources: [
        { host_id: "0F3A9C2E-77B1-4C1D-9A55-1B2C3D4E5F60", host_name: "Office PC", id: "lab-server-9f31", name: "Lab server", connector: "rustdesk", connector_name: "RustDesk (by IP address)", online: true, capabilities: { files: true }, address: "192.168.1.30" },
        { host_id: "5D21B7AA-0C4E-4F7B-8E11-99A0B1C2D3E4", host_name: "Lab PC", id: "press-7c02", name: "Press room", connector: "acme-view", connector_name: "Acme Viewer", online: false }
    ]
    property var serverSources: firstServerSources

    // Back to the data above, as a fresh start has it.
    function startOver() {
        role = "admin";
        oldServer = false;
        nextWrite = null;
        calls = [];
        probed = [];
        capabilitiesAsked = [];
        ownSources = firstOwnSources;
        hosts = firstHosts;
        serverSources = firstServerSources;
        extra = [];
        setupAnswer = firstSetup;
        setupStatus = 200;
        cancelled = [];
        setupAsked = 0;
        groupsSaid = [];
        setupGroup = "*";
        setupHost = "";
        waylandCapture = "auto";
        waylandCaptureSet = [];
    }

    // ---- Signals of the real bridge that the pages listen to ----
    signal loginResult(bool success, string errorMsg, string tok, string user, string srv)
    signal mfaRequired(string srv, string user)
    signal credentialsLoaded(bool success, string srv, string tok, string user)
    signal hostsResult(bool success, string errorMsg, string hostsJson)
    signal appsResult(bool success, string errorMsg, string hostId, string appsJson)
    signal agentTokenResult(bool success, string errorMsg, string tok)
    signal apiResult(string reqId, int status, string body)
    signal capabilitiesUpdated(string displaysJson, string encodersJson, string gpuInfo, string hostOs, string sourcesJson)

    // ---- Stores and status ----
    function loadStore(name) { return stub.stores[name] || ""; }
    function saveStore(name, json) { var next = Object.assign({}, stub.stores); next[name] = json; stub.stores = next; }
    function appVersion() { return "0.5.0"; }
    function screenRefreshRate() { return 60; }
    function savedDirectAddress() { return ""; }
    function savedDirectPassword() { return ""; }
    function saveDirectTarget(address, password) {}
    function getLocalHostname() { return "this-pc"; }
    function isAutostartEnabled() { return false; }
    function setAutostartEnabled(on) {}
    // The updater (updater/src/service.rs): the tests set `updateState`
    // and count what the pages ask for.
    property var updateState: ({ state: "idle", current: "0.5.0", latest: "", notes: "", releaseUrl: "",
                                 canInstall: false, manualReason: "", manualMessage: "", progress: 0,
                                 error: "", errorKind: "", autoCheck: true, lastChecked: 0, installKind: "deb" })
    property int updateChecks: 0
    property int updateInstalls: 0
    property var updateAutoChecks: []
    function checkForUpdates() { stub.updateChecks += 1; }
    function updateStatus() { return JSON.stringify(stub.updateState); }
    function updateInstall() { stub.updateInstalls += 1; }
    function updateRestart() { return false; }
    function updateAutoCheck() { return stub.updateState.autoCheck; }
    function setUpdateAutoCheck(on) { stub.updateAutoChecks = stub.updateAutoChecks.concat([on]); }
    function hostStatus() {
        return JSON.stringify({ enabled: true, running: true, port: 8091, password: "k7m2p9qd", addresses: ["192.168.1.10"],
            hostname: "this-pc", input_ready: true, allow_clipboard: true, allow_file_transfer: true, full_disk_access: false,
            approve_mode: "password-click", rustdesk_enabled: false, rustdesk_port: 21118, rustdesk_file_transfer: false,
            wayland_capture: stub.waylandCapture, error: "" });
    }
    function hostStart() { return ""; }
    function hostStop() { return ""; }
    function hostSetPassword(p) { return ""; }
    function hostSetPort(p) { return ""; }
    function hostSetRustdesk(on, port) { return ""; }
    function hostSetSharing(c, f) { return ""; }
    function hostSetFileAccess(full) { return ""; }
    function hostSetApproveMode(mode) { return ""; }
    // The host's `wayland_capture` ("auto" / "portal") and what was set, newest last.
    property string waylandCapture: "auto"
    property var waylandCaptureSet: []
    function hostSetWaylandCapture(mode) {
        stub.waylandCaptureSet = stub.waylandCaptureSet.concat([mode]);
        if (mode !== "auto" && mode !== "portal") return "unknown Wayland capture mode " + mode;
        stub.waylandCapture = mode;
        return "";
    }
    function hostUseServer(server, token) { return "already"; }
    function serverStatus() {
        return JSON.stringify({ enabled: false, running: false, ours: false, port: 8090, local_url: "", lan_url: "", admin_password: "", data_dir: "", error: "" });
    }
    function serverStart() { return ""; }
    function serverStop() { return ""; }
    function serverSetPort(p) { return ""; }
    function probePeer(address) { stub.probed = stub.probed.concat([address]); }
    function peerOnline(address) { return address.indexOf("192.168.1.20") === 0 ? 1 : 0; }
    function fetchCapabilities(server, token, hostId) { stub.capabilitiesAsked = stub.capabilitiesAsked.concat([server + " " + hostId]); }
    function fetchAgentToken(server, token) {}
    function deleteHost(id) {}
    function logout() {}
    function login(server, user, password) {}
    // Two-factor sign-in: what the dialog sent, newest last ({ code, recovery }).
    property var mfaCalls: []
    property int mfaCancels: 0
    function loginMfa(code, recovery) { stub.mfaCalls = stub.mfaCalls.concat([{ code: code, recovery: recovery }]); }
    function cancelMfa() { stub.mfaCancels += 1; }

    // ---- The app's own computers of other types ----
    function bridgeConnectors() { return JSON.stringify([stub.rustdesk, stub.acme].concat(stub.extra)); }
    function bridgeSources() { return JSON.stringify(stub.ownSources); }
    // Whether each of them answers: source id -> 1 / 0; not in it: not known (-1).
    property var sourceStatus: ({})
    function bridgeSourceOnline(id) { return stub.sourceStatus[id] === undefined ? -1 : stub.sourceStatus[id]; }
    function shownValues(connectorId, values) {
        var c = connectorId === "rustdesk" ? stub.rustdesk : connectorId === "sunshine" ? stub.sunshine : stub.acme;
        var out = {};
        for (var i = 0; i < c.fields.length; i++)
            if (c.fields[i].kind !== "secret" && values[c.fields[i].key]) out[c.fields[i].key] = values[c.fields[i].key];
        return out;
    }
    function bridgeSourceSetup(setup) {
        stub.setupAsked += 1;
        var answer = stub.setupAnswer;
        if (answer.state === "done" && !stub.ownSources.some(function (s) { return s.id === answer.id; })) {
            // Saved now, with its group (the agent does it when the set-up ends).
            var host = stub.setupHost || "192.168.1.70";
            var g = groupForNew(host, stub.setupGroup);
            var added = { id: answer.id, name: "Living room", connector: "sunshine", connector_name: stub.sunshine.name, values: { host: host } };
            if (g !== "") added.group = g;
            stub.ownSources = stub.ownSources.concat([added]);
        }
        return JSON.stringify(answer);
    }
    function bridgeCancelSetup(setup) { stub.cancelled = stub.cancelled.concat([setup]); }
    // ---- Groups, as the agent keeps them (agent/src/connector/sources.rs) ----
    // What the pages said of a group, newest last: { op, id, group }.
    property var groupsSaid: []
    function validGroup(group) { return /^[A-Za-z0-9._:-]{1,64}$/.test(group); }
    // The group of a computer being added at `host`: "*" is the group of
    // the first computer at that address (which gets one now if it had
    // none), "" none, anything else that group.
    function groupForNew(host, said) {
        if (said !== "*") return said;
        var address = String(host || "").trim().toLowerCase();
        if (address === "") return "";
        var group = "";
        var made = "g-" + ("000000000000" + (stub.ownSources.length + 1).toString(16)).slice(-12);
        var found = false;
        stub.ownSources = stub.ownSources.map(function (s) {
            if (found || String((s.values || {}).host || "").trim().toLowerCase() !== address) return s;
            found = true;
            group = s.group || made;
            return s.group ? s : Object.assign({}, s, { group: group });
        });
        return group;
    }
    // The set-up "su-1" is of a computer with this group and this address.
    property string setupGroup: "*"
    property string setupHost: ""
    function bridgeAddSource(name, connector, valuesJson) { return bridgeAddSourceGrouped(name, connector, valuesJson, "*"); }
    function bridgeAddSourceGrouped(name, connector, valuesJson, group) {
        stub.groupsSaid = stub.groupsSaid.concat([{ op: "add", id: "", group: group }]);
        if (group !== "*" && group !== "" && !validGroup(group)) return JSON.stringify({ error: "That group of computers is not valid" });
        var values = JSON.parse(valuesJson);
        // Its type has a set-up: nothing is added yet.
        if (connector === "sunshine") {
            stub.setupGroup = group;
            stub.setupHost = String(values.host || "");
            return JSON.stringify({ setup: "su-1", message: stub.firstSetup.message, code: stub.firstSetup.code });
        }
        var c = connector === "rustdesk" ? stub.rustdesk : stub.acme;
        var id = name.toLowerCase().replace(/[^a-z0-9]+/g, "-") + "-0001";
        var g = groupForNew(values.host, group);
        var added = { id: id, name: name, connector: connector, connector_name: c.name, values: shownValues(connector, values) };
        if (g !== "") added.group = g;
        stub.ownSources = stub.ownSources.concat([added]);
        return JSON.stringify({ id: id });
    }
    function bridgeUpdateSource(id, name, valuesJson) { return bridgeUpdateSourceGrouped(id, name, valuesJson, "*"); }
    function bridgeUpdateSourceGrouped(id, name, valuesJson, group) {
        stub.groupsSaid = stub.groupsSaid.concat([{ op: "update", id: id, group: group }]);
        if (group !== "*" && group !== "" && !validGroup(group)) return JSON.stringify({ error: "That group of computers is not valid" });
        var found = false;
        stub.ownSources = stub.ownSources.map(function (s) {
            if (s.id !== id) return s;
            found = true;
            var next = Object.assign({}, s, { name: name, values: Object.assign({}, s.values, shownValues(s.connector, JSON.parse(valuesJson))) });
            if (group === "") delete next.group;
            else if (group !== "*") next.group = group;
            return next;
        });
        return JSON.stringify(found ? { id: id } : { error: "That computer is not there any more" });
    }
    function bridgeRemoveSource(id) {
        stub.ownSources = stub.ownSources.filter(function (s) { return s.id !== id; });
        return "";
    }

    // ---- The canned server ----
    function loadSavedCredentials() { stub.credentialsLoaded(true, "http://192.168.1.5:8090", "session-token", "huy"); }
    function fetchHosts() { stub.hostsResult(true, "", JSON.stringify(stub.hosts)); }
    function apiRequest(reqId, method, path, body) {
        stub.calls = stub.calls.concat([{ method: method, path: path, body: body }]);
        var status = 200;
        var answer = "{}";
        if (path === "/api/auth/me") {
            answer = JSON.stringify({ username: "huy", role: stub.role });
        } else if (path === "/api/sources" || path === "/api/connectors") {
            if (stub.oldServer) { status = 404; answer = ""; }
            else if (path === "/api/sources") answer = JSON.stringify(stub.serverSources);
            else answer = JSON.stringify([{ host_id: stub.hosts[0].id, host_name: stub.hosts[0].name, connectors: [stub.rustdesk, stub.acme].concat(stub.extra) }]);
        } else if (/^\/api\/hosts\/[^/]+\/sources/.test(path)) {
            if (stub.nextWrite) { status = stub.nextWrite.status; answer = stub.nextWrite.body; stub.nextWrite = null; }
            else if (/\/sources\/setup\/[^/]+$/.test(path)) {
                // The set-up of a computer being added: asked about, or stopped.
                if (method === "DELETE") stub.cancelled = stub.cancelled.concat([path.slice(path.lastIndexOf("/") + 1)]);
                else {
                    stub.setupAsked += 1;
                    status = stub.setupStatus;
                    answer = status === 200 ? JSON.stringify(stub.setupAnswer) : JSON.stringify({ error: "That set-up is not there any more. Add the computer again." });
                    if (status === 200 && stub.setupAnswer.state === "done") {
                        var paired = { host_id: stub.hosts[0].id, host_name: stub.hosts[0].name, id: stub.setupAnswer.id, name: "Living room", connector: "sunshine", connector_name: stub.sunshine.name, online: true, capabilities: stub.sunshine.capabilities };
                        if (stub.setupGroup !== "*" && stub.setupGroup !== "") paired.group = stub.setupGroup;
                        stub.serverSources = stub.serverSources.concat([paired]);
                    }
                }
            }
            else if (JSON.parse(body || "{}").group !== undefined && JSON.parse(body).group !== "" && !validGroup(JSON.parse(body).group)) {
                status = 400;
                answer = JSON.stringify({ error: "That group of computers is not valid" });
            }
            else if (method === "POST" && JSON.parse(body).connector === "sunshine") {
                stub.setupGroup = JSON.parse(body).group === undefined ? "*" : JSON.parse(body).group;
                status = 202;
                answer = JSON.stringify({ setup: "su-1", message: stub.firstSetup.message, code: stub.firstSetup.code });
            }
            else if (method === "POST") {
                var form = JSON.parse(body);
                var fresh = { host_id: stub.hosts[0].id, host_name: stub.hosts[0].name, id: "new-0001", name: form.name, connector: form.connector, connector_name: form.connector === "rustdesk" ? stub.rustdesk.name : stub.acme.name, online: true };
                if (form.group) fresh.group = form.group;
                stub.serverSources = stub.serverSources.concat([fresh]);
                answer = JSON.stringify({ id: "new-0001" });
            } else if (method === "DELETE") {
                var id = path.slice(path.lastIndexOf("/") + 1);
                stub.serverSources = stub.serverSources.filter(function (s) { return s.id !== id; });
            } else {
                // PUT: the name, and the group when the form said.
                var edited = path.slice(path.lastIndexOf("/") + 1);
                var change = JSON.parse(body || "{}");
                stub.serverSources = stub.serverSources.map(function (s) {
                    if (s.id !== edited) return s;
                    var next = Object.assign({}, s, { name: change.name || s.name });
                    if (change.group === "") delete next.group;
                    else if (change.group !== undefined) next.group = change.group;
                    return next;
                });
                answer = JSON.stringify({ id: edited });
            }
        }
        stub.apiResult(reqId, status, answer);
    }
}
