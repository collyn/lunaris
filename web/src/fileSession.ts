// File transfer with a remote computer over a connection of its own (its own
// signaling WebSocket and WebRTC connection, like RustDesk's file-transfer
// connection), so transfers never slow the stream's video or input.
// Protocol: agent/src/files.rs.

import { t } from './i18n';

/** webrtc-rs (the agent) closes a channel on messages over 65535 bytes. */
const CHUNK = 48 * 1024;
const MAX_BUFFERED = 4 * 1024 * 1024;

export type RemoteEntry = { name: string; dir: boolean; size: number; mtime: number; hidden: boolean };
export type RemoteDir = { path: string; parent: string | null; entries: RemoteEntry[] };
export type Hello = { home: string; downloads: string; os: string; sep: string; roots: { name: string; path: string }[] };

type Reply = { type: string; id?: string; message?: string; [k: string]: unknown };

export type TransferState = 'queued' | 'running' | 'done' | 'skipped' | 'failed' | 'cancelled';
export type Transfer = {
  id: string;
  direction: 'up' | 'down';
  name: string;
  /** Remote path (destination of uploads, source of downloads). */
  remotePath: string;
  size: number;
  done: number;
  state: TransferState;
  message?: string;
  startedAt?: number;
};

export const humanSize = (b: number) =>
  b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : b >= 1 << 20 ? `${(b / (1 << 20)).toFixed(1)} MB` : b >= 1024 ? `${Math.round(b / 1024)} KB` : `${b} B`;

/** Joins remote path parts with the remote computer's separator. */
export const joinRemote = (dir: string, name: string, sep: string) =>
  dir.endsWith('/') || dir.endsWith('\\') ? dir + name : dir + sep + name;

type Opts = { wsUrl: string; hostId: string; onClose?: (reason: string) => void };

export class FileSession {
  private ws: WebSocket | null = null;
  private pc: RTCPeerConnection | null = null;
  private ch: RTCDataChannel | null = null;
  private waiting = new Map<string, (r: Reply) => void>();
  private seq = 0;
  /** Binary chunks of the download in progress go here. */
  private sink: ((data: ArrayBuffer) => void) | null = null;
  private closed = false;
  hello: Hello | null = null;

  private opts: Opts;
  constructor(opts: Opts) {
    this.opts = opts;
  }

  get open() {
    return !!this.ch && this.ch.readyState === 'open';
  }

