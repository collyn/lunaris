import QtQuick
import QtQuick.Controls

Rectangle {
    id: settingsRoot
    width: 480
    height: 480
    radius: 16
    color: Qt.rgba(15/255, 22/255, 38/255, 0.92)
    border.color: Qt.rgba(0, 240/255, 255/255, 0.3)
    border.width: 1.5
    visible: false

    // Signals
    signal applySettings(string res, int fps, string codec, int bitrate, int queueLimit)

    function open() {
        settingsRoot.visible = true;
    }

    function close() {
        settingsRoot.visible = false;
    }

    function setCurrentSettings(res, fps, codec, bitrate, queueLimit) {
        if (res.indexOf("1920") !== -1 || res.indexOf("1080") !== -1) {
            resCombo.currentIndex = 0;
        } else if (res.indexOf("1280") !== -1 || res.indexOf("720") !== -1) {
            resCombo.currentIndex = 1;
        } else if (res.indexOf("960") !== -1 || res.indexOf("540") !== -1) {
            resCombo.currentIndex = 2;
        }

        var fpsVal = parseInt(fps);
        if (fpsVal === 240) fpsCombo.currentIndex = 0;
        else if (fpsVal === 144) fpsCombo.currentIndex = 1;
        else if (fpsVal === 120) fpsCombo.currentIndex = 2;
        else if (fpsVal === 90) fpsCombo.currentIndex = 3;
        else if (fpsVal === 60) fpsCombo.currentIndex = 4;
        else if (fpsVal === 30) fpsCombo.currentIndex = 5;
        else fpsCombo.currentIndex = 4; // Default to 60

        var codecLower = codec.toLowerCase();
        if (codecLower.indexOf("264") !== -1) {
            codecCombo.currentIndex = 0;
        } else if (codecLower.indexOf("265") !== -1 || codecLower.indexOf("hevc") !== -1) {
            codecCombo.currentIndex = 1;
        } else if (codecLower.indexOf("av1") !== -1) {
            codecCombo.currentIndex = 2;
        }

        bitrateSpin.value = bitrate;

        var qlVal = parseInt(queueLimit);
        if (qlVal === 0) queueCombo.currentIndex = 0;
        else if (qlVal === 64) queueCombo.currentIndex = 1;
        else if (qlVal === 256) queueCombo.currentIndex = 2;
        else if (qlVal === 1024) queueCombo.currentIndex = 3;
        else if (qlVal === 4096) queueCombo.currentIndex = 4;
        else if (qlVal === 16384) queueCombo.currentIndex = 5;
        else queueCombo.currentIndex = 2; // Default to 256
    }

    // Modal overlay blocker
    MouseArea {
        anchors.fill: parent
        propagateComposedEvents: false
    }

    // Gradient bar at the top for premium look
    Rectangle {
        id: accentBar
        width: parent.width
        height: 4
        anchors.top: parent.top
        radius: 16
        color: "#00f0ff"
        
        // Hide the rounded bottom corners of the accent bar by overlaying a rect
        Rectangle {
            width: parent.width
            height: 2
            anchors.bottom: parent.bottom
            color: "#00f0ff"
        }
    }

    // Header
    Text {
        id: headerTitle
        text: "STREAM CONFIGURATION"
        font.pixelSize: 15
        font.bold: true
        font.letterSpacing: 1.5
        color: "#00f0ff"
        anchors.top: parent.top
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 24
    }

    // Subtitle
    Text {
        id: headerSubtitle
        text: "Adjust settings to optimize latency and video quality"
        font.pixelSize: 11
        color: "#94a3b8"
        anchors.top: headerTitle.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 6
    }

    // Grid layout for fields
    Grid {
        anchors.top: headerSubtitle.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: buttonRow.top
        anchors.margins: 32
        columns: 2
        spacing: 20
        verticalItemAlignment: Grid.AlignVCenter

        // Resolution
        Text { text: "Resolution:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: resCombo
            width: 260
            model: ["1080p (1920x1080)", "720p (1280x720)", "540p (960x540)"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: resCombo.width
                contentItem: Text {
                    text: modelData
                    color: resCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: resCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: resCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: resCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: resCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: resCombo.height + 4
                width: resCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: resCombo.popup.visible ? resCombo.delegateModel : null
                    currentIndex: resCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // FPS
        Text { text: "Target FPS:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: fpsCombo
            width: 260
            model: ["240", "144", "120", "90", "60", "30"]
            currentIndex: 4

            delegate: ItemDelegate {
                width: fpsCombo.width
                contentItem: Text {
                    text: modelData
                    color: fpsCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: fpsCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: fpsCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: fpsCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: fpsCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: fpsCombo.height + 4
                width: fpsCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: fpsCombo.popup.visible ? fpsCombo.delegateModel : null
                    currentIndex: fpsCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Codec
        Text { text: "Video Codec:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: codecCombo
            width: 260
            model: ["H264", "H265", "AV1"]
            currentIndex: 0

            delegate: ItemDelegate {
                width: codecCombo.width
                contentItem: Text {
                    text: modelData
                    color: codecCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: codecCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: codecCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: codecCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: codecCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: codecCombo.height + 4
                width: codecCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: codecCombo.popup.visible ? codecCombo.delegateModel : null
                    currentIndex: codecCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }

        // Bitrate
        Text { text: "Bitrate (Kbps):"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        SpinBox {
            id: bitrateSpin
            width: 260
            from: 1000
            to: 150000
            stepSize: 1000
            value: 8000
            editable: true

            contentItem: TextInput {
                z: 2
                text: bitrateSpin.value + " Kbps"
                font.pixelSize: 13
                color: "#ffffff"
                selectionColor: "#4f46e5"
                selectedTextColor: "#ffffff"
                horizontalAlignment: Qt.AlignHCenter
                verticalAlignment: Qt.AlignVCenter
                readOnly: !bitrateSpin.editable
                validator: bitrateSpin.validator
                inputMethodHints: Qt.ImhFormattedNumbersOnly
            }

            up.indicator: Rectangle {
                x: bitrateSpin.width - width
                height: bitrateSpin.height
                width: 36
                radius: 8
                color: bitrateSpin.up.pressed ? Qt.rgba(99/255, 102/255, 241/255, 0.2) : "transparent"
                
                Text {
                    text: "+"
                    color: bitrateSpin.up.hovered ? "#00f0ff" : "#cbd5e1"
                    font.pixelSize: 15
                    font.bold: true
                    anchors.centerIn: parent
                }
            }

            down.indicator: Rectangle {
                x: 0
                height: bitrateSpin.height
                width: 36
                radius: 8
                color: bitrateSpin.down.pressed ? Qt.rgba(99/255, 102/255, 241/255, 0.2) : "transparent"
                
                Text {
                    text: "-"
                    color: bitrateSpin.down.hovered ? "#00f0ff" : "#cbd5e1"
                    font.pixelSize: 15
                    font.bold: true
                    anchors.centerIn: parent
                }
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: bitrateSpin.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }
        }

        // Mouse Queue Limit
        Text { text: "Mouse Queue:"; color: "#cbd5e1"; font.pixelSize: 13; font.bold: true; width: 120 }
        ComboBox {
            id: queueCombo
            width: 260
            model: [
                "0 B (Strict No Queue)",
                "64 B (Ultra Low Buffer)",
                "256 B (Recommended)",
                "1024 B (Moderate Buffer)",
                "4096 B (High Buffer)",
                "16384 B (Previous Default)"
            ]
            currentIndex: 2

            delegate: ItemDelegate {
                width: queueCombo.width
                contentItem: Text {
                    text: modelData
                    color: queueCombo.highlightedIndex === index ? "#ffffff" : "#cbd5e1"
                    font.pixelSize: 13
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: queueCombo.highlightedIndex === index ? "#4f46e5" : "transparent"
                }
                highlighted: queueCombo.highlightedIndex === index
            }

            contentItem: Text {
                leftPadding: 12
                text: queueCombo.displayText
                font.pixelSize: 13
                color: "#ffffff"
                verticalAlignment: Text.AlignVCenter
            }

            background: Rectangle {
                implicitHeight: 38
                color: "#0f1626"
                border.color: queueCombo.activeFocus ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.08)
                border.width: 1
                radius: 8
            }

            popup: Popup {
                y: queueCombo.height + 4
                width: queueCombo.width
                implicitHeight: contentItem.implicitHeight
                padding: 1

                contentItem: ListView {
                    clip: true
                    implicitHeight: contentHeight
                    model: queueCombo.popup.visible ? queueCombo.delegateModel : null
                    currentIndex: queueCombo.highlightedIndex
                }

                background: Rectangle {
                    color: "#0f1626"
                    border.color: Qt.rgba(1, 1, 1, 0.15)
                    border.width: 1
                    radius: 8
                }
            }
        }
    }

    // Button Row
    Row {
        id: buttonRow
        anchors.bottom: parent.bottom
        anchors.right: parent.right
        anchors.margins: 28
        spacing: 12

        Button {
            id: cancelButton
            text: "Cancel"
            onClicked: {
                settingsRoot.close();
            }
            background: Rectangle {
                color: cancelButton.hovered ? Qt.rgba(1, 1, 1, 0.05) : "transparent"
                border.color: cancelButton.hovered ? "#00f0ff" : Qt.rgba(1, 1, 1, 0.2)
                border.width: 1
                radius: 8
                implicitWidth: 100
                implicitHeight: 36
            }
            contentItem: Text {
                text: cancelButton.text
                color: cancelButton.hovered ? "#00f0ff" : "#cbd5e1"
                font.bold: true
                font.pixelSize: 13
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }

        Button {
            id: applyButton
            text: "Save & Apply"
            onClicked: {
                var selectedRes = "1920x1080"
                if (resCombo.currentIndex === 1) selectedRes = "1280x720"
                else if (resCombo.currentIndex === 2) selectedRes = "960x540"

                var fps = parseInt(fpsCombo.currentText)
                var codec = codecCombo.currentText.toLowerCase()
                var bitrate = bitrateSpin.value

                var queueLimit = 256
                if (queueCombo.currentIndex === 0) queueLimit = 0
                else if (queueCombo.currentIndex === 1) queueLimit = 64
                else if (queueCombo.currentIndex === 2) queueLimit = 256
                else if (queueCombo.currentIndex === 3) queueLimit = 1024
                else if (queueCombo.currentIndex === 4) queueLimit = 4096
                else if (queueCombo.currentIndex === 5) queueLimit = 16384

                settingsRoot.applySettings(selectedRes, fps, codec, bitrate, queueLimit);
                settingsRoot.close();
            }
            background: Rectangle {
                color: applyButton.hovered ? "#00e0ff" : "#00f0ff"
                radius: 8
                implicitWidth: 120
                implicitHeight: 36
            }
            contentItem: Text {
                text: applyButton.text
                color: "#080c14"
                font.bold: true
                font.pixelSize: 13
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }
    }
}
