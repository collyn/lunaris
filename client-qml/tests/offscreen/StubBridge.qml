import QtQuick

// Stands in for the Rust `bridge` (StreamBridge) when the pages are
// rendered off screen: the functions and signals of the window shown
// without a link (Launcher.qml), the update notice and the session
// settings, answering from canned data. Nothing leaves this process and
// nothing is stored.
QtObject {
    id: stub

    // ---- What the tests set ----
    // The JSON stores (`loadStore`/`saveStore`): { name: json }.
    property var stores: ({})
    // The trusted servers, as the bridge lists them: [{ server, web }].
    property var trusted: []
    // Web pages "opened" are not: the tests read what was asked.
    property var added: []

    signal linkFailed(string message)
    signal capabilitiesUpdated(string displaysJson, string encodersJson, string gpuInfo, string hostOs, string sourcesJson)

    // ---- Stores and status ----
    function loadStore(name) { return stub.stores[name] || ""; }
    function saveStore(name, json) { var next = Object.assign({}, stub.stores); next[name] = json; stub.stores = next; }
    function appVersion() { return "0.5.0"; }
    function screenRefreshRate() { return 60; }

    // ---- Trusted servers (deeplink.rs) ----
    function trustedServers() { return JSON.stringify(stub.trusted); }
    // A server address: "https://" when no scheme is given; "" for what is
    // not one (as the bridge answers).
    function addTrustedServer(address) {
        var a = String(address).trim();
        if (a === "" || a.indexOf(" ") !== -1) return "";
        var web = a.indexOf("://") === -1 ? "https://" + a : a;
        if (!/^https?:\/\/[A-Za-z0-9.\[\]:-]+\/?$/.test(web)) return "";
        web = web.replace(/\/$/, "");
        stub.added = stub.added.concat([web]);
        stub.trusted = [{ server: web, web: web }].concat(stub.trusted.filter(function (s) { return s.web !== web; }));
        return web;
    }
    function removeTrustedServer(server) {
        var before = stub.trusted.length;
        stub.trusted = stub.trusted.filter(function (s) { return s.server !== server; });
        return stub.trusted.length !== before;
    }

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
}
