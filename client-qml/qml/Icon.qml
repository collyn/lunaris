import QtQuick
import QtQuick.Shapes

// Line icons drawn from SVG path data on a 24x24 grid (no SVG image plugin
// needed), stroked in `color`.
Item {
    id: icon
    property string name: ""
    property color color: Theme.text
    property real size: 18
    property real strokeWidth: 1.8

    implicitWidth: size
    implicitHeight: size
    width: size
    height: size
    // Smooth strokes (works on Qt 6.4, which lacks Shape.CurveRenderer).
    layer.enabled: true
    layer.samples: 4

    readonly property var paths: ({
        "monitor": "M3 4h18v12H3z M8 20h8 M12 16v4",
        "clock": "M12 3a9 9 0 1 0 0.01 0z M12 7v5l3 2",
        "star": "M12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1L3.2 9.5l6.1-.9z",
        "server": "M4 4h16v6H4z M4 14h16v6H4z M8 7h.01 M8 17h.01",
        "settings": "M12 8.5a3.5 3.5 0 1 0 0.01 0z M12 2.5v2.5 M12 19v2.5 M2.5 12H5 M19 12h2.5 M5.3 5.3l1.8 1.8 M16.9 16.9l1.8 1.8 M5.3 18.7l1.8-1.8 M16.9 7.1l1.8-1.8",
        "home": "M3 11l9-7 9 7 M5 9.5V20h14V9.5 M10 20v-6h4v6",
        "copy": "M9 9h11v11H9z M5 15H4V4h11v1",
        "refresh": "M20 12a8 8 0 1 1-2.3-5.6 M20 4v5h-5",
        "edit": "M4 20h4L19 9l-4-4L4 16z M13.5 6.5l4 4",
        "eye": "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z M12 9a3 3 0 1 0 0.01 0z",
        "eyeOff": "M3 3l18 18 M10.6 5.1A10 10 0 0 1 12 5c6.5 0 10 7 10 7a17 17 0 0 1-3 3.9 M6.6 6.6C3.8 8.4 2 12 2 12s3.5 7 10 7a9.7 9.7 0 0 0 4.4-1 M9.9 9.9a3 3 0 0 0 4.2 4.2",
        "more": "M12 5h.01 M12 12h.01 M12 19h.01",
        "search": "M11 4a7 7 0 1 0 0.01 0z M20 20l-4-4",
        "close": "M6 6l12 12 M18 6L6 18",
        "chevronDown": "M6 9l6 6 6-6",
        "shield": "M12 3l8 3v6c0 4.5-3.4 8.3-8 9-4.6-.7-8-4.5-8-9V6z",
        "link": "M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1 M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1",
        "info": "M12 3a9 9 0 1 0 0.01 0z M12 11v5 M12 8h.01",
        "globe": "M12 3a9 9 0 1 0 0.01 0z M3 12h18 M12 3c2.5 2.5 3.5 5.5 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-5.5-3.5-9s1-6.5 3.5-9z",
        "plus": "M12 5v14 M5 12h14",
        "trash": "M4 7h16 M9 7V4h6v3 M6 7l1 13h10l1-13",
        "power": "M12 3v8 M7 6.5a7 7 0 1 0 10 0",
        "check": "M5 12l5 5 9-10",
        "arrowRight": "M5 12h14 M13 6l6 6-6 6",
        "logout": "M15 4h4v16h-4 M10 8l-4 4 4 4 M6 12h10",
        "user": "M12 4a4 4 0 1 0 0.01 0z M4 21c0-4 3.6-6 8-6s8 2 8 6",
        "grid": "M4 4h7v7H4z M13 4h7v7h-7z M4 13h7v7H4z M13 13h7v7h-7z",
        "list": "M8 6h12 M8 12h12 M8 18h12 M4 6h.01 M4 12h.01 M4 18h.01",
        "volume": "M4 9v6h4l5 4V5L8 9H4z M16 9a3.5 3.5 0 0 1 0 6 M18.5 6.5a7 7 0 0 1 0 11",
        "upload": "M12 16V4 M7 9l5-5 5 5 M4 16v4h16v-4",
        "clipboard": "M9 4h6v3H9z M7 5H5v16h14V5h-2",
        "folder": "M3 6a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z",
        "file": "M6 3h8l4 4v14H6z M14 3v4h4",
        "arrowUp": "M12 19V5 M5 12l7-7 7 7",
        "gamepad": "M7 7h10a5 5 0 0 1 4.9 6l-.8 4a2.5 2.5 0 0 1-4.4 1.1L15 16H9l-1.7 2.1a2.5 2.5 0 0 1-4.4-1.1l-.8-4A5 5 0 0 1 7 7z M7.5 10v3 M6 11.5h3 M15.5 11h.01 M17.5 13h.01"
    })

    Shape {
        anchors.centerIn: parent
        width: 24
        height: 24
        scale: icon.size / 24
        ShapePath {
            strokeColor: icon.color
            strokeWidth: icon.strokeWidth
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathSvg { path: icon.paths[icon.name] || "" }
        }
    }
}
