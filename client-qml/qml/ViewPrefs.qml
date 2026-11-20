pragma Singleton
import QtQuick

// Settings of this device's viewing, not of a remote computer (the session
// menu's settings, SessionSettings.qml): applied right away and saved by
// main.qml (store "view_prefs"). The web client keeps the
// same ones in its browser (streamOptions.ts).
QtObject {
    // "fit" whole picture | "fill" cover the window | "stretch" | "native" 1:1
    property string displayMode: "fit"
    property bool audioEnabled: true
    // 0-100
    property int volume: 100
    // Ask the remote computer to record its microphone into the sound too.
    property bool recordMicrophone: false
    // Clipboard both ways and sending files with the remote computer.
    property bool clipboardSync: true
    property bool fileTransfer: true
    // Send this device's game controllers to the remote computer.
    property bool gamepad: true
    // Key combinations this computer's desktop keeps for itself (Alt+Tab,
    // the Super key, ...) go to the remote computer: "off" never,
    // "fullscreen" in full screen, "always" while the pointer is over the
    // picture. The first two are moonlight-qt's modes (its default is off,
    // streamingpreferences.cpp 155-156); "always" is how RustDesk works, and
    // the default here, as for a remote desktop.
    property string systemKeys: "always"

    readonly property var displayModes: [
        { value: "fit", label: "Fit window" },
        { value: "fill", label: "Fill window" },
        { value: "stretch", label: "Stretch" },
        { value: "native", label: "Original size" }
    ]

    function toJson() {
        return JSON.stringify({ displayMode: displayMode, audioEnabled: audioEnabled, volume: volume,
                                recordMicrophone: recordMicrophone, clipboardSync: clipboardSync, fileTransfer: fileTransfer, gamepad: gamepad,
                                systemKeys: systemKeys });
    }
    function fromJson(text) {
        if (!text) return;
        try {
            var p = JSON.parse(text);
            if (["fit", "fill", "stretch", "native"].indexOf(p.displayMode) !== -1) displayMode = p.displayMode;
            if (typeof p.audioEnabled === "boolean") audioEnabled = p.audioEnabled;
            if (typeof p.volume === "number") volume = Math.max(0, Math.min(100, Math.round(p.volume)));
            if (typeof p.recordMicrophone === "boolean") recordMicrophone = p.recordMicrophone;
            if (typeof p.clipboardSync === "boolean") clipboardSync = p.clipboardSync;
            if (typeof p.fileTransfer === "boolean") fileTransfer = p.fileTransfer;
            if (typeof p.gamepad === "boolean") gamepad = p.gamepad;
            if (["off", "fullscreen", "always"].indexOf(p.systemKeys) !== -1) systemKeys = p.systemKeys;
        } catch (e) { console.log("view_prefs: " + e); }
    }
}
