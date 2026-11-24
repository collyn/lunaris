// The keyboard of the stream view: which key is sent for a `KeyboardEvent`,
// which keys are held on the remote computer, and when the browser is asked
// for the key combinations it keeps for itself.

/** `KeyboardEvent.code` (where the key is) to the Windows virtual-key code of
 *  the key at that place on a US keyboard, which is what the host presses.
 *  Shift, Control and Alt go with their side (VK_LSHIFT 0xA0 ... VK_RMENU
 *  0xA5): the right Alt is AltGr on many layouts, and a host tells the sides
 *  apart. Keys without an entry are not sent. */
export const KEY_TO_VK: Record<string, number> = {
  "Backspace": 8, "Tab": 9, "Enter": 13, "NumpadEnter": 13,
  "ShiftLeft": 0xA0, "ShiftRight": 0xA1,
  "ControlLeft": 0xA2, "ControlRight": 0xA3, "AltLeft": 0xA4, "AltRight": 0xA5,
  "Pause": 19, "CapsLock": 20, "Escape": 27, "Space": 32, "PageUp": 33,
  "PageDown": 34, "End": 35, "Home": 36, "ArrowLeft": 37, "ArrowUp": 38,
  "ArrowRight": 39, "ArrowDown": 40, "PrintScreen": 44, "Insert": 45,
  "Delete": 46, "Digit0": 48, "Digit1": 49, "Digit2": 50, "Digit3": 51,
  "Digit4": 52, "Digit5": 53, "Digit6": 54, "Digit7": 55, "Digit8": 56,
  "Digit9": 57, "KeyA": 65, "KeyB": 66, "KeyC": 67, "KeyD": 68, "KeyE": 69,
  "KeyF": 70, "KeyG": 71, "KeyH": 72, "KeyI": 73, "KeyJ": 74, "KeyK": 75,
  "KeyL": 76, "KeyM": 77, "KeyN": 78, "KeyO": 79, "KeyP": 80, "KeyQ": 81,
  "KeyR": 82, "KeyS": 83, "KeyT": 84, "KeyU": 85, "KeyV": 86, "KeyW": 87,
  "KeyX": 88, "KeyY": 89, "KeyZ": 90, "MetaLeft": 91, "MetaRight": 92,
  "ContextMenu": 93,
  "Numpad0": 96, "Numpad1": 97, "Numpad2": 98, "Numpad3": 99, "Numpad4": 100,
  "Numpad5": 101, "Numpad6": 102, "Numpad7": 103, "Numpad8": 104, "Numpad9": 105,
  "NumpadMultiply": 106, "NumpadAdd": 107, "NumpadSubtract": 109, "NumpadDecimal": 110,
  "NumpadDivide": 111, "F1": 112, "F2": 113, "F3": 114, "F4": 115, "F5": 116,
  "F6": 117, "F7": 118, "F8": 119, "F9": 120, "F10": 121, "F11": 122, "F12": 123,
  "F13": 124, "F14": 125, "F15": 126, "F16": 127, "F17": 128, "F18": 129,
  "F19": 130, "F20": 131, "F21": 132, "F22": 133, "F23": 134, "F24": 135,
  "NumLock": 144, "ScrollLock": 145, "Semicolon": 186, "Equal": 187, "Comma": 188,
  "Minus": 189, "Period": 190, "Slash": 191, "Backquote": 192, "BracketLeft": 219,
  "Backslash": 220, "BracketRight": 221, "Quote": 222, "IntlBackslash": 226
};

/** The virtual-key code sent for a key, 0 when the key is not sent. */
export const vkOfCode = (code: string): number => KEY_TO_VK[code] || 0;

/** Alt, with or without a side (VK_MENU, VK_LMENU, VK_RMENU). */
const isAlt = (vk: number): boolean => vk === 0x12 || vk === 0xA4 || vk === 0xA5;

/** A key event to send: the virtual-key code and whether it is a press. */
export type KeyStroke = [vk: number, down: boolean];

/** The keys this viewer has pressed on the remote computer and not let go.
 *  They are let go there when the page stops getting the keyboard (another
 *  window or tab, a dialog of the page, the end of the stream): their
 *  releases will not come. moonlight-qt does the same with `m_KeysDown` and
 *  `raiseAllKeys()` (app/streaming/input/keyboard.cpp 521-537, called from
 *  input.cpp notifyFocusLost 273-285: "certain keys used in shortcuts that
 *  cause focus loss (such as Alt+Tab) may get stuck down"). */
export class PressedKeys {
  private down: number[] = [];

  note(vk: number, isDown: boolean): void {
    this.down = this.down.filter((k) => k !== vk);
    if (isDown) this.down.push(vk);
  }

  get count(): number { return this.down.length; }

  /** The events that let every held key go, last pressed first. After Alt,
   *  one more press and release of it, as RustDesk does (src/keyboard.rs
   *  release_remote_keys_for_events 791-807: "the controlled side may hold
   *  the alt state, if local window is switched by [Alt + Tab]"): a lone Alt
   *  opens the window's menu on Windows, the second one closes it. */
  releaseAll(): KeyStroke[] {
    const strokes: KeyStroke[] = [];
    for (const vk of [...this.down].reverse()) {
      strokes.push([vk, false]);
      if (isAlt(vk)) strokes.push([vk, true], [vk, false]);
    }
    this.down = [];
    return strokes;
  }
}

