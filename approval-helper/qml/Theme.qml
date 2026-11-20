pragma Singleton
import QtQuick

// Design tokens shared with the web client (web/src/App.css) and client-qml/qml/Theme.qml.
// Theme.*       follows the OS light/dark preference (or `mode`).
// Theme.live.*  is always dark — used by the streaming UI so nothing glares
//               around the video.
QtObject {
    id: theme

    // "system" | "light" | "dark"
    property string mode: "system"
    // Qt.styleHints.colorScheme / Qt.ColorScheme only exist from Qt 6.5 on
    // (Ubuntu 24.04 ships 6.4): without them, follow "system" as dark.
    readonly property bool systemLight: Qt.ColorScheme !== undefined
        && Qt.styleHints.colorScheme === Qt.ColorScheme.Light
    readonly property bool dark: mode === "dark" || (mode !== "light" && !systemLight)

    readonly property color bg:          dark ? "#0a0a0c" : "#f6f6f7"
    readonly property color surface:     dark ? "#111114" : "#ffffff"
    readonly property color surface2:    dark ? "#18181c" : "#f4f4f5"
    readonly property color surface3:    dark ? "#212126" : "#e9e9ec"
    readonly property color textStrong:  dark ? "#fafafa" : "#09090b"
    readonly property color text:        dark ? "#e4e4e7" : "#27272a"
    readonly property color textMuted:   dark ? "#a1a1aa" : "#52525b"
    readonly property color textSubtle:  "#71717a"
    readonly property color accent:      "#2563eb"   // fills: 5.2:1 with white text
    readonly property color accentHover: "#1d4ed8"
    readonly property color accentFg:    dark ? "#60a5fa" : "#1d4ed8"   // accent text/borders
    readonly property color textOnAccent:    "#ffffff"
    readonly property color success:     "#22c55e"
    readonly property color warning:     "#f59e0b"
    readonly property color danger:      "#ef4444"
    readonly property color border:       ink(0.08)
    readonly property color borderStrong: ink(0.14)

    function ink(a) { return dark ? Qt.rgba(1, 1, 1, a) : Qt.rgba(24 / 255, 24 / 255, 27 / 255, a) }
    function accentA(a) { return dark ? Qt.rgba(59 / 255, 130 / 255, 246 / 255, a) : Qt.rgba(37 / 255, 99 / 255, 235 / 255, a) }
    function surfaceA(a) { return dark ? Qt.rgba(17 / 255, 17 / 255, 20 / 255, a) : Qt.rgba(1, 1, 1, a) }

    readonly property int radiusSm: 6
    readonly property int radius: 10
    readonly property int radiusLg: 14

    component LivePalette: QtObject {
        readonly property color bg:          "#0a0a0c"
        readonly property color surface:     "#111114"
        readonly property color surface2:    "#18181c"
        readonly property color surface3:    "#212126"
        readonly property color textStrong:  "#fafafa"
        readonly property color text:        "#e4e4e7"
        readonly property color textMuted:   "#a1a1aa"
        readonly property color textSubtle:  "#71717a"
        readonly property color accent:      "#2563eb"
        readonly property color accentHover: "#1d4ed8"
        readonly property color accentFg:    "#60a5fa"
        readonly property color textOnAccent:    "#ffffff"
        readonly property color success:     "#22c55e"
        readonly property color warning:     "#f59e0b"
        readonly property color danger:      "#ef4444"
        readonly property color border:       Qt.rgba(1, 1, 1, 0.08)
        readonly property color borderStrong: Qt.rgba(1, 1, 1, 0.14)
        function ink(a) { return Qt.rgba(1, 1, 1, a) }
        function accentA(a) { return Qt.rgba(59 / 255, 130 / 255, 246 / 255, a) }
        function surfaceA(a) { return Qt.rgba(17 / 255, 17 / 255, 20 / 255, a) }
    }

    readonly property LivePalette live: LivePalette {}
}
