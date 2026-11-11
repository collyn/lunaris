// Game controllers: reads this browser's controllers (Gamepad API) and sends
// their state to the remote computer on the `gamepad` data channel, where
// each becomes a virtual Xbox 360 controller (agent/src/gamepad.rs has the
// packet format).
//
// Browsers only expose controllers on secure pages (https or localhost), and
// only after a button is pressed while the page has focus.

export const readGamepadEnabled = () => localStorage.getItem('lunaris_gamepad') !== 'false';

export type PadKind = 'xbox' | 'playstation' | 'nintendo' | 'generic';

export const B = {
  A: 0x0001, B: 0x0002, X: 0x0004, Y: 0x0008,
  UP: 0x0010, DOWN: 0x0020, LEFT: 0x0040, RIGHT: 0x0080,
  LB: 0x0100, RB: 0x0200, START: 0x0400, BACK: 0x0800,
  LS: 0x1000, RS: 0x2000, GUIDE: 0x4000, TOUCHPAD: 0x8000,
} as const;

/** Standard-mapping button index -> our bit (6/7 are the analog triggers). */
const STANDARD: [number, number][] = [
  [0, B.A], [1, B.B], [2, B.X], [3, B.Y], [4, B.LB], [5, B.RB],
  [8, B.BACK], [9, B.START], [10, B.LS], [11, B.RS],
  [12, B.UP], [13, B.DOWN], [14, B.LEFT], [15, B.RIGHT], [16, B.GUIDE], [17, B.TOUCHPAD],
];

export function padKind(id: string): PadKind {
  const s = id.toLowerCase();
  if (/054c|playstation|dualsense|dualshock|wireless controller|ps[345]/.test(s)) return 'playstation';
  if (/045e|xbox|xinput|x-box/.test(s)) return 'xbox';
  if (/057e|nintendo|pro controller|joy-con/.test(s)) return 'nintendo';
  return 'generic';
}

export const KIND_CODE: Record<PadKind, number> = { generic: 0, xbox: 1, playstation: 2, nintendo: 3 };

export type PadState = {
  index: number;
  name: string;
  kind: PadKind;
  standard: boolean;
  buttons: number;
  lt: number; // 0..255
  rt: number;
  lx: number; // -32768..32767, y grows downwards
  ly: number;
  rx: number;
  ry: number;
  rumble: boolean;
};

const stick = (v: number | undefined) => Math.max(-32768, Math.min(32767, Math.round((v || 0) * 32767)));
const trigger = (b: GamepadButton | undefined) => Math.max(0, Math.min(255, Math.round((b ? Math.max(b.value, b.pressed ? 1 : 0) : 0) * 255)));

export function readPad(gp: Gamepad): PadState {
  let buttons = 0;
  for (const [i, bit] of STANDARD) {
    if (gp.buttons[i]?.pressed) buttons |= bit;
  }
  return {
    index: gp.index,
    name: gp.id.replace(/\s*\(.*?Vendor.*?\)\s*/i, ' ').trim() || `Controller ${gp.index + 1}`,
    kind: padKind(gp.id),
    standard: gp.mapping === 'standard',
    buttons,
    lt: trigger(gp.buttons[6]),
    rt: trigger(gp.buttons[7]),
    lx: stick(gp.axes[0]),
    ly: stick(gp.axes[1]),
    rx: stick(gp.axes[2]),
    ry: stick(gp.axes[3]),
    rumble: !!(gp as Gamepad & { vibrationActuator?: unknown }).vibrationActuator,
  };
}

export function connectedPads(): PadState[] {
  if (typeof navigator.getGamepads !== 'function') return [];
  let list: (Gamepad | null)[] = [];
  try { list = Array.from(navigator.getGamepads()); } catch { return []; }
  return list.filter((g): g is Gamepad => !!g && g.connected).map(readPad);
}

