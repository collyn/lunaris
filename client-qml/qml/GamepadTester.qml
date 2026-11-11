import QtQuick
import QtQuick.Layouts

// Live view of this computer's controllers (gamepad.rs through the bridge):
// every button, stick and trigger lights up as it is used, labelled the way
// the controller is (PlayStation, Xbox, Nintendo), with a vibration test.
// Same drawing as the web client's GamepadTester.
ColumnLayout {
    id: tester
    // The StreamBridge (gamepadsJson, vibrateGamepad).
    property var source: null
    property var pads: []
    property string error: ""
    spacing: 14

    // Bits of the controller state (agent/src/gamepad.rs).
    readonly property var bit: ({ a: 0x1, b: 0x2, x: 0x4, y: 0x8, up: 0x10, down: 0x20, left: 0x40, right: 0x80,
                                  lb: 0x100, rb: 0x200, start: 0x400, back: 0x800, ls: 0x1000, rs: 0x2000, guide: 0x4000, touchpad: 0x8000 })
    // Face buttons by position: bottom, right, left, top.
    readonly property var labels: ({
        playstation: { face: ["✕", "○", "□", "△"], colors: ["#5b8def", "#e5534b", "#d36fb4", "#3fb68b"], lb: "L1", rb: "R1", lt: "L2", rt: "R2", back: "Create", start: "Options", guide: "PS", ls: "L3", rs: "R3", touchpad: "Touchpad", name: "PlayStation" },
        xbox: { face: ["A", "B", "X", "Y"], colors: ["#3fb68b", "#e5534b", "#5b8def", "#e3b341"], lb: "LB", rb: "RB", lt: "LT", rt: "RT", back: "View", start: "Menu", guide: "Xbox", ls: "LS", rs: "RS", touchpad: "", name: "Xbox" },
        nintendo: { face: ["B", "A", "Y", "X"], colors: ["#9aa4b2", "#9aa4b2", "#9aa4b2", "#9aa4b2"], lb: "L", rb: "R", lt: "ZL", rt: "ZR", back: "−", start: "+", guide: "Home", ls: "LS", rs: "RS", touchpad: "", name: "Nintendo" },
        generic: { face: ["A", "B", "X", "Y"], colors: ["#3fb68b", "#e5534b", "#5b8def", "#e3b341"], lb: "LB", rb: "RB", lt: "LT", rt: "RT", back: "Back", start: "Start", guide: "Home", ls: "LS", rs: "RS", touchpad: "", name: "Generic" }
    })

    Timer {
        interval: 16
        repeat: true
        running: tester.visible && tester.source !== null
        triggeredOnStart: true
        onTriggered: {
            var text = tester.source.gamepadsJson();
            try {
                var data = JSON.parse(text);
                tester.error = data.error || "";
                // Rebuild the model only when the list of controllers changes;
                // states go to the delegates as properties.
                var pads = data.pads || [];
                var same = pads.length === tester.pads.length;
                for (var i = 0; same && i < pads.length; i++)
                    same = pads[i].slot === tester.pads[i].slot && pads[i].name === tester.pads[i].name;
                if (!same) tester.pads = pads;
                else for (var j = 0; j < pads.length; j++) repeater.itemAt(j) && (repeater.itemAt(j).pad = pads[j]);
            } catch (e) {}
        }
    }

    Rectangle {
        visible: tester.pads.length === 0
        Layout.fillWidth: true
        implicitHeight: emptyText.implicitHeight + 32
        radius: Theme.radius
        color: "transparent"
        border.color: Theme.borderStrong
        Text {
            id: emptyText
            anchors.fill: parent
            anchors.margins: 16
            wrapMode: Text.WordWrap
            color: Theme.textMuted
            font.pixelSize: 13
            text: tester.error !== "" ? I18n.t(tester.error) : I18n.t("No controller found. Connect one (USB or Bluetooth) and press any button on it.")
        }
    }

    Repeater {
        id: repeater
        model: tester.pads
        delegate: Rectangle {
            id: card
            required property var modelData
            property var pad: modelData
            readonly property var lab: tester.labels[pad.kind] || tester.labels.generic
            function on(b) { return (card.pad.buttons & b) !== 0; }
            Layout.fillWidth: true
            implicitHeight: cardColumn.implicitHeight + 28
            radius: Theme.radius
            color: Theme.surface
            border.color: Theme.border

            ColumnLayout {
                id: cardColumn
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: 14
                spacing: 8
                RowLayout {
                    Layout.fillWidth: true
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Text { text: card.pad.name; color: Theme.textStrong; font.pixelSize: 14; font.weight: Font.DemiBold; Layout.fillWidth: true; elide: Text.ElideRight }
                        Text { text: I18n.t("Controller %1").arg(card.pad.slot + 1) + " · " + card.lab.name; color: Theme.textMuted; font.pixelSize: 12 }
                    }
                    SecondaryButton {
                        text: I18n.t("Test vibration")
                        enabled: card.pad.rumble
                        onClicked: tester.source.vibrateGamepad(card.pad.slot)
                    }
                }

                // The controller, drawn on a 400 x 250 grid and scaled.
                Item {
                    Layout.fillWidth: true
                    Layout.preferredHeight: Math.min(width, 460) * 250 / 400
                    Item {
                        id: grid
                        width: 400
                        height: 250
                        anchors.horizontalCenter: parent.horizontalCenter
                        scale: Math.min(parent.width, 460) / 400
                        transformOrigin: Item.Top

                        readonly property color off: Theme.dark ? Qt.rgba(1, 1, 1, 0.08) : Qt.rgba(0, 0, 0, 0.06)
                        readonly property color line: Theme.dark ? Qt.rgba(1, 1, 1, 0.35) : Qt.rgba(0, 0, 0, 0.3)

                        // Body: grips first, the opaque body over their tops.
                        readonly property color shell: Theme.dark ? "#1b1b20" : "#f1f1f3"
                        Rectangle { x: 20; y: 150; width: 100; height: 85; radius: 45; color: grid.shell; border.color: grid.line }
                        Rectangle { x: 280; y: 150; width: 100; height: 85; radius: 45; color: grid.shell; border.color: grid.line }
                        Rectangle { x: 8; y: 50; width: 384; height: 150; radius: 60; color: grid.shell; border.color: grid.line }
                        Rectangle { x: 30; y: 190; width: 80; height: 12; color: grid.shell }
                        Rectangle { x: 290; y: 190; width: 80; height: 12; color: grid.shell }

                        // Triggers (analog) and bumpers.
                        Repeater {
                            model: [{ x: 55, v: card.pad.lt, l: card.lab.lt }, { x: 275, v: card.pad.rt, l: card.lab.rt }]
                            Rectangle {
                                required property var modelData
                                x: modelData.x; y: 8; width: 70; height: 14; radius: 7
                                color: grid.off; border.color: grid.line
                                Rectangle { width: parent.width * parent.modelData.v / 255; height: parent.height; radius: 7; color: Theme.accent }
                                Text { anchors.centerIn: parent; text: parent.modelData.l + " " + Math.round(parent.modelData.v * 100 / 255) + "%"; font.pixelSize: 10; font.weight: Font.DemiBold; color: Theme.textStrong }
                            }
                        }
                        Repeater {
                            model: [{ x: 55, b: tester.bit.lb, l: card.lab.lb }, { x: 275, b: tester.bit.rb, l: card.lab.rb }]
                            Rectangle {
                                required property var modelData
                                readonly property bool down: card.on(modelData.b)
                                x: modelData.x; y: 28; width: 70; height: 16; radius: 6
                                color: down ? Theme.accent : grid.off; border.color: grid.line
                                Text { anchors.centerIn: parent; text: parent.modelData.l; font.pixelSize: 10; font.weight: Font.DemiBold; color: parent.down ? "white" : Theme.textStrong }
                            }
                        }

                        // Touchpad (PlayStation), Back/Start, Guide.
                        Rectangle {
                            visible: card.lab.touchpad !== ""
                            x: 160; y: 58; width: 80; height: 34; radius: 6
                            color: card.on(tester.bit.touchpad) ? Theme.accent : grid.off; border.color: grid.line
                            Text { anchors.centerIn: parent; text: card.lab.touchpad; font.pixelSize: 9; color: card.on(tester.bit.touchpad) ? "white" : Theme.textMuted }
                        }
                        Repeater {
                            model: [{ cx: card.lab.touchpad !== "" ? 135 : 165, b: tester.bit.back, l: card.lab.back },
                                    { cx: card.lab.touchpad !== "" ? 265 : 235, b: tester.bit.start, l: card.lab.start }]
                            Rectangle {
                                required property var modelData
                                readonly property bool down: card.on(modelData.b)
                                x: modelData.cx - 22; y: 66; width: 44; height: 18; radius: 9
                                color: down ? Theme.accent : grid.off; border.color: grid.line
                                Text { anchors.centerIn: parent; text: parent.modelData.l; font.pixelSize: 9; font.weight: Font.DemiBold; color: parent.down ? "white" : Theme.textStrong }
                            }
                        }
                        Rectangle {
                            readonly property bool down: card.on(tester.bit.guide)
                            x: 187; y: card.lab.touchpad !== "" ? 99 : 79; width: 26; height: 26; radius: 13
                            color: down ? Theme.accent : grid.off; border.color: grid.line
                            Text { anchors.centerIn: parent; text: card.lab.guide; font.pixelSize: 8; font.weight: Font.Bold; color: parent.down ? "white" : Theme.textStrong }
                        }

                        // Sticks: the dot follows the stick, the ring lights when clicked.
                        Repeater {
                            model: [{ cx: 85, cy: 105, x: card.pad.lx, y: card.pad.ly, b: tester.bit.ls, l: card.lab.ls },
                                    { cx: 250, cy: 165, x: card.pad.rx, y: card.pad.ry, b: tester.bit.rs, l: card.lab.rs }]
                            Item {
                                required property var modelData
                                x: modelData.cx - 30; y: modelData.cy - 30; width: 60; height: 76
                                Rectangle { width: 60; height: 60; radius: 30; color: card.on(parent.modelData.b) ? Theme.accent : grid.off; border.color: grid.line; border.width: 1.5 }
                                Rectangle {
                                    width: 24; height: 24; radius: 12
                                    x: 18 + parent.modelData.x / 32767 * 18
                                    y: 18 + parent.modelData.y / 32767 * 18
                                    color: Theme.surface; border.color: grid.line; border.width: 1.5
                                }
                                Text { y: 62; anchors.horizontalCenter: parent.horizontalCenter; text: parent.modelData.l; font.pixelSize: 11; color: Theme.textMuted }
                            }
                        }

                        // D-pad.
                        Repeater {
                            model: [{ x: 129, y: 136, w: 22, h: 24, b: tester.bit.up }, { x: 129, y: 182, w: 22, h: 24, b: tester.bit.down },
                                    { x: 105, y: 160, w: 24, h: 22, b: tester.bit.left }, { x: 151, y: 160, w: 24, h: 22, b: tester.bit.right }]
                            Rectangle {
                                required property var modelData
                                x: modelData.x; y: modelData.y; width: modelData.w; height: modelData.h; radius: 3
                                color: card.on(modelData.b) ? Theme.accent : grid.off; border.color: grid.line
                            }
                        }

                        // Face buttons: bottom, right, left, top.
                        Repeater {
                            model: [{ cx: 315, cy: 135, b: tester.bit.a, i: 0 }, { cx: 345, cy: 105, b: tester.bit.b, i: 1 },
                                    { cx: 285, cy: 105, b: tester.bit.x, i: 2 }, { cx: 315, cy: 75, b: tester.bit.y, i: 3 }]
                            Rectangle {
                                required property var modelData
                                readonly property bool down: card.on(modelData.b)
                                readonly property color tint: card.lab.colors[modelData.i]
                                x: modelData.cx - 14; y: modelData.cy - 14; width: 28; height: 28; radius: 14
                                color: down ? tint : grid.off; border.color: tint; border.width: 2
                                Text { anchors.centerIn: parent; text: card.lab.face[parent.modelData.i]; font.pixelSize: 13; font.weight: Font.Bold; color: parent.down ? "white" : parent.tint }
                            }
                        }
                    }
                }

                Text {
                    Layout.alignment: Qt.AlignHCenter
                    color: Theme.textMuted
                    font.pixelSize: 12
                    text: I18n.t("Left stick") + " " + (card.pad.lx / 32767).toFixed(2) + ", " + (card.pad.ly / 32767).toFixed(2)
                          + "    " + I18n.t("Right stick") + " " + (card.pad.rx / 32767).toFixed(2) + ", " + (card.pad.ry / 32767).toFixed(2)
                }
            }
        }
    }
}
