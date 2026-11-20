import QtQuick
import QtQuick.Controls as QQC
import QtQuick.Layouts
import "Cards.js" as Cards

// "Add computer" and "Edit": the Type, then that type's form. Adding on a
// wide window, the types are a list at the left (each a row with its logo
// to click) and the chosen one's form is at the right, under what that type
// says about itself; on a narrow window (and to edit) the types are chips
// in a row above the form.
//
// "Lunaris" is the form there always was (address, password). Every other
// type is a connector, described by data: { id, name, fields: [{ key,
// label, kind, default, required, options }] } with kind "text", "port",
// "secret" or "choice" (one of `options`: [{ value, label }], shown as
// that many options to click). Nothing here knows a particular connector
// (a type's logo and short name are looked up by its id, Cards.js).
//
// On a wide window short fields share a row (Cards.layoutFields); on a
// narrow one every field has its own. A choice is options to click, or a
// drop-down when they are many or long (Cards.choiceAsList). The buttons
// stay in sight while a long form scrolls (and a long list of types, on
// its own).
//
// The dialog checks what was typed and hands it over (lunarisSubmitted /
// sourceSubmitted); the owner saves it, and closes the dialog or sets
// `error`. What was typed is dropped when the dialog closes.
//
// A type can have a set-up of its own (a pairing: its connector makes a PIN
// that is typed at the other computer). The owner then calls setupShow()
// with what the connector wants shown; the dialog shows it, asks the owner
// once a second how it stands (setupAsked; answered with setupUpdate(),
// setupFailed() or setupFinished()), and tells it when it is closed while
// the set-up still waits (setupCancelled).
//
// A computer can be listed in more than one way (Cards.js). "Add a
// connection method" (openAddMethod) adds one more way to a card that is
// there; "Edit" can say which card a way belongs to ("Same computer as").
// Either way the dialog only says which card: `sourceSubmitted` carries a
// `group` that is "*" (nothing said), "" (a computer of its own), a group,
// or "@<card key>" for a card that has no group yet (the owner makes one).
//
// What is shown here can come from another computer or a server (names, a
// connector's note, a set-up's message, an error): every such text is
// plain text.
DialogFrame {
    id: dlg
    // Wide where there is room, never wider than the window (wider when
    // adding: the list of types stands beside the form).
    width: Math.max(300, Math.min(editing ? 600 : 820, (QQC.Overlay.overlay ? QQC.Overlay.overlay.width : 640) - 40))
    // Adding with room for both: the types at the left, the form at the right.
    readonly property bool twoPane: !dlg.editing && dlg.availableWidth >= 640
    readonly property int railWidth: 176
    // Short fields share a row from this width on (the form's part of a
    // two-pane dialog is always that wide).
    readonly property bool wide: dlg.twoPane || dlg.availableWidth >= 480
    // Not closed by a click beside it while a set-up waits for its PIN.
    closePolicy: dlg.setupWaiting ? QQC.Popup.CloseOnEscape : (QQC.Popup.CloseOnEscape | QQC.Popup.CloseOnPressOutside)

    // The connector types to offer besides Lunaris.
    property var connectors: []
    // Server tab: the computer is added to a server, and a connector type
    // is reached through one of its Lunaris computers. `through` lists
    // them with what each offers: [{ host_id, host_name, connectors }].
    property bool serverMode: false
    property var through: []

    property bool editing: false
    // "lunaris" or a connector's id.
    property string typeId: "lunaris"
    property string throughHost: ""
    property string sourceId: ""
    property string editName: ""
    property string editTypeName: ""
    property string editThroughName: ""
    // Edit: the stored values that may be shown (never a secret).
    property var editValues: ({})
    // Edit: whether `editValues` are what is stored (a server from before
    // this sends none: the form then starts empty and keeps what is left so).
    property bool editValuesKnown: false
    property string error: ""
    property bool busy: false

    signal lunarisSubmitted(string address, string password, string name)
    // `sourceId` is "" when adding. `values` is { "<field key>": "<text>" };
    // when editing, a field left empty is left out (it keeps its value).
    // `group`: see the top of this file.
    signal sourceSubmitted(string connectorId, string hostId, string sourceId, string name, var values, string group)

    // "Add a connection method": the card it is added to ({ key, title,
    // group }), or null.
    property var methodCard: null
    readonly property bool addingMethod: methodCard !== null && !editing
    // Edit: the cards it can be said to be the same computer as,
    // [{ value: "<card key>", label, group }], and the one it is on now
    // ("" = a computer of its own).
    property var sameOptions: []
    property string sameCurrent: ""
    property string sameChosen: ""
    // The group to hand over with what was typed.
    function groupSaid() {
        if (addingMethod) return validWord(methodCard.group) ? methodCard.group : "@" + methodCard.key;
        if (!editing || sameChosen === sameCurrent) return "*";
        if (sameChosen === "") return "";
        for (var i = 0; i < sameOptions.length; i++)
            if (sameOptions[i].value === sameChosen)
                return validWord(sameOptions[i].group) ? sameOptions[i].group : "@" + sameChosen;
        return "*";
    }
    function validWord(group) { return typeof group === "string" && /^[A-Za-z0-9._:-]{1,64}$/.test(group); }

    // ---- The set-up of the computer being added ----
    // "" when there is none. `setupHost` is the Lunaris computer it runs
    // on (server tab), `setupName` the new computer's name.
    property string setupId: ""
    property string setupHost: ""
    property string setupName: ""
    property string setupType: ""
    // What the connector wants shown, in its own words, and its code (a PIN).
    property string setupMessage: ""
    property string setupCode: ""
    // Why it did not work, when it did not.
    property string setupError: ""
    readonly property bool setupShown: setupId !== ""
    readonly property bool setupWaiting: setupId !== "" && setupError === ""

    // Asked once a second while the set-up waits.
    signal setupAsked(string setup, string hostId)
    // The dialog was closed while the set-up waited: it is to be stopped.
    signal setupCancelled(string setup, string hostId)

    function setupShow(setup, hostId, message, code) {
        busy = false;
        error = "";
        setupError = "";
        setupHost = hostId || "";
        setupName = nameField.text.trim();
        setupType = typeId;
        setupMessage = String(message || "");
        setupCode = String(code || "");
        setupId = String(setup || "");
    }
    function setupUpdate(message, code) {
        setupMessage = String(message || "");
        setupCode = String(code || "");
    }
    function setupFailed(message) {
        if (setupId === "") return;
        setupError = String(message || "") !== "" ? String(message) : I18n.t("The computer could not be added");
    }
    // Done (the computer is added): nothing is left to stop.
    function setupFinished() {
        setupId = "";
        setupError = "";
    }
    // Back to the form, as it was filled in.
    function setupBack() {
        setupId = "";
        setupError = "";
    }

    Timer {
        interval: 1000
        repeat: true
        running: dlg.visible && dlg.setupWaiting
        onTriggered: dlg.setupAsked(dlg.setupId, dlg.setupHost)
    }

    readonly property bool isLunaris: typeId === "lunaris"
    // The types to choose from: { value, label, note }. A way to connect is
    // added to a card that is there through a connector only (a Lunaris
    // computer is a card of its own).
    // (`label` is the type's whole name; a tile shows a short one.)
    readonly property string lunarisNote: serverMode
        ? "The server connects to the Lunaris app on that computer (remote control turned on). Its address and password are shown under \"This computer\" there."
        : "A computer with the Lunaris app (remote control turned on). Its address and password are shown under \"This computer\" there."
    readonly property var typeOptions: (addingMethod ? [] : [{ value: "lunaris", label: "Lunaris", note: lunarisNote }]).concat(
        connectors.map(function (c) { return { value: c.id, label: c.name, note: String(c.note || "") }; }))
    // The chosen type, as one of `typeOptions` (null when it is not one of them: an edit).
    readonly property var typeChosen: {
        for (var i = 0; i < typeOptions.length; i++)
            if (typeOptions[i].value === dlg.typeId) return typeOptions[i];
        return null;
    }
    // The fields of the chosen type, each with the room it takes in its row.
    readonly property var placedFields: Cards.layoutFields(dlg.visible && dlg.connector ? dlg.connector.fields : [])
    function chooseType(id) {
        error = "";
        typeId = id;
    }
    // The Lunaris computers that offer the chosen type.
    readonly property var throughOptions: {
        var options = [];
        for (var i = 0; i < through.length; i++) {
            var offered = through[i].connectors || [];
            for (var k = 0; k < offered.length; k++) {
                if (offered[k].id === dlg.typeId) {
                    options.push({ value: through[i].host_id, label: through[i].host_name });
                    break;
                }
            }
        }
        return options;
    }
    // The chosen type's description: as the chosen Lunaris computer has it
    // (computers may run different versions), else as listed.
    readonly property var connector: {
        if (dlg.isLunaris) return null;
        for (var i = 0; i < through.length; i++) {
            if (through[i].host_id !== dlg.throughHost) continue;
            var offered = through[i].connectors || [];
            for (var k = 0; k < offered.length; k++)
                if (offered[k].id === dlg.typeId) return offered[k];
        }
        for (var j = 0; j < connectors.length; j++)
            if (connectors[j].id === dlg.typeId) return connectors[j];
        return null;
    }
    onThroughOptionsChanged: pickThrough()
    function pickThrough() {
        if (!serverMode || editing) return;
        for (var i = 0; i < throughOptions.length; i++)
            if (throughOptions[i].value === throughHost) return;
        throughHost = throughOptions.length > 0 ? throughOptions[0].value : "";
    }

    // A field that may stay empty in the form (it keeps what is stored): a
    // secret (never shown), and on a server that does not send the stored
    // values every field.
    function keepsEmpty(field) {
        return dlg.editing && (field.kind === "secret" || (dlg.serverMode && !dlg.editValuesKnown));
    }

    function clearForm() {
        error = "";
        busy = false;
        sameChosen = sameCurrent;
        addressField.text = "";
        lunarisPasswordField.text = "";
        lunarisNameField.text = "";
        lunarisPasswordField.shown = false;
    }

    // One more way to connect to the computer of `card` ({ key, title,
    // group }): a connector type, named after the card.
    function openAddMethod(card) {
        sameOptions = [];
        sameCurrent = "";
        clearForm();
        editing = false;
        sourceId = "";
        editName = "";
        editValues = ({});
        editValuesKnown = false;
        methodCard = { key: String(card.key || ""), title: String(card.title || ""), group: String(card.group || "") };
        typeId = typeOptions.length > 0 ? typeOptions[0].value : "";
        nameField.text = methodCard.title;
        open();
        nameField.input.forceActiveFocus();
    }

    function openAdd() {
        methodCard = null;
        sameOptions = [];
        sameCurrent = "";
        clearForm();
        editing = false;
        sourceId = "";
        editName = "";
        editValues = ({});
        editValuesKnown = false;
        typeId = "lunaris";
        nameField.text = "";
        open();
        addressField.input.forceActiveFocus();
    }

    // `source`: { id, name, connector, connector_name, host_id (server) };
    // `values`: what may be shown of its fields (left out: not known, as
    // from a server before they were sent). `same`: the cards it can be
    // said to be the same computer as, and `sameNow` the one it is on.
    function openEdit(source, values, same, sameNow) {
        methodCard = null;
        sameOptions = same || [];
        sameCurrent = String(sameNow || "");
        clearForm();
        editing = true;
        sourceId = source.id;
        editName = source.name || "";
        editTypeName = source.connector_name || source.connector || "";
        editValuesKnown = values !== undefined && values !== null && typeof values === "object";
        editValues = editValuesKnown ? values : ({});
        throughHost = source.host_id || "";
        editThroughName = source.host_name || "";
        typeId = source.connector || "";
        nameField.text = editName;
        open();
        nameField.input.forceActiveFocus();
    }

    function submit() {
        if (busy || setupShown) return;
        error = "";
        if (isLunaris) {
            if (addressField.text.trim() === "") { error = I18n.t("Enter the address of the computer"); return; }
            lunarisSubmitted(addressField.text.trim(), lunarisPasswordField.text.trim(), lunarisNameField.text.trim());
            return;
        }
        if (serverMode && throughHost === "") { error = I18n.t("Choose the Lunaris computer it is reached through"); return; }
        var name = nameField.text.trim();
        if (name === "") { error = I18n.t("Give the computer a name"); return; }
        var values = {};
        for (var i = 0; i < fieldRepeater.count; i++) {
            var item = fieldRepeater.itemAt(i);
            if (!item) continue;
            var field = item.field;
            // A password is taken as typed; anything else without the spaces around it.
            var text = field.kind === "secret" ? item.value : item.value.trim();
            if (text === "") {
                if (keepsEmpty(field)) continue;
                if (field.required) { error = I18n.t("%1 is required").arg(I18n.t(field.label)); return; }
                // Edit: an optional field whose stored value was shown and is
                // now empty is sent empty (cleared); one that had none is left out.
                if (dlg.editing && dlg.editValuesKnown && String(dlg.editValues[field.key] || "") !== "")
                    values[field.key] = "";
                if (dlg.editing) continue;
            } else if (field.kind === "port" && !(/^[0-9]{1,5}$/.test(text) && parseInt(text) >= 1 && parseInt(text) <= 65535)) {
                error = I18n.t("%1 must be a port from 1 to 65535").arg(I18n.t(field.label));
                return;
            }
            values[field.key] = text;
        }
        sourceSubmitted(typeId, throughHost, sourceId, name, values, groupSaid());
    }

    onClosed: {
        if (setupWaiting) setupCancelled(setupId, setupHost);
        setupId = "";
        setupError = "";
        setupMessage = "";
        setupCode = "";
        clearForm();
        nameField.text = "";
    }

    IntValidator { id: portValidator; bottom: 1; top: 65535 }

    component FieldLabel: Text {
        Layout.fillWidth: true
        textFormat: Text.PlainText
        color: Theme.textMuted
        font.pixelSize: 12
        font.weight: Font.Medium
        elide: Text.ElideRight
    }
    component Note: Text {
        Layout.fillWidth: true
        textFormat: Text.PlainText
        color: Theme.textMuted
        font.pixelSize: 11
        wrapMode: Text.WordWrap
    }

    // The room there is for the form: a long one (many fields) scrolls
    // instead of leaving the window; the title and the buttons stay.
    readonly property real roomHeight: (QQC.Overlay.overlay ? QQC.Overlay.overlay.height : 720) - 90
    // What is left of it under the title and over the buttons.
    readonly property real bodyRoom: Math.max(120, dlg.roomHeight - titleText.implicitHeight - 24
        - (buttonRow.visible ? buttonRow.implicitHeight + 12 : 0)
        - (errorText.visible ? errorText.implicitHeight + 12 : 0))

    FontMetrics { id: optionMetrics; font.pixelSize: 13; font.weight: Font.DemiBold }

    contentItem: ColumnLayout {
        spacing: 12

        Text {
            id: titleText
            Layout.fillWidth: true
            objectName: "dialogTitle"
            textFormat: Text.PlainText
            text: dlg.editing ? I18n.t("Edit %1").arg(dlg.editName)
                : dlg.addingMethod ? I18n.t("Add a connection method to %1").arg(dlg.methodCard.title)
                : I18n.t("Add computer")
            color: Theme.textStrong
            font.pixelSize: 16
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }

      RowLayout {
        id: body
        Layout.fillWidth: true
        spacing: 16

        // ---- The types, as a list beside the form (adding, wide) ----
        ColumnLayout {
            id: rail
            objectName: "typeRail"
            visible: dlg.twoPane && !dlg.setupShown
            Layout.preferredWidth: dlg.railWidth
            Layout.maximumWidth: dlg.railWidth
            Layout.alignment: Qt.AlignTop
            spacing: 6
            FieldLabel { id: railLabel; text: I18n.t("Type") }
            Flickable {
                id: railScroller
                objectName: "railScroller"
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(railColumn.implicitHeight, dlg.bodyRoom - railLabel.implicitHeight - rail.spacing)
                contentHeight: railColumn.implicitHeight
                clip: contentHeight > height
                boundsBehavior: Flickable.StopAtBounds
                QQC.ScrollBar.vertical: QQC.ScrollBar { id: railBar; policy: railScroller.contentHeight > railScroller.height + 1 ? QQC.ScrollBar.AlwaysOn : QQC.ScrollBar.AlwaysOff }
                Column {
                    id: railColumn
                    objectName: "typeOptions"
                    width: railScroller.width - (railBar.policy === QQC.ScrollBar.AlwaysOn ? 12 : 0)
                    spacing: 6
                    Accessible.role: Accessible.Grouping
                    Accessible.name: I18n.t("Type")
                    Repeater {
                        model: dlg.twoPane ? dlg.typeOptions : []
                        delegate: TypeTile {
                            required property var modelData
                            objectName: "typeOption-" + modelData.value
                            width: railColumn.width
                            typeId: modelData.value
                            fullName: I18n.t(modelData.label)
                            text: Cards.typeShortName(modelData.value, I18n.t(modelData.label))
                            chosen: dlg.typeId === modelData.value
                            onClicked: dlg.chooseType(modelData.value)
                        }
                    }
                }
            }
        }
        Rectangle {
            visible: rail.visible
            Layout.fillHeight: true
            Layout.preferredWidth: 1
            color: Theme.border
        }

      Flickable {
        id: scroller
        objectName: "formScroller"
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignTop
        Layout.preferredHeight: Math.min(form.implicitHeight, dlg.bodyRoom)
        contentHeight: form.implicitHeight
        clip: contentHeight > height
        boundsBehavior: Flickable.StopAtBounds
        QQC.ScrollBar.vertical: QQC.ScrollBar { id: scrollBar; policy: scroller.contentHeight > scroller.height ? QQC.ScrollBar.AlwaysOn : QQC.ScrollBar.AlwaysOff }

      ColumnLayout {
        id: form
        width: scroller.width - (scrollBar.policy === QQC.ScrollBar.AlwaysOn ? 12 : 0)
        spacing: 10

        Note {
            objectName: "methodNote"
            visible: dlg.addingMethod && !dlg.setupShown
            font.pixelSize: 12
            text: dlg.addingMethod ? I18n.t("It is shown as one more way to connect to %1.").arg(dlg.methodCard.title) : ""
        }

        // ---- The set-up of the computer being added ----
        ColumnLayout {
            objectName: "setupView"
            visible: dlg.setupShown
            Layout.fillWidth: true
            spacing: 14
            Note {
                font.pixelSize: 12
                text: I18n.t("Setting up %1").arg(dlg.setupName)
            }
            // In the connector's own words.
            Text {
                objectName: "setupMessage"
                visible: dlg.setupWaiting
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: I18n.t(dlg.setupMessage)
                color: Theme.textStrong
                font.pixelSize: 14
                wrapMode: Text.WordWrap
            }
            Rectangle {
                visible: dlg.setupWaiting && dlg.setupCode !== ""
                Layout.alignment: Qt.AlignHCenter
                implicitWidth: Math.min(codeText.implicitWidth + 44, dlg.availableWidth)
                implicitHeight: codeText.implicitHeight + 20
                radius: Theme.radiusLg
                color: "transparent"
                border.color: Theme.borderStrong
                Text {
                    id: codeText
                    objectName: "setupCode"
                    anchors.centerIn: parent
                    width: Math.min(implicitWidth, dlg.availableWidth - 44)
                    textFormat: Text.PlainText
                    text: dlg.setupCode
                    color: Theme.textStrong
                    font.family: "monospace"
                    font.pixelSize: 40
                    font.weight: Font.Bold
                    font.letterSpacing: 10
                    wrapMode: Text.WrapAnywhere
                    horizontalAlignment: Text.AlignHCenter
                    Accessible.name: I18n.t("Code") + ": " + dlg.setupCode
                }
            }
            RowLayout {
                visible: dlg.setupWaiting
                Layout.fillWidth: true
                spacing: 8
                Rectangle {
                    implicitWidth: 8
                    implicitHeight: 8
                    radius: 4
                    color: Theme.textMuted
                    SequentialAnimation on opacity {
                        running: dlg.visible && dlg.setupWaiting
                        loops: Animation.Infinite
                        NumberAnimation { from: 1; to: 0.25; duration: 700 }
                        NumberAnimation { from: 0.25; to: 1; duration: 700 }
                    }
                }
                Text {
                    objectName: "setupWaitingText"
                    Layout.fillWidth: true
                    text: I18n.t("Waiting for the other computer…")
                    color: Theme.textMuted
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
            }
            Text {
                objectName: "setupErrorText"
                visible: dlg.setupError !== ""
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: dlg.setupError
                color: Theme.danger
                font.pixelSize: 13
                wrapMode: Text.WordWrap
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                SecondaryButton { objectName: "setupBackButton"; visible: dlg.setupError !== ""; text: I18n.t("Back"); onClicked: dlg.setupBack() }
                SecondaryButton { objectName: "setupCancelButton"; visible: dlg.setupWaiting; text: I18n.t("Cancel"); onClicked: dlg.close() }
                PrimaryButton { objectName: "setupCloseButton"; visible: dlg.setupError !== ""; text: I18n.t("Close"); onClicked: dlg.close() }
            }
        }

        // ---- Type ----
        ColumnLayout {
            visible: !dlg.setupShown
            Layout.fillWidth: true
            spacing: 6
            FieldLabel { visible: !dlg.twoPane; text: I18n.t("Type") }
            // Narrow (or editing, where it is not shown): the types in one
            // row of chips; with many types, in as many rows of the same
            // length as they need.
            GridLayout {
                id: typeGrid
                objectName: "typeChips"
                visible: !dlg.editing && !dlg.twoPane
                Layout.fillWidth: true
                // The most chips that fit side by side (a chip is 120 wide at least).
                readonly property int fit: Math.max(1, Math.floor((form.width + columnSpacing) / (120 + columnSpacing)))
                readonly property int rowsNeeded: Math.max(1, Math.ceil(dlg.typeOptions.length / fit))
                columns: Math.max(1, Math.ceil(dlg.typeOptions.length / rowsNeeded))
                columnSpacing: 8
                rowSpacing: 8
                Accessible.role: Accessible.Grouping
                Accessible.name: I18n.t("Type")
                Repeater {
                    model: !dlg.editing && !dlg.twoPane ? dlg.typeOptions : []
                    delegate: TypeTile {
                        required property var modelData
                        objectName: "typeOption-" + modelData.value
                        centered: true
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1
                        typeId: modelData.value
                        fullName: I18n.t(modelData.label)
                        text: Cards.typeShortName(modelData.value, I18n.t(modelData.label))
                        chosen: dlg.typeId === modelData.value
                        onClicked: dlg.chooseType(modelData.value)
                    }
                }
            }
            // The chosen type: its whole name and, once, what it says about
            // itself (a connector's own words).
            ColumnLayout {
                objectName: "typeAbout"
                visible: !dlg.editing && dlg.typeChosen !== null
                Layout.fillWidth: true
                Layout.topMargin: dlg.twoPane ? 0 : 2
                Layout.bottomMargin: 4
                spacing: 2
                Text {
                    objectName: "typeFullName"
                    Layout.fillWidth: true
                    textFormat: Text.PlainText
                    text: dlg.typeChosen ? I18n.t(dlg.typeChosen.label) : ""
                    color: Theme.textStrong
                    font.pixelSize: 12
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }
                Note {
                    objectName: "typeNote"
                    visible: dlg.typeChosen !== null && dlg.typeChosen.note !== ""
                    font.pixelSize: 12
                    text: dlg.typeChosen && dlg.typeChosen.note !== "" ? I18n.t(dlg.typeChosen.note) : ""
                }
            }
            // The type of a computer stays what it was added as.
            RowLayout {
                visible: dlg.editing
                Layout.fillWidth: true
                spacing: 8
                TypeLogo { typeId: dlg.typeId; name: I18n.t(dlg.editTypeName); size: 22 }
                Text {
                    Layout.fillWidth: true
                    textFormat: Text.PlainText
                    text: I18n.t(dlg.editTypeName)
                    color: Theme.textStrong
                    font.pixelSize: 13
                    elide: Text.ElideRight
                }
            }
        }

        // ---- Lunaris ----
        GridLayout {
            visible: dlg.isLunaris && !dlg.setupShown
            Layout.fillWidth: true
            // The address and the password share a row where there is room.
            columns: dlg.wide ? 2 : 1
            columnSpacing: 12
            rowSpacing: 10
            ColumnLayout {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                Layout.alignment: Qt.AlignTop
                spacing: 4
                FieldLabel { text: I18n.t("Address") + " *" }
                InputField { id: addressField; objectName: "addressField"; placeholder: I18n.t("Address, e.g. 192.168.1.20"); onAccepted: dlg.submit() }
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                Layout.alignment: Qt.AlignTop
                spacing: 4
                FieldLabel { text: I18n.t("Password (optional)") }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 4
                    InputField {
                        id: lunarisPasswordField
                        objectName: "lunarisPasswordField"
                        property bool shown: false
                        password: !shown
                        onAccepted: dlg.submit()
                    }
                    IconButton {
                        iconName: lunarisPasswordField.shown ? "eyeOff" : "eye"
                        tip: lunarisPasswordField.shown ? I18n.t("Hide password") : I18n.t("Show password")
                        onClicked: lunarisPasswordField.shown = !lunarisPasswordField.shown
                    }
                }
            }
            Note {
                Layout.columnSpan: parent.columns
                text: I18n.t("Without the password, the person at that computer is asked to accept each connection.")
            }
            ColumnLayout {
                visible: !dlg.serverMode
                Layout.fillWidth: true
                Layout.columnSpan: parent.columns
                Layout.alignment: Qt.AlignTop
                spacing: 4
                FieldLabel { text: I18n.t("Name (optional)") }
                InputField { id: lunarisNameField; objectName: "lunarisNameField"; placeholder: I18n.t("Name, e.g. Office PC"); input.maximumLength: 64; onAccepted: dlg.submit() }
            }
        }

        // ---- A connector type ----
        ColumnLayout {
            visible: !dlg.isLunaris && !dlg.setupShown
            Layout.fillWidth: true
            spacing: 10

            // The name and, on a server, the Lunaris computer it is reached
            // through: side by side where that is a line or two options.
            GridLayout {
                id: nameGrid
                objectName: "nameGrid"
                Layout.fillWidth: true
                readonly property bool paired: dlg.wide && dlg.serverMode && (dlg.editing || dlg.throughOptions.length <= 2)
                columns: paired ? 2 : 1
                columnSpacing: 12
                rowSpacing: 10

                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.preferredWidth: 1
                    Layout.alignment: Qt.AlignTop
                    spacing: 4
                    FieldLabel { text: I18n.t("Name") + " *" }
                    InputField { id: nameField; objectName: "nameField"; placeholder: I18n.t("Name, e.g. Office PC"); input.maximumLength: 64; onAccepted: dlg.submit() }
                }

                ColumnLayout {
                    visible: dlg.serverMode
                    Layout.fillWidth: true
                    Layout.preferredWidth: 1
                    Layout.alignment: Qt.AlignTop
                    spacing: 4
                    FieldLabel { text: I18n.t("Through which Lunaris computer") }
                    // A few: each a card to click. Many: a list to open.
                    GridLayout {
                        objectName: "throughOptions"
                        visible: !dlg.editing && dlg.throughOptions.length <= 4
                        Layout.fillWidth: true
                        columns: dlg.throughOptions.length > 1 ? 2 : 1
                        columnSpacing: 8
                        rowSpacing: 8
                        Accessible.role: Accessible.Grouping
                        Accessible.name: I18n.t("Through which Lunaris computer")
                        Repeater {
                            model: dlg.throughOptions.length <= 4 ? dlg.throughOptions : []
                            delegate: OptionCard {
                                required property var modelData
                                objectName: "throughOption-" + modelData.value
                                Layout.fillWidth: true
                                Layout.preferredWidth: 1
                                Layout.minimumWidth: 0
                                plain: true
                                text: String(modelData.label)
                                chosen: dlg.throughHost === modelData.value
                                onClicked: dlg.throughHost = modelData.value
                            }
                        }
                    }
                    ThemedCombo {
                        objectName: "throughCombo"
                        visible: !dlg.editing && dlg.throughOptions.length > 4
                        Layout.fillWidth: true
                        options: dlg.throughOptions
                        value: dlg.throughHost
                        onPicked: (v) => dlg.throughHost = v
                        Accessible.name: I18n.t("Through which Lunaris computer")
                    }
                    Text {
                        visible: dlg.editing
                        Layout.fillWidth: true
                        Layout.preferredHeight: 36
                        verticalAlignment: Text.AlignVCenter
                        textFormat: Text.PlainText
                        text: {
                            for (var i = 0; i < dlg.through.length; i++)
                                if (dlg.through[i].host_id === dlg.throughHost) return dlg.through[i].host_name;
                            return dlg.editThroughName;
                        }
                        color: Theme.textStrong
                        font.pixelSize: 13
                        elide: Text.ElideRight
                    }
                }
                Note {
                    visible: dlg.serverMode && !dlg.editing
                    Layout.columnSpan: nameGrid.columns
                    text: I18n.t("That computer connects to the new one and shows it. The address and password are kept there, not on the server.")
                }
            }

            // The type's own fields, 6 columns to a row (Cards.layoutFields).
            GridLayout {
                id: fieldGrid
                objectName: "fieldGrid"
                visible: fieldRepeater.count > 0
                Layout.fillWidth: true
                // (The row that sets the columns' width has no height of its own.)
                Layout.topMargin: -rowSpacing
                columns: 6
                columnSpacing: 12
                rowSpacing: 10

                // (Six columns of one width, whatever stands in them.)
                Repeater {
                    model: 6
                    delegate: Item { Layout.fillWidth: true; Layout.preferredWidth: 1; implicitHeight: 0; Layout.maximumHeight: 0 }
                }
                Repeater {
                    id: fieldRepeater
                    // Only while the dialog is open, so that nothing typed outlives it.
                    model: dlg.placedFields
                    delegate: ColumnLayout {
                        id: fieldItem
                        required property var modelData
                        readonly property var field: modelData.field
                        readonly property bool secret: field.kind === "secret"
                        readonly property bool port: field.kind === "port"
                        // A choice with nothing to choose from is a line to type.
                        readonly property var options: field.kind === "choice" && field.options instanceof Array ? field.options : []
                        readonly property bool choice: options.length > 0
                        // Many options, or long ones: a drop-down.
                        readonly property bool asList: Cards.choiceAsList(field)
                        // Edit: what is stored and may be shown; add: the connector's default.
                        readonly property string first: dlg.editing ? (secret ? "" : String(dlg.editValues[field.key] || ""))
                                                                    : String(field["default"] || "")
                        property string picked: first
                        readonly property string value: choice ? picked : fieldInput.text
                        property bool shown: false
                        objectName: "fieldItem-" + field.key
                        Layout.columnSpan: dlg.wide ? modelData.span : 6
                        Layout.fillWidth: true
                        Layout.preferredWidth: Layout.columnSpan
                        Layout.minimumWidth: 0
                        Layout.alignment: Qt.AlignTop
                        spacing: 4
                        FieldLabel {
                            text: I18n.t(fieldItem.field.label) + (fieldItem.field.required && !dlg.keepsEmpty(fieldItem.field) ? " *" : "")
                        }
                        RowLayout {
                            visible: !fieldItem.choice
                            Layout.fillWidth: true
                            spacing: 4
                            InputField {
                                id: fieldInput
                                objectName: "field-" + fieldItem.field.key
                                // A port's box is as wide as its part of the row, and short where it has a row of its own.
                                Layout.fillWidth: !fieldItem.port || fieldItem.Layout.columnSpan < 6
                                implicitWidth: fieldItem.port ? 110 : 120
                                password: fieldItem.secret && !fieldItem.shown
                                text: fieldItem.first
                                // (A port's box is too narrow for a hint; the note below says it.)
                                placeholder: !dlg.editing || fieldItem.port ? ""
                                    : fieldItem.secret ? I18n.t("Leave empty to keep the password")
                                    : dlg.keepsEmpty(fieldItem.field) ? I18n.t("Leave empty to keep it") : ""
                                input.validator: fieldItem.port ? portValidator : null
                                input.maximumLength: fieldItem.port ? 5 : 255
                                input.inputMethodHints: fieldItem.secret ? Qt.ImhSensitiveData | Qt.ImhNoPredictiveText : Qt.ImhNone
                                onAccepted: dlg.submit()
                                Accessible.name: I18n.t(fieldItem.field.label)
                            }
                            IconButton {
                                visible: fieldItem.secret
                                iconName: fieldItem.shown ? "eyeOff" : "eye"
                                tip: fieldItem.shown ? I18n.t("Hide password") : I18n.t("Show password")
                                onClicked: fieldItem.shown = !fieldItem.shown
                            }
                            Item { visible: fieldItem.port && fieldItem.Layout.columnSpan === 6; Layout.fillWidth: true }
                        }
                        // One of a few: each an option to click (side by side
                        // where their names fit); of many, a drop-down.
                        ColumnLayout {
                            objectName: "choice-" + fieldItem.field.key
                            visible: fieldItem.choice
                            Layout.fillWidth: true
                            spacing: 0
                            ThemedCombo {
                                objectName: "fieldCombo-" + fieldItem.field.key
                                visible: fieldItem.asList
                                Layout.fillWidth: true
                                implicitWidth: 120
                                options: fieldItem.asList ? fieldItem.options : []
                                value: fieldItem.picked
                                onPicked: (v) => fieldItem.picked = String(v)
                                Accessible.name: I18n.t(fieldItem.field.label)
                            }
                            GridLayout {
                                id: choiceGrid
                                visible: !fieldItem.asList
                                Layout.fillWidth: true
                                readonly property real widest: {
                                    var most = 0;
                                    for (var i = 0; i < fieldItem.options.length; i++)
                                        most = Math.max(most, optionMetrics.advanceWidth(I18n.t(String(fieldItem.options[i].label))));
                                    return most + 56;
                                }
                                readonly property int count: Math.max(1, fieldItem.options.length)
                                columns: (fieldItem.width + columnSpacing) / count - columnSpacing >= widest ? count : 1
                                columnSpacing: 8
                                rowSpacing: 6
                                Accessible.role: Accessible.Grouping
                                Accessible.name: I18n.t(fieldItem.field.label)
                                Repeater {
                                    model: fieldItem.asList ? [] : fieldItem.options
                                    delegate: OptionCard {
                                        required property var modelData
                                        objectName: "field-" + fieldItem.field.key + "-" + modelData.value
                                        Layout.fillWidth: true
                                        Layout.preferredWidth: 1
                                        plain: true
                                        text: I18n.t(String(modelData.label))
                                        chosen: fieldItem.picked === String(modelData.value)
                                        onClicked: fieldItem.picked = String(modelData.value)
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Note {
                visible: dlg.editing && dlg.serverMode && !dlg.editValuesKnown && dlg.connector !== null
                text: I18n.t("Leave a field empty to keep it. The server never shows what is stored.")
            }
            Note {
                objectName: "passwordsNote"
                visible: dlg.editing && dlg.serverMode && dlg.editValuesKnown && dlg.connector !== null
                    && (dlg.connector.fields || []).some(function (f) { return f.kind === "secret"; })
                text: I18n.t("Passwords are not shown: leave one empty to keep it.")
            }

            // ---- Edit: which computer it is ----
            ColumnLayout {
                objectName: "sameView"
                visible: dlg.editing && dlg.sameOptions.length > 0
                Layout.fillWidth: true
                spacing: 4
                FieldLabel { text: I18n.t("Same computer as") }
                QQC.ScrollView {
                    id: sameScroll
                    Layout.fillWidth: true
                    Layout.preferredHeight: Math.min(sameGrid.implicitHeight, 134)
                    clip: true
                    contentWidth: availableWidth
                    GridLayout {
                        id: sameGrid
                        width: sameScroll.availableWidth
                        columns: dlg.wide ? 2 : 1
                        columnSpacing: 8
                        rowSpacing: 6
                        OptionCard {
                            objectName: "sameOption-own"
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            plain: true
                            text: I18n.t("A computer of its own")
                            chosen: dlg.sameChosen === ""
                            onClicked: dlg.sameChosen = ""
                        }
                        Repeater {
                            model: dlg.editing ? dlg.sameOptions : []
                            delegate: OptionCard {
                                required property var modelData
                                objectName: "sameOption-" + modelData.value
                                Layout.fillWidth: true
                                Layout.preferredWidth: 1
                                plain: true
                                text: String(modelData.label)
                                chosen: dlg.sameChosen === modelData.value
                                onClicked: dlg.sameChosen = modelData.value
                            }
                        }
                    }
                }
                Note { text: I18n.t("Computers that are the same machine are shown as one, with a way to connect for each.") }
            }
            Note {
                visible: dlg.editing && dlg.connector === null
                text: dlg.serverMode
                    ? I18n.t("Its other fields can be changed when the Lunaris computer it is reached through is online.")
                    : I18n.t("This type is not installed any more: only the name can be changed.")
            }
        }
      }
      }
      }

        // What is wrong with the form, and the buttons: always in sight.
        Text {
            id: errorText
            objectName: "errorText"
            visible: dlg.error !== "" && !dlg.setupShown
            Layout.fillWidth: true
            textFormat: Text.PlainText
            text: dlg.error
            color: Theme.danger
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }

        RowLayout {
            id: buttonRow
            objectName: "buttonRow"
            visible: !dlg.setupShown
            Layout.fillWidth: true
            spacing: 8
            Item { Layout.fillWidth: true }
            SecondaryButton { text: I18n.t("Cancel"); onClicked: dlg.close() }
            PrimaryButton {
                objectName: "submitButton"
                text: dlg.editing ? (dlg.busy ? I18n.t("Saving…") : I18n.t("Save"))
                                  : (dlg.busy ? I18n.t("Adding…") : I18n.t("Add"))
                onClicked: dlg.submit()
            }
        }
    }
}
