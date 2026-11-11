pragma Singleton
import QtQuick

// Stream setting choices, the same as the web client's
// (web/src/streamOptions.ts): keep the two in step.
QtObject {
    // value: "WxH" sent to the agent
    readonly property var resolutions: [
        { value: "1920x1080", label: "1080p" },
        { value: "1280x720", label: "720p" },
        { value: "960x540", label: "540p" }
    ]
    readonly property var fps: [240, 144, 120, 90, 75, 60, 30]
    // kbps
    readonly property var bitrates: [2000, 5000, 8000, 10000, 15000, 20000, 30000, 40000, 50000, 75000, 100000, 150000]
    // "auto": the best codec both ends handle (see the client's codec choice)
    readonly property var codecs: [
        { value: "auto", label: "Auto" },
        { value: "h264", label: "H.264" },
        { value: "h265", label: "H.265" },
        { value: "av1", label: "AV1" }
    ]
    // 4:4:4 keeps coloured text sharp; H.264 only (encoded by NVENC or the CPU)
    readonly property var chromas: [
        { value: "420", label: "4:2:0" },
        { value: "444", label: "4:4:4" }
    ]
    // bytes of mouse movement allowed to queue before old moves are dropped
    readonly property var mouseQueues: [0, 64, 256, 1024, 4096, 16384]
    readonly property var inputProtocols: [
        { value: "webrtc", label: "WebRTC" },
        { value: "webtransport", label: "WebTransport" }
    ]

    function bitrateLabel(kbps) { return (kbps / 1000) + " Mbps"; }

    function indexOfValue(list, value) {
        for (var i = 0; i < list.length; i++) {
            var v = (typeof list[i] === "object") ? list[i].value : list[i];
            if (String(v) === String(value)) return i;
        }
        return -1;
    }

    // Closest preset (bitrates, mouse queues) for a value not in the list.
    function nearestIndex(list, value) {
        var best = 0;
        for (var i = 1; i < list.length; i++)
            if (Math.abs(list[i] - value) < Math.abs(list[best] - value)) best = i;
        return best;
    }

    function resolutionIndex(res) {
        var i = indexOfValue(resolutions, res);
        return i === -1 ? 0 : i;
    }

    // The app keeps one codec string: "h264_444" is H.264 with 4:4:4 chroma.
    function codecPart(codec) {
        var c = String(codec || "auto").toLowerCase();
        if (c === "h264_444") return "h264";
        if (c === "hevc") return "h265";
        return indexOfValue(codecs, c) === -1 ? "auto" : c;
    }
    function chromaPart(codec) { return String(codec).toLowerCase() === "h264_444" ? "444" : "420"; }
    function composeCodec(codec, chroma) { return chroma === "444" ? "h264_444" : codec; }
}
