// Clipboard sharing with the remote computer, over the `clipboard` data
// channel the agent opens (messages: agent/src/share.rs). Files have their
// own connection (fileSession.ts).
//
// Browsers limit clipboard access: reading and writing it freely needs a
// secure page (https or localhost) and the window focused. So:
// - remote -> here: written at once when allowed, otherwise kept and written
//   on the next key press or click in the stream (a user gesture);
// - here -> remote: the browser's paste event (Ctrl/Cmd+V) carries the
//   text; the paste key is held back briefly so the remote computer has the
//   text before the keys arrive. On secure pages the clipboard is also read
//   when the window gets focus.
import { t } from './i18n';

const MAX_CLIPBOARD = 1024 * 1024;

export const readClipboardSync = () => localStorage.getItem('lunaris_clipboard_sync') !== 'false';
export const readFileTransfer = () => localStorage.getItem('lunaris_file_transfer') !== 'false';

type Reply = { type: string; message?: string };

export class WebShare {
  private clipboard: RTCDataChannel | null = null;
  private lastText = '';
  private pendingCopy: string | null = null;
  private pasteKeys: (() => void)[] = [];
  private pasteTimer: number | null = null;

  private notify: (message: string) => void;

  constructor(notify: (message: string) => void) {
    this.notify = notify;
  }

  /** A data channel from the agent; true when it is one of ours. */
  attach(label: string, channel: RTCDataChannel): boolean {
    if (label === 'clipboard') {
      this.clipboard = channel;
      channel.onmessage = (e) => this.onClipboardMessage(e.data);
      return true;
    }
    return false;
  }

  detach() {
    this.clipboard = null;
  }

  // ---------------- clipboard: remote -> here

  private onClipboardMessage(data: unknown) {
    if (typeof data !== 'string') return;
    let msg: Reply & { text?: string };
    try { msg = JSON.parse(data); } catch { return; }
    if (msg.type === 'clipboard_error') {
      this.notify(t(msg.message || 'Clipboard sharing is turned off on the remote computer'));
      return;
    }
    if (msg.type !== 'clipboard' || typeof msg.text !== 'string' || !readClipboardSync()) return;
    this.lastText = msg.text;
    if (window.isSecureContext && navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
      navigator.clipboard.writeText(msg.text).catch(() => { this.pendingCopy = msg.text!; });
    } else {
      this.pendingCopy = msg.text;
    }
  }

  /** Call on a key press or click: writes remote text the browser refused earlier. */
  flushPendingCopy() {
    if (this.pendingCopy === null) return;
    const text = this.pendingCopy;
    this.pendingCopy = null;
    const active = document.activeElement as HTMLElement | null;
    const area = document.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    try { document.execCommand('copy'); } catch { /* not allowed */ }
    area.remove();
    active?.focus?.();
  }

  // ---------------- clipboard: here -> remote

  private sendText(text: string) {
    if (!readClipboardSync() || !this.clipboard || this.clipboard.readyState !== 'open') return;
    if (!text || text === this.lastText || text.length > MAX_CLIPBOARD) return;
    this.lastText = text;
    this.clipboard.send(JSON.stringify({ type: 'clipboard', text }));
  }

  /**
   * Ctrl/Cmd+V: returns true when the key is held back (don't preventDefault,
   * so the browser fires `paste`); `send` runs once the text went out.
   */
  holdPasteKey(e: KeyboardEvent, send: () => void): boolean {
    const isPaste = (e.ctrlKey || e.metaKey) && e.code === 'KeyV' && !e.altKey;
    if (!isPaste || !readClipboardSync() || !this.clipboard || this.clipboard.readyState !== 'open') return false;
    this.pasteKeys.push(send);
    if (this.pasteTimer === null) {
      this.pasteTimer = window.setTimeout(() => this.releasePasteKeys(), 200);
    }
    return true;
  }

  /** Key events after a held paste key wait for it (keeps their order). */
  queueAfterPaste(send: () => void): boolean {
    if (this.pasteTimer === null) return false;
    this.pasteKeys.push(send);
    return true;
  }

  private releasePasteKeys() {
    if (this.pasteTimer !== null) window.clearTimeout(this.pasteTimer);
    this.pasteTimer = null;
    const keys = this.pasteKeys;
    this.pasteKeys = [];
    keys.forEach(k => k());
  }

  /** document `paste` event while streaming. */
  onPaste(e: ClipboardEvent) {
    const text = e.clipboardData?.getData('text/plain');
    if (text) this.sendText(text);
    e.preventDefault();
    // Give the remote computer a moment to take the text, then the keys.
    if (this.pasteTimer !== null) {
      window.clearTimeout(this.pasteTimer);
      this.pasteTimer = window.setTimeout(() => this.releasePasteKeys(), 40);
    }
  }

  /** Window got focus: on secure pages, pick up what was copied meanwhile. */
  async syncOnFocus() {
    if (!window.isSecureContext || !navigator.clipboard || typeof navigator.clipboard.readText !== 'function' || !readClipboardSync()) return;
    try { this.sendText(await navigator.clipboard.readText()); } catch { /* permission */ }
  }
}
