import QtQuick

// Calls the signed-in server's REST API through the bridge (with the saved
// token). call(method, path, body, cb) → cb(ok, data, error): `data` is the
// parsed JSON answer, `error` a message for the user when !ok.
Item {
    id: api
    visible: false
    property var pending: ({})
    property int seq: 0

    function call(method, path, body, cb) {
        seq += 1;
        var id = "q" + Date.now().toString(36) + "_" + seq + "_" + Math.floor(Math.random() * 1e6);
        pending[id] = cb;
        bridge.apiRequest(id, method, path, body === undefined || body === null ? "" : JSON.stringify(body));
    }

    Connections {
        target: bridge
        function onApiResult(reqId, status, body) {
            var cb = api.pending[reqId];
            if (!cb) return;
            delete api.pending[reqId];
            var data = null;
            try { data = body ? JSON.parse(body) : null; } catch (e) { data = null; }
            var ok = status >= 200 && status < 300;
            var err = "";
            if (!ok) {
                if (status === 0) err = I18n.t("Cannot reach the server");
                else if (data && data.error) err = I18n.t(String(data.error));
                else if (status === 401) err = I18n.t("Your sign-in has expired. Sign in again.");
                else if (status === 403) err = I18n.t("Only administrators can do this");
                else err = I18n.t("Error") + " (" + status + ")";
            }
            cb(ok, data, err);
        }
    }
}
