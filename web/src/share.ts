// Clipboard and file sharing with the remote computer, over the `clipboard`
// and `file` data channels the agent opens (messages: agent/src/share.rs).
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

// The agent (webrtc-rs) reads messages into a 65535-byte buffer and closes
// the channel on anything larger; 16 KB is also the size every browser takes.
const CHUNK = 16 * 1024;
const MAX_BUFFERED = 4 * 1024 * 1024;
const MAX_CLIPBOARD = 1024 * 1024;

export const readClipboardSync = () => localStorage.getItem('lunaris_clipboard_sync') !== 'false';
export const readFileTransfer = () => localStorage.getItem('lunaris_file_transfer') !== 'false';

const humanSize = (b: number) =>
  b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : b >= 1 << 20 ? `${(b / (1 << 20)).toFixed(1)} MB` : b >= 1024 ? `${Math.round(b / 1024)} KB` : `${b} B`;

type Reply = { type: string; id?: string; path?: string; message?: string };

export class WebShare {
  private clipboard: RTCDataChannel | null = null;
  private file: RTCDataChannel | null = null;
  private lastText = '';
  private pendingCopy: string | null = null;
  private pasteKeys: (() => void)[] = [];
  private pasteTimer: number | null = null;
  private replies: ((r: Reply) => void) | null = null;
  private sending = false;

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
    if (label === 'file') {
      this.file = channel;
      channel.binaryType = 'arraybuffer';
      channel.onmessage = (e) => {
        if (typeof e.data !== 'string') return;
        try { this.replies?.(JSON.parse(e.data)); } catch { /* ignore */ }
      };
      return true;
    }
    return false;
  }

  detach() {
    this.clipboard = null;
    this.file = null;
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

  // ---------------- files

  private waitReply(id: string, ms: number): Promise<Reply> {
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => { this.replies = null; reject(new Error(t('no answer from the remote computer'))); }, ms);
      this.replies = (r) => {
        if (r.id !== id) return;
        window.clearTimeout(timer);
        this.replies = null;
        resolve(r);
      };
    });
  }

  private async drain(ch: RTCDataChannel) {
    while (ch.bufferedAmount > MAX_BUFFERED) {
      if (ch.readyState !== 'open') throw new Error(t('connection closed'));
      await new Promise(r => setTimeout(r, 10));
    }
  }

  /** Sends files to the remote computer's Downloads/Lunaris. */
  async sendFiles(files: File[]) {
    if (!files.length) return;
    if (!readFileTransfer()) { this.notify(t('File transfer is turned off in Settings')); return; }
    const ch = this.file;
    if (!ch || ch.readyState !== 'open') { this.notify(t('Connect to a computer to send files')); return; }
    if (this.sending) { this.notify(t("Already sending files; try again when it's done")); return; }
    this.sending = true;
    try {
      for (let i = 0; i < files.length; i++) {
        const f = files[i];
        const id = `f${Date.now().toString(36)}${i}`;
        const prefix = files.length > 1 ? `(${i + 1}/${files.length}) ` : '';
        ch.send(JSON.stringify({ type: 'file_start', id, name: f.name, size: f.size }));
        const accepted = await this.waitReply(id, 15000);
        if (accepted.type === 'file_error') throw new Error(`${f.name}: ${t(accepted.message || 'refused')}`);
        let sent = 0;
        let lastReport = 0;
        while (sent < f.size) {
          const chunk = await f.slice(sent, sent + CHUNK).arrayBuffer();
          await this.drain(ch);
          ch.send(chunk);
          sent += chunk.byteLength;
          if (performance.now() - lastReport > 400) {
            lastReport = performance.now();
            this.notify(`${prefix}${t('Sending %1: %2% of %3', f.name, Math.floor((sent * 100) / Math.max(1, f.size)), humanSize(f.size))}`);
          }
        }
        ch.send(JSON.stringify({ type: 'file_end', id }));
        const done = await this.waitReply(id, 600000);
        if (done.type !== 'file_saved') throw new Error(`${f.name}: ${t(done.message || 'failed')}`);
        this.notify(t('Sent %1 → %2', f.name, done.path || ''));
      }
      if (files.length > 1) this.notify(t('Sent %1 files', files.length));
    } catch (e) {
      this.notify(String((e as Error).message || e));
    } finally {
      this.sending = false;
    }
  }
}
