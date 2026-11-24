import QtQuick
import QtTest

// main.qml itself, off screen, with the stand-ins of stubs/: the viewer's
// own pointer is hidden while a button is held and the host draws its
// pointer into the picture. Either the host says so (a Windows host while a
// window is moved), or the client found the pointer in the decoded picture
// (pointerInPictureUpdated with found). The host's "maybe" alone hides
// nothing. A host whose pointer is part of its picture for the whole session
// (pointerAlwaysInPictureUpdated: a Sunshine source) gets no pointer at all
// over the picture, button or not, pointer locked or not.
Item {
    id: root
    width: 200
    height: 100

    TestCase {
        name: "PointerInPicture"
        when: windowShown

        property var app: null
        property var bridge: null

        function init() {
            var component = Qt.createComponent("../../qml/main.qml");
            compare(component.status, Component.Ready, component.errorString());
            app = component.createObject(null);
            verify(app !== null);
            bridge = findChild(app, "bridge");
            verify(bridge !== null);
        }

        function cleanup() {
            if (app) app.destroy();
            app = null;
        }

        function test_maybe_alone_hides_nothing() {
            bridge.hostOsUpdated("Windows");
            app.hostCursorMouseDown = true;
            bridge.hostCursorUpdated(100, 100, true, "arrow", false);
            bridge.pointerInPictureUpdated(true, false);
            verify(app.pointerMaybeInPicture);
            verify(!app.pointerFoundInPicture);
            verify(!app.shouldHidePredictedCursor(), "only \"maybe\": the viewer's pointer stays");
        }

        function test_found_hides_while_the_button_is_held_data() {
            // Whatever the host's system is said to be: the picture was looked at.
            return [{ tag: "windows" }, { tag: "linux" }, { tag: "unknown" }];
        }

        function test_found_hides_while_the_button_is_held(data) {
            bridge.hostOsUpdated(data.tag);
            bridge.hostCursorUpdated(100, 100, true, "arrow", false);
            verify(!app.shouldHidePredictedCursor());

            app.hostCursorMouseDown = true;
            verify(!app.shouldHidePredictedCursor());
            bridge.pointerInPictureUpdated(true, false);
            verify(!app.shouldHidePredictedCursor());
            bridge.pointerInPictureUpdated(true, true);
            verify(app.shouldHidePredictedCursor(), "found in the picture: hidden");

            // The host's "maybe" went false, or the button was let go: shown again.
            bridge.pointerInPictureUpdated(false, false);
            verify(!app.shouldHidePredictedCursor());
            bridge.pointerInPictureUpdated(true, true);
            verify(app.shouldHidePredictedCursor());
            app.hostCursorMouseDown = false;
            verify(!app.shouldHidePredictedCursor(), "no button held: never hidden");
        }

        function test_the_hosts_own_flag_still_hides() {
            bridge.hostOsUpdated("Windows");
            app.hostCursorMouseDown = true;
            bridge.hostCursorUpdated(100, 100, true, "arrow", true);
            verify(app.shouldHidePredictedCursor(), "in_window_move_size of a Windows host");
            bridge.hostCursorUpdated(100, 100, true, "arrow", false);
            verify(!app.shouldHidePredictedCursor());
            // As before, that flag counts for a Windows host only.
            bridge.hostOsUpdated("linux");
            bridge.hostCursorUpdated(100, 100, true, "arrow", true);
            verify(!app.shouldHidePredictedCursor());
        }

        // What the stream view shows as a pointer: the system's pointer in
        // the host's shape (HostCursor, blank while `hidden`), the mouse
        // area's own cursor shape when HostCursor is not driving it, and the
        // pointer drawn while the pointer is locked.
        function pointers() {
            var hostCursor = findChild(app, "hostCursor");
            var area = findChild(app, "streamMouseArea");
            var overlay = findChild(app, "hostCursorOverlay");
            verify(hostCursor !== null && area !== null && overlay !== null);
            return {
                // The system pointer over the picture is blank.
                systemBlank: hostCursor.active ? hostCursor.hidden : area.cursorShape === Qt.BlankCursor,
                drawn: overlay.visible
            };
        }

        function test_always_in_the_picture_shows_no_pointer_data() {
            // Whatever the host's system is said to be (a Sunshine host does not say).
            return [{ tag: "unknown" }, { tag: "windows" }, { tag: "linux" }];
        }

        function test_always_in_the_picture_shows_no_pointer(data) {
            bridge.hostOsUpdated(data.tag);
            app.isStreamMode = true;
            // The person's mouse is over the picture: an ordinary host gets a pointer.
            app.updateLocalCursorPrediction(10, 10);
            verify(app.localCursorVisible);
            verify(!app.shouldHidePredictedCursor());
            verify(!pointers().systemBlank, "an ordinary host: the host-shaped pointer");

            bridge.pointerAlwaysInPictureUpdated(true);
            verify(app.pointerAlwaysInPicture);
            // No button held, and neither of the other two flags.
            verify(!app.hostCursorMouseDown && !app.hostCursorInWindowMoveSize && !app.pointerFoundInPicture);
            verify(app.shouldHidePredictedCursor());
            verify(pointers().systemBlank, "absolute mode: no system pointer");
            verify(!pointers().drawn);

            // A cursor message that says "visible", a move, a button: still none.
            bridge.hostCursorUpdated(100, 100, true, "arrow", false);
            app.updateLocalCursorPrediction(20, 20);
            app.hostCursorMouseDown = true;
            verify(pointers().systemBlank);
            verify(!pointers().drawn);
            app.hostCursorMouseDown = false;
            verify(pointers().systemBlank);

            // "Show local cursor": still none over the picture.
            app.hideLocalCursor = false;
            verify(pointers().systemBlank, "show local cursor: no system pointer either");
            verify(!pointers().drawn);
            app.hideLocalCursor = true;

            // Pointer locked (relative mode): none drawn.
            app.isPointerLocked = true;
            verify(pointers().systemBlank, "pointer locked: no system pointer");
            verify(!pointers().drawn, "pointer locked: no drawn pointer");
            app.isPointerLocked = false;

            // The session ends: the next one starts as an ordinary host's.
            app.isStreamMode = false;
            verify(!app.pointerAlwaysInPicture);
            verify(!app.shouldHidePredictedCursor());
        }

        function test_an_ordinary_host_keeps_its_pointers() {
            bridge.hostOsUpdated("linux");
            app.isStreamMode = true;
            bridge.pointerAlwaysInPictureUpdated(false);
            bridge.hostCursorUpdated(100, 100, true, "arrow", false);
            app.updateLocalCursorPrediction(10, 10);
            verify(!app.pointerAlwaysInPicture);
            verify(!pointers().systemBlank, "the host-shaped system pointer");
            // Pointer locked: the pointer is drawn.
            app.isPointerLocked = true;
            verify(pointers().drawn, "pointer locked: the drawn pointer");
            app.isPointerLocked = false;
            app.isStreamMode = false;
        }

        // The game mouse moves the host's pointer: while the pointer is
        // locked one cursor is drawn, where the host says (the Windows
        // desktop), and none while the host's is hidden (a game that draws
        // its own into the picture).
        function test_the_game_mouse_draws_the_hosts_pointer_where_it_is() {
            bridge.hostOsUpdated("windows");
            app.isStreamMode = true;
            app.gameMouseOn = true;
            app.isPointerLocked = true;
            bridge.hostCursorUpdated(100, 50, true, "arrow", false);
            verify(!app.shouldHidePredictedCursor());
            verify(pointers().drawn, "the host's pointer is drawn");
            compare(app.localCursorX, 100);
            compare(app.localCursorY, 50);
            // A locked move is not predicted: the host says where it went.
            bridge.localCursorDelta(40, 40);
            compare(app.localCursorX, 100);
            bridge.hostCursorUpdated(300, 200, true, "arrow", false);
            compare(app.localCursorX, 300);
            compare(app.localCursorY, 200);
            // In a game: hidden on the host, so none drawn.
            bridge.hostCursorUpdated(300, 200, false, "arrow", false);
            verify(!pointers().drawn, "the game's pointer only");
            app.isPointerLocked = false;
            app.gameMouseOn = false;
            app.isStreamMode = false;
        }
    }
}