/** Why controllers can't work here, or null when they can. */
export function gamepadUnavailableReason(): string | null {
  if (!window.isSecureContext) return 'Controllers need a secure page: open the web console over https (or on localhost).';
  if (typeof navigator.getGamepads !== 'function') return 'This browser does not support game controllers.';
  return null;
}

export async function vibrate(index: number, ms = 600) {
  const gp = navigator.getGamepads?.()[index] as (Gamepad & { vibrationActuator?: { playEffect?: (t: string, p: object) => Promise<unknown> } }) | null;
  try {
    await gp?.vibrationActuator?.playEffect?.('dual-rumble', { startDelay: 0, duration: ms, weakMagnitude: 1, strongMagnitude: 1 });
  } catch { /* not supported */ }
}

function encodeState(id: number, s: PadState): ArrayBuffer {
  const buf = new ArrayBuffer(17);
  const v = new DataView(buf);
  v.setUint8(0, 0);
  v.setUint8(1, id);
  v.setUint8(2, KIND_CODE[s.kind]);
  v.setUint32(3, s.buttons >>> 0, false);
  v.setUint8(7, s.lt);
  v.setUint8(8, s.rt);
  v.setInt16(9, s.lx, false);
  v.setInt16(11, s.ly, false);
  v.setInt16(13, s.rx, false);
  v.setInt16(15, s.ry, false);
  return buf;
}

const same = (a: PadState, b: PadState) =>
  a.buttons === b.buttons && a.lt === b.lt && a.rt === b.rt && a.lx === b.lx && a.ly === b.ly && a.rx === b.rx && a.ry === b.ry;

/** Sends this browser's controllers while a session is open. */
export class GamepadSender {
  private channel: RTCDataChannel | null = null;
  private raf: number | null = null;
  private last = new Map<number, { state: PadState; at: number }>();
  private onError: (message: string) => void;

  constructor(onError: (message: string) => void) {
    this.onError = onError;
  }

  attach(channel: RTCDataChannel) {
    this.channel = channel;
    channel.binaryType = 'arraybuffer';
    channel.onmessage = (e) => {
      if (typeof e.data !== 'string') return;
      try {
        const msg = JSON.parse(e.data);
        if (msg.type === 'gamepad_error' && msg.message) this.onError(msg.message);
      } catch { /* ignore */ }
    };
    this.start();
  }

  detach() {
    this.stop();
    this.channel = null;
    this.last.clear();
  }

  private start() {
    if (this.raf !== null) return;
    const tick = () => {
      this.raf = requestAnimationFrame(tick);
      this.poll();
    };
    this.raf = requestAnimationFrame(tick);
  }

  private stop() {
    if (this.raf !== null) cancelAnimationFrame(this.raf);
    this.raf = null;
  }

  private send(data: ArrayBuffer) {
    const ch = this.channel;
    if (!ch || ch.readyState !== 'open') return;
    try { ch.send(data); } catch { /* closing */ }
  }

  private poll() {
    if (!this.channel || this.channel.readyState !== 'open') return;
    const enabled = readGamepadEnabled() && document.visibilityState === 'visible';
    const pads = enabled ? connectedPads() : [];
    const now = performance.now();
    const seen = new Set<number>();
    for (const pad of pads) {
      // Up to 8 controllers, numbered by the browser's slot.
      if (pad.index >= 8) continue;
      seen.add(pad.index);
      const prev = this.last.get(pad.index);
      // Send changes at once and repeat the state every second (the remote
      // side unplugs controllers it hasn't heard from in 5 s).
      if (!prev || !same(prev.state, pad) || now - prev.at > 1000) {
        this.send(encodeState(pad.index, pad));
        this.last.set(pad.index, { state: pad, at: now });
      }
    }
    for (const index of Array.from(this.last.keys())) {
      if (!seen.has(index)) {
        this.last.delete(index);
        const buf = new Uint8Array([1, index]).buffer;
        this.send(buf);
      }
    }
  }
}