  /** Connects (signaling, WebRTC, the `files` channel) and says hello. */
  connect(timeoutMs = 20000): Promise<Hello> {
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => { this.close(); reject(new Error(t('The remote computer did not answer'))); }, timeoutMs);
      const fail = (e: Error) => { window.clearTimeout(timer); this.close(); reject(e); };
      const ws = new WebSocket(this.opts.wsUrl);
      this.ws = ws;
      const send = (type: string, payload: object) => ws.readyState === WebSocket.OPEN && ws.send(JSON.stringify({ event: 'Signaling', data: { type, payload } }));
      ws.onopen = () => send('RequestFileSession', { host_id: this.opts.hostId });
      ws.onerror = () => fail(new Error(t('Cannot reach the server')));
      ws.onclose = () => { if (!this.open) fail(new Error(t('Connection closed'))); else this.closedBy(t('Connection closed')); };
      ws.onmessage = async (event) => {
        let msg: { event?: string; data?: { type: string; payload: any } };
        try { msg = JSON.parse(event.data); } catch { return; }
        if (msg.event !== 'Signaling' || !msg.data) return;
        const { type, payload } = msg.data;
        if (type === 'Error') { fail(new Error(payload?.message || t('Error'))); return; }
        if (type === 'Sdp' && payload?.sdp?.ty === 'offer') {
          const servers = (payload.ice_servers || []) as { urls: string[]; username?: string; credential?: string }[];
          const pc = new RTCPeerConnection({ iceServers: servers.length ? servers : [{ urls: 'stun:stun.l.google.com:19302' }] });
          this.pc = pc;
          pc.onicecandidate = (e) => {
            if (!e.candidate) return;
            send('IceCandidate', {
              target_id: this.opts.hostId,
              candidate: { candidate: e.candidate.candidate, sdp_mid: e.candidate.sdpMid, sdp_mline_index: e.candidate.sdpMLineIndex, username_fragment: e.candidate.usernameFragment || null },
            });
          };
          pc.onconnectionstatechange = () => {
            if (pc.connectionState === 'failed') fail(new Error(t('Could not connect to the remote computer')));
            if (pc.connectionState === 'disconnected' || pc.connectionState === 'closed') this.closedBy(t('Connection closed'));
          };
          pc.ondatachannel = (e) => {
            if (e.channel.label !== 'files') return;
            const ch = e.channel;
            ch.binaryType = 'arraybuffer';
            this.ch = ch;
            ch.onmessage = (m) => this.onMessage(m.data);
            ch.onclose = () => this.closedBy(t('Connection closed'));
            const ready = async () => {
              try {
                const h = await this.request({ type: 'hello' });
                if (h.type === 'error') throw new Error(t(String(h.message)));
                this.hello = h as unknown as Hello;
                window.clearTimeout(timer);
                resolve(this.hello);
              } catch (err) { fail(err as Error); }
            };
            if (ch.readyState === 'open') void ready(); else ch.onopen = () => void ready();
          };
          await pc.setRemoteDescription({ type: 'offer', sdp: payload.sdp.sdp });
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          send('Sdp', { target_id: this.opts.hostId, sdp: { ty: 'answer', sdp: answer.sdp }, ice_servers: null, webtransport_port: null, webtransport_cert_hash: null });
        } else if (type === 'IceCandidate' && this.pc) {
          try {
            await this.pc.addIceCandidate({ candidate: payload.candidate.candidate, sdpMid: payload.candidate.sdp_mid, sdpMLineIndex: payload.candidate.sdp_mline_index, usernameFragment: payload.candidate.username_fragment });
          } catch { /* stale */ }
        }
      };
    });
  }

  private closedBy(reason: string) {
    if (this.closed) return;
    this.closed = true;
    for (const w of this.waiting.values()) w({ type: 'error', message: reason });
    this.waiting.clear();
    this.opts.onClose?.(reason);
  }

  close() {
    this.closed = true;
    try { this.ch?.close(); } catch { /* */ }
    try { this.pc?.close(); } catch { /* */ }
    try { this.ws?.close(); } catch { /* */ }
    this.ch = null; this.pc = null; this.ws = null;
  }

  private onMessage(data: unknown) {
    if (typeof data !== 'string') {
      this.sink?.(data as ArrayBuffer);
      return;
    }
    let r: Reply;
    try { r = JSON.parse(data); } catch { return; }
    if (r.id && this.waiting.has(r.id)) this.waiting.get(r.id)!(r);
  }

  private nextId() {
    this.seq += 1;
    return `w${Date.now().toString(36)}${this.seq}`;
  }

  /** Sends a request and waits for its first answer. */
  private request(body: Record<string, unknown>, ms = 30000): Promise<Reply> {
    const id = (body.id as string) || this.nextId();
    return new Promise((resolve) => {
      const timer = window.setTimeout(() => { this.waiting.delete(id); resolve({ type: 'error', id, message: t('No answer from the remote computer') }); }, ms);
      this.waiting.set(id, (r) => { window.clearTimeout(timer); this.waiting.delete(id); resolve(r); });
      try { this.ch?.send(JSON.stringify({ ...body, id })); } catch (e) { this.waiting.get(id)?.({ type: 'error', id, message: String(e) }); }
    });
  }

  private async ok(body: Record<string, unknown>, ms?: number): Promise<Reply> {
    const r = await this.request(body, ms);
    if (r.type === 'error') throw new Error(String(r.message || t('Error')));
    return r;
  }

  async list(path: string, hidden = false): Promise<RemoteDir> {
    return (await this.ok({ type: 'list', path, hidden })) as unknown as RemoteDir;
  }
  async walk(path: string): Promise<{ files: { rel: string; size: number; mtime: number }[]; dirs: string[] }> {
    return (await this.ok({ type: 'walk', path }, 120000)) as any;
  }
  async mkdir(path: string) { await this.ok({ type: 'mkdir', path }); }
  async remove(path: string) { await this.ok({ type: 'remove', path }, 120000); }
  async rename(path: string, name: string) { await this.ok({ type: 'rename', path, name }); }

  cancel(id: string) {
    try { this.ch?.send(JSON.stringify({ type: 'cancel', id })); } catch { /* */ }
  }

  private async drain() {
    const ch = this.ch;
    while (ch && ch.bufferedAmount > MAX_BUFFERED) {
      if (ch.readyState !== 'open') throw new Error(t('Connection closed'));
      await new Promise(r => setTimeout(r, 5));
    }
  }

  /**
   * Uploads `file` to `remotePath` (a full path). The remote computer skips
   * identical files and resumes an interrupted upload of the same file.
   */
  async upload(tr: Transfer, file: File, onProgress: () => void, isCancelled: () => boolean, onExists: 'rename' | 'replace' | 'skip' = 'rename') {
    const mtime = Math.floor(file.lastModified / 1000);
    const ready = await this.request({ type: 'put', id: tr.id, path: tr.remotePath, size: file.size, mtime, on_exists: onExists });
    if (ready.type === 'error') throw new Error(String(ready.message));
    if (ready.type === 'put_skip') {
      tr.state = 'skipped';
      tr.message = ready.reason === 'identical' ? t('Already there (same file)') : t('Already exists');
      tr.done = tr.size;
      return;
    }
    if (typeof ready.path === 'string') tr.remotePath = ready.path;
    let sent = Number(ready.offset || 0);
    tr.done = sent;
    const ch = this.ch!;
    while (sent < file.size) {
      if (isCancelled()) { this.cancel(tr.id); tr.state = 'cancelled'; return; }
      const chunk = await file.slice(sent, sent + CHUNK).arrayBuffer();
      await this.drain();
      ch.send(chunk);
      sent += chunk.byteLength;
      tr.done = sent;
      onProgress();
    }
    const end = await this.request({ type: 'put_end', id: tr.id }, 600000);
    if (end.type !== 'done') throw new Error(String(end.message || t('Failed')));
    tr.remotePath = String(end.path || tr.remotePath);
    tr.state = 'done';
  }

  /**
   * Downloads a remote file, handing chunks to `write` (in order). A
   * cancelled download still waits for the remote side's `get_end`, so none
   * of its chunks can reach the next one.
   */
  async download(tr: Transfer, write: (chunk: ArrayBuffer) => Promise<void> | void, onProgress: () => void, isCancelled: () => boolean) {
    let pending: Promise<void> = Promise.resolve();
    let failed: Error | null = null;
    let cancelSent = false;
    const finished = new Promise<Reply>((resolve) => {
      this.waiting.set(tr.id, (r) => {
        if (r.type === 'get_start') {
          tr.size = Number(r.size || 0);
          this.sink = (data) => {
            if (cancelSent) return;
            if (isCancelled()) { cancelSent = true; this.cancel(tr.id); return; }
            tr.done += data.byteLength;
            pending = pending.then(() => write(data)).catch((e) => { failed = e; });
            onProgress();
          };
          return;
        }
        this.sink = null;
        this.waiting.delete(tr.id);
        resolve(r);
      });
    });
    this.ch!.send(JSON.stringify({ type: 'get', id: tr.id, path: tr.remotePath, offset: 0 }));
    const r = await finished;
    await pending;
    if (failed) throw failed;
    if (r.cancelled || cancelSent) { tr.state = 'cancelled'; return; }
    if (r.type !== 'get_end') throw new Error(String(r.message || t('Failed')));
    tr.state = 'done';
  }
}