/** macOS browsers send no `keyup` for a key let go while Command is held, so
 *  such a key would stay down on the remote computer. It is pressed and let
 *  go at once instead, which is what noVNC does (core/input/keyboard.js
 *  _handleKeyDown: "macOS doesn't send proper key releases if a key is
 *  pressed while meta is held down"). */
export const tapsUnderMeta = (isMac: boolean, metaHeld: boolean, code: string): boolean =>
  isMac && metaHeld && code !== 'MetaLeft' && code !== 'MetaRight';

/** "System shortcuts" (Settings, Input): "In full screen only" unless set to "Never". */
export const readSystemKeys = (): boolean => localStorage.getItem('lunaris_system_keys') !== 'false';

/** What to ask of the browser's Keyboard Lock (Chromium; `navigator.keyboard`):
 *  - 'all': every key the browser can give a page in full screen (Alt+Tab,
 *    the Windows/Super key, Ctrl+W/T/N, Esc), when the setting is on;
 *  - 'escape': Esc alone, so that the page's own "hold Esc" works while the
 *    pointer is locked or the page is in full screen;
 *  - 'none': nothing locked.
 *  One answer for every caller: a later `lock()` replaces the earlier one,
 *  so locking Esc for the pointer lock after locking every key for full
 *  screen left only Esc locked. */
export const keyboardLockFor = (fullscreen: boolean, pointerLocked: boolean, systemKeys: boolean): 'all' | 'escape' | 'none' => {
  if (fullscreen && systemKeys) return 'all';
  if (fullscreen || pointerLocked) return 'escape';
  return 'none';
};

type KeyboardLockApi = { lock?: (codes?: string[]) => Promise<void>; unlock?: () => void };

/** Whether Esc can be kept from the browser while the pointer is locked
 *  (Keyboard Lock: a tap goes to the remote computer, a 3 s hold lets go).
 *  Chromium gives it only to secure pages (https, localhost); on an http
 *  page, in Safari and in Firefox the browser lets go of the pointer at the
 *  first Esc. */
export const escapeCanBeKept = (keyboard: KeyboardLockApi | undefined, secure: boolean): boolean =>
  secure && !!keyboard && typeof keyboard.lock === 'function';

/** This page over the server's https port (`/api/server/ports`), to keep
 *  Esc; null when the server has none, or the page is https already. */
export const secureAddress = (
  page: { protocol: string; hostname: string; pathname: string; search: string },
  httpsPort: number | null | undefined,
): string | null => {
  if (page.protocol === 'https:' || !httpsPort) return null;
  const host = page.hostname.includes(':') ? `[${page.hostname}]` : page.hostname;
  return `https://${host}${httpsPort === 443 ? '' : `:${httpsPort}`}${page.pathname}${page.search}`;
};

/** Applies [`keyboardLockFor`]; false when the browser has no Keyboard Lock. */
export const applyKeyboardLock = (keyboard: KeyboardLockApi | undefined, want: 'all' | 'escape' | 'none'): boolean => {
  if (!keyboard || typeof keyboard.lock !== 'function') return false;
  if (want === 'none') {
    if (typeof keyboard.unlock === 'function') keyboard.unlock();
    return true;
  }
  (want === 'all' ? keyboard.lock() : keyboard.lock(['Escape'])).catch(() => {});
  return true;
};

/** "Send keys" in the stream toolbar: combinations the browser or this
 *  computer takes before the page sees them. `notOnWindows`: a Windows
 *  computer takes Ctrl+Alt+Del only from its own keyboard (the secure
 *  attention sequence); the Lunaris agent cannot press it there. */
export const KEY_COMBOS: { id: string; label: string; keys: number[]; notOnWindows?: boolean }[] = [
  { id: 'alt-tab', label: 'Alt+Tab', keys: [0xA4, 0x09] },
  { id: 'win', label: 'Windows key (Super)', keys: [0x5B] },
  { id: 'alt-f4', label: 'Alt+F4', keys: [0xA4, 0x73] },
  { id: 'ctrl-shift-esc', label: 'Ctrl+Shift+Esc', keys: [0xA2, 0xA0, 0x1B] },
  { id: 'ctrl-w', label: 'Ctrl+W', keys: [0xA2, 0x57] },
  { id: 'ctrl-t', label: 'Ctrl+T', keys: [0xA2, 0x54] },
  { id: 'ctrl-n', label: 'Ctrl+N', keys: [0xA2, 0x4E] },
  { id: 'f11', label: 'F11', keys: [0x7A] },
  { id: 'print-screen', label: 'Print Screen', keys: [0x2C] },
  { id: 'ctrl-alt-del', label: 'Ctrl+Alt+Del', keys: [0xA2, 0xA4, 0x2E], notOnWindows: true },
];

/** The combinations offered for a remote computer (`hostOs` as it says it). */
export const combosFor = (hostOs: string) =>
  KEY_COMBOS.filter((combo) => !(combo.notOnWindows && hostOs.toLowerCase() === 'windows'));

/** A combination pressed: the keys down in order, then up in the opposite one. */
export const comboStrokes = (keys: number[]): KeyStroke[] => [
  ...keys.map((vk): KeyStroke => [vk, true]),
  ...[...keys].reverse().map((vk): KeyStroke => [vk, false]),
];
