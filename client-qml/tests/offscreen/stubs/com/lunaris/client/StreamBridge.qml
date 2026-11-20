import QtQuick
import "../../../.."

// Stands in for the Rust StreamBridge type so that main.qml itself loads
// off screen: StubBridge plus the stream side, which does nothing but note
// what was asked for. A test finds it by its object name and plays the
// host's part by emitting its signals.
StubBridge {
    id: stub
    objectName: "bridge"

    property int stopped: 0

    signal statsUpdated(real ping, real decode, real frames, real bit, string requestedCodec, string negotiatedCodec, string decodeBackend, string presentBackend, bool gpuDecodeEnabled, string fallbackReason, int packetLossEvents, int packetsLost, int queueLagEvents, int maxQueueLag, string connectionType)
    signal hostCursorUpdated(int x, int y, bool visible, string kind, bool inWindowMoveSize)
    signal pointerInPictureUpdated(bool maybe, bool found)
    signal pointerAlwaysInPictureUpdated(bool always)
    signal hostCursorImageUpdated(string kind, string source, int width, int height, int hotspotX, int hotspotY)
    signal hostOsUpdated(string hostOs)
    signal hostInfoUpdated(string gpuInfo, string hostOs)
    signal encoderStatusUpdated(string encoder, string hwType, string gpuInfo, string requestedEncoder, string hostOs, string displayId, string displayName)
    signal localCursorDelta(int rx, int ry)
    signal newVersionAvailable(string version, string url)
    signal settingsLoaded(string res, int fps, string codec, int bitrate, int queueLimit, string hostName, bool disableCuda, string inputProtocol, string renderBackend)
    signal deeplinkReceived(string url)
    signal shareStatus(string message)
    signal streamFailed(string message)
    signal sourceStatus(string status, string reason)
    signal sourceDisplaysUpdated(string displaysJson, string current)

    property int started: 0
    function startStream() { stub.started += 1; }
    function stopStream() { stub.stopped += 1; }
    function setVideoSink(sink) {}
    function requestSettings() {}
    // The stream settings asked for, newest last: { res, fps, codec, bitrate, queueLimit, renderBackend, inputProtocol }.
    property var configs: []
    function updateStreamConfigWithBackend(res, fps, codec, bitrate, queueLimit, disableCuda, renderBackend, inputProtocol) {
        stub.configs = stub.configs.concat([{ res: res, fps: fps, codec: codec, bitrate: bitrate, queueLimit: queueLimit, renderBackend: renderBackend, inputProtocol: inputProtocol }]);
    }
    function pollStats() {}
    function pollCursor() {}
    function pollEvents() {}
    function hasConnectionArgs() { return false; }
    function pendingLinkInfo() { return ""; }
    function answerPendingLink(accept) { return false; }
    function sendMouseMove(x, y, w, h, rx, ry, locked) {}
    function sendMouseClick(button, down) {}
    function sendMouseWheel(x, y) {}
    // The key events sent by Qt key code, newest last: [key, down].
    property var keyEvents: []
    function sendKeyEvent(key, modifiers, down) { stub.keyEvents = stub.keyEvents.concat([[key, down]]); }
    function sendKeyEventAt(key, modifiers, nativeScanCode, down, autoRepeat) {}
    // How often the held keys were let go on the remote computer.
    property int keysReleased: 0
    function releaseAllKeys() { stub.keysReleased += 1; }
    // What was asked of the window system, newest last (true: the system's shortcuts).
    property var keyboardGrabs: []
    property string systemKeys: "grab"
    function systemKeysSupport() { return stub.systemKeys; }
    function warpCursor(x, y) {}
    function setKeyboardGrab(grab) { stub.keyboardGrabs = stub.keyboardGrabs.concat([grab]); }
    function setPointerLocked(locked) {}
    // What the session settings applied last.
    property var audio: ({ on: true, volume: 100 })
    property bool microphone: false
    property var share: ({ clipboard: true, files: true })
    property bool gamepad: true
    function setAudio(on, volume) { stub.audio = { on: on, volume: volume }; }
    function setRecordMicrophone(on) { stub.microphone = on; }
    function setShareOptions(clipboard, files) { stub.share = { clipboard: clipboard, files: files }; }
    function setGamepadEnabled(on) { stub.gamepad = on; }
    function gamepadsJson() { return "[]"; }
    function vibrateGamepad(slot) {}
    function filesOpenCurrent() { return false; }
    function filesClose() {}
    function filesVersion() { return 0; }
    function filesState() { return "{}"; }
    function filesListRemote(path, hidden) {}
    function filesRemoteOp(op, path, name) {}
    function filesLocalPlaces() { return JSON.stringify({ home: "/" }); }
    function filesListLocal(path, hidden) { return "[]"; }
    function filesLocalOp(op, path, name) { return ""; }
    function filesUpload(paths, dir) {}
    function filesUploadDropped(paths) {}
    function filesDownload(names, remoteDir, localDir) {}
    function filesCancel(id) {}
    function filesClearFinished() {}
}
