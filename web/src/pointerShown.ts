// Which pointers the stream view shows.
//
// Three things can be a pointer over the picture: the host's cursor drawn by
// the page at the place the host reports ("host cursor"), the page's own
// cursor drawn where the person's mouse or finger is ("own cursor", shaped
// like the host's), and the system's pointer (the CSS `cursor` of the
// picture). What the host says decides between them:
//
// - nothing: the host sends its cursor's shape and position apart from the
//   picture, and the page shows one of the three.
// - `in_window_move_size` (a Windows host while a window is moved), or the
//   page found the host's pointer in the picture during this drag
//   (pointerInPicture.ts, after the host's `pointer_maybe_in_picture`): the
//   pointer is in the picture for the moment, so the drawn ones hide while a
//   button is held. For a Windows host only, as before.
// - `pointer_in_picture: "always"` (a computer shown through a connector
//   that always draws its pointer into what it sends, such as a Sunshine
//   host): for the whole session the only pointer is the one in the picture.
//   The page draws none and the system's pointer is hidden over the picture,
//   in every input mode, whatever the host's system and the "hide local
//   cursor" choice.
//
// The desktop client decides the same way (client-qml: main.qml
// `shouldHidePredictedCursor`, bridge.rs `pointer_always_in_picture`);
// change neither alone.

export type PointerView = {
  /** The session shows a picture. */
  streaming: boolean;
  /** The host's system as the host says it ("windows", "linux", ..., "unknown"). */
  hostOs: string;
  /** `pointer_in_picture: "always"` of the host's cursor messages. */
  alwaysInPicture: boolean;
  /** A mouse button is held over the picture. */
  buttonHeld: boolean;
  /** `in_window_move_size` of the host's last cursor message. */
  inWindowMoveSize: boolean;
  /** The host's pointer was found in the picture during this drag. */
  foundInPicture: boolean;
  /** `visible` of the host's last cursor message (false before the first). */
  hostCursorVisible: boolean;
  /** A real mouse moved over the picture (not only touches). */
  hardwareMouse: boolean;
  /** The touch mode is "trackpad". */
  trackpad: boolean;
  /** The person's choice "hide local cursor" (on by default). */
  hideLocalCursor: boolean;
  pointerLocked: boolean;
  /** The host's cursor as a CSS `cursor` value, once a cursor message came. */
  hostCssCursor: string | null;
  /** The host delivered a cursor image (not only a kind). */
  hasNativeCursorImage: boolean;
  /** The game mouse is on and the pointer locked (src/gameMouse.ts): a game
   *  is played, which draws its own pointer into the picture. */
  gameMouse?: boolean;
};

/** `pointer_in_picture` of a `host_cursor` message: only "always" says so. */
export const saysPointerAlwaysInPicture = (message: { pointer_in_picture?: unknown } | null | undefined): boolean =>
  message?.pointer_in_picture === 'always';

/** The host's pointer is part of the picture now: the page draws none. */
export const hostPointerInPicture = (view: PointerView): boolean =>
  view.alwaysInPicture
  || !!view.gameMouse
  || (view.buttonHeld && view.hostOs === 'windows' && (view.inWindowMoveSize || view.foundInPicture));

/** The host's cursor drawn at the place the host reports. */
export const showsHostCursor = (view: PointerView): boolean => {
  if (hostPointerInPicture(view)) return false;
  // Touchpad on a phone: only the page's own cursor is shown.
  if (view.trackpad && !view.hardwareMouse) return false;
  // Under a real mouse the own cursor or the system's pointer stands for it.
  return view.hostCursorVisible && !view.hardwareMouse;
};

/** The page's own cursor, drawn where the person's mouse or finger is. */
export const showsOwnCursor = (view: PointerView): boolean => {
  if (hostPointerInPicture(view)) return false;
  const trackpad = view.trackpad && view.streaming && !view.hardwareMouse;
  // While the system draws the host-shaped CSS cursor (pointer not locked,
  // host not Windows) the cursor drawn by the page is not needed.
  const systemDrawsHostCursor = view.hostCssCursor !== null && view.hostOs !== 'windows' && !view.pointerLocked;
  const windowsPrediction = view.hostOs === 'windows' && view.hasNativeCursorImage;
  const underMouse = view.streaming
    && view.hardwareMouse
    && (view.hideLocalCursor || windowsPrediction)
    && !systemDrawsHostCursor;
  return trackpad || underMouse;
};

/** The CSS `cursor` of the picture: the system's pointer over it. */
export const streamCursorCss = (view: PointerView): string => {
  if (!view.streaming) return 'default';
  if (view.alwaysInPicture) return 'none';
  // The system draws the host's cursor when it can (as smooth as a pointer
  // of this computer); otherwise it is hidden and the page draws one.
  if (view.hideLocalCursor && view.hostOs !== 'windows' && !view.pointerLocked && view.hostCssCursor !== null) {
    return view.hostCssCursor;
  }
  return view.hideLocalCursor || view.hostOs === 'windows' ? 'none' : 'default';
};
