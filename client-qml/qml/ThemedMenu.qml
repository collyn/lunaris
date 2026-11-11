import QtQuick
import QtQuick.Controls

// Menu in the app's colors.
Menu {
    id: menu
    palette.window: Theme.surface
    palette.base: Theme.surface
    palette.windowText: Theme.text
    palette.text: Theme.text
    palette.buttonText: Theme.text
    palette.highlight: Theme.accent
    palette.highlightedText: Theme.textOnAccent
    palette.mid: Theme.borderStrong
    palette.dark: Theme.borderStrong
    palette.light: Theme.surface2
    background: Rectangle {
        implicitWidth: 190
        radius: Theme.radiusSm
        color: Theme.surface
        border.color: Theme.borderStrong
    }
}
