pragma Singleton
import QtQuick

// Settings of this device's viewing, not of a remote computer: applied right
// away and saved by main.qml (store "view_prefs"). The web client keeps the
// same ones in its browser (streamOptions.ts).
QtObject {
    // "fit" whole picture | "fill" cover the window | "stretch" | "native" 1:1
    property string displayMode: "fit"
    property bool audioEnabled: true
    // 0-100
    property int volume: 100
    // Clipboard both ways and sending files with the remote computer.
    property bool clipboardSync: true
    property bool fileTransfer: true
    // Send this device's game controllers to the remote computer.
    property bool gamepad: true

    readonly property var displayModes: [
        { value: "fit", label: "Fit window" },
        { value: "fill", label: "Fill window" },
        { value: "stretch", label: "Stretch" },
        { value: "native", label: "Original size" }
    ]

    function toJson() {
        return JSON.stringify({ displayMode: displayMode, audioEnabled: audioEnabled, volume: volume,
                                clipboardSync: clipboardSync, fileTransfer: fileTransfer, gamepad: gamepad });
    }
    function fromJson(text) {
        if (!text) return;
        try {
            var p = JSON.parse(text);
            if (["fit", "fill", "stretch", "native"].indexOf(p.displayMode) !== -1) displayMode = p.displayMode;
            if (typeof p.audioEnabled === "boolean") audioEnabled = p.audioEnabled;
            if (typeof p.volume === "number") volume = Math.max(0, Math.min(100, Math.round(p.volume)));
            if (typeof p.clipboardSync === "boolean") clipboardSync = p.clipboardSync;
            if (typeof p.fileTransfer === "boolean") fileTransfer = p.fileTransfer;
            if (typeof p.gamepad === "boolean") gamepad = p.gamepad;
        } catch (e) { console.log("view_prefs: " + e); }
    }
}
