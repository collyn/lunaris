// Touch gestures for controlling a remote computer from a phone or tablet,
// modelled on RustDesk's mobile controls. Pure logic (no DOM): StreamPlayer
// feeds it the touches and turns its actions into mouse input and view zoom.
//
//                      trackpad (default)             direct
//   1 finger drag      move the cursor (like a        pan the view when zoomed
//                      laptop touchpad)
//   tap                left click at the cursor       left click where touched
//   long press         right click                    right click where touched
//   tap, then touch    drag (button held while        drag from where touched
//   and drag           the finger moves)
//   2 finger tap       right click                    right click
//   2 finger drag      scroll                         scroll
//   pinch              zoom around the fingers        zoom around the fingers

export type TouchMode = 'trackpad' | 'direct';

export interface TouchPoint {
  id: number;
  x: number;
  y: number;
}

export interface GestureActions {
  /** Trackpad: move the cursor by this many screen pixels. */
  moveBy(dx: number, dy: number): void;
  /** Direct: put the cursor at this screen point. */
  moveTo(x: number, y: number): void;
  /** Mouse button 1 (left) or 3 (right) down/up. */
  button(button: 1 | 3, down: boolean): void;
  /** Scroll by this many screen pixels of finger movement (+y: fingers moved down). */
  scroll(dx: number, dy: number): void;
  /** Move the zoomed view by screen pixels. */
  pan(dx: number, dy: number): void;
  /** A pinch began at `mid` (screen point). */
  pinchStart(mid: { x: number; y: number }): void;
  /** Pinch: distance between the fingers / at the start, and the midpoint now. */
  pinch(ratio: number, mid: { x: number; y: number }): void;
  pinchEnd(): void;
  /** Short vibration as feedback (right click, drag start). */
  haptic?(ms: number): void;
}

export interface GestureTiming {
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/** Thresholds, in ms and screen px. */
export const GESTURE = {
  tapMaxMs: 300,
  slopPx: 10,
  longPressMs: 500,
  /** A touch starting this soon after a tap (and near it) can start a drag. */
  dragAfterTapMs: 320,
  dragAfterTapPx: 60,
  twoFingerTapMaxMs: 300,
  pinchStartPx: 24,
  twoFingerMoveStartPx: 12,
  clickHoldMs: 30,
};

type OneFinger = {
  id: number;
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  startTime: number;
  moved: boolean;
  /** Started right after a tap: moving turns it into a drag. */
  dragCandidate: boolean;
  dragging: boolean;
  longPressed: boolean;
};

type TwoFinger = {
  startTime: number;
  startDist: number;
  startMid: { x: number; y: number };
  lastMid: { x: number; y: number };
  mode: 'undecided' | 'pinch' | 'scroll';
};

const dist = (a: TouchPoint, b: TouchPoint) => Math.hypot(a.x - b.x, a.y - b.y);
const mid = (a: TouchPoint, b: TouchPoint) => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });

export class TouchGestures {
  mode: TouchMode;
  private one: OneFinger | null = null;
  private two: TwoFinger | null = null;
  /** Ignore the touches left after a multi-finger gesture until all lift. */
  private waitAllUp = false;
  private lastTap: { time: number; x: number; y: number } | null = null;
  private longPressTimer: unknown = null;
  private readonly actions: GestureActions;
  private readonly timing: GestureTiming;

  constructor(mode: TouchMode, actions: GestureActions, timing?: GestureTiming) {
    this.mode = mode;
    this.actions = actions;
    this.timing = timing ?? {
      now: () => performance.now(),
      setTimeout: (fn, ms) => window.setTimeout(fn, ms),
      clearTimeout: (h) => window.clearTimeout(h as number),
    };
  }

  /** All touches currently on the screen, after a touch started. */
  start(touches: TouchPoint[]) {
    const now = this.timing.now();
    if (touches.length === 1 && !this.waitAllUp) {
      const t = touches[0];
      const last = this.lastTap;
      const dragCandidate = !!last
        && now - last.time < GESTURE.dragAfterTapMs
        && Math.hypot(t.x - last.x, t.y - last.y) < GESTURE.dragAfterTapPx;
      this.one = {
        id: t.id, startX: t.x, startY: t.y, lastX: t.x, lastY: t.y, startTime: now,
        moved: false, dragCandidate, dragging: false, longPressed: false,
      };
      this.cancelLongPress();
      if (!dragCandidate) {
        this.longPressTimer = this.timing.setTimeout(() => this.onLongPress(), GESTURE.longPressMs);
      }
      return;
    }
    if (touches.length === 2) {
      this.cancelLongPress();
      // A second finger ends a one-finger gesture.
      if (this.one?.dragging) this.actions.button(1, false);
      this.one = null;
      this.lastTap = null;
      const m = mid(touches[0], touches[1]);
      this.two = { startTime: now, startDist: dist(touches[0], touches[1]), startMid: m, lastMid: m, mode: 'undecided' };
      this.waitAllUp = true;
      return;
    }
    if (touches.length > 2) {
      this.endTwo(false);
      this.cancelLongPress();
      this.one = null;
      this.waitAllUp = true;
    }
  }

  move(touches: TouchPoint[]) {
    if (this.one && touches.length === 1) {
      const t = touches.find((p) => p.id === this.one!.id) ?? touches[0];
      const o = this.one;
      const dx = t.x - o.lastX;
      const dy = t.y - o.lastY;
      o.lastX = t.x;
      o.lastY = t.y;
      if (!o.moved && Math.hypot(t.x - o.startX, t.y - o.startY) > GESTURE.slopPx) {
        o.moved = true;
        this.cancelLongPress();
        if (o.dragCandidate) {
          o.dragging = true;
          if (this.mode === 'direct') this.actions.moveTo(o.startX, o.startY);
          this.actions.button(1, true);
          this.actions.haptic?.(30);
        }
      }
      if (o.longPressed) return;
      if (this.mode === 'trackpad') {
        // The cursor follows the finger from the first pixel (like a
        // touchpad); a tap still counts if it stayed within the slop.
        if (dx !== 0 || dy !== 0) this.actions.moveBy(dx, dy);
      } else if (o.dragging) {
        this.actions.moveTo(t.x, t.y);
      } else if (o.moved && (dx !== 0 || dy !== 0)) {
        this.actions.pan(dx, dy);
      }
      return;
    }
    if (this.two && touches.length >= 2) {
      const tw = this.two;
      const m = mid(touches[0], touches[1]);
      const d = dist(touches[0], touches[1]);
      if (tw.mode === 'undecided') {
        if (Math.abs(d - tw.startDist) > GESTURE.pinchStartPx) {
          tw.mode = 'pinch';
          this.actions.pinchStart(tw.startMid);
        } else if (Math.hypot(m.x - tw.startMid.x, m.y - tw.startMid.y) > GESTURE.twoFingerMoveStartPx) {
          tw.mode = 'scroll';
          tw.lastMid = tw.startMid;
        }
      }
      if (tw.mode === 'pinch') {
        this.actions.pinch(tw.startDist > 0 ? d / tw.startDist : 1, m);
      } else if (tw.mode === 'scroll') {
        this.actions.scroll(m.x - tw.lastMid.x, m.y - tw.lastMid.y);
      }
      tw.lastMid = m;
    }
  }

  /** The touches still on the screen after one ended (or was cancelled). */
  end(remaining: TouchPoint[], cancelled = false) {
    const now = this.timing.now();
    if (this.two) {
      if (remaining.length < 2) this.endTwo(!cancelled);
    } else if (this.one && !remaining.some((p) => p.id === this.one!.id)) {
      const o = this.one;
      this.one = null;
      this.cancelLongPress();
      if (o.dragging) {
        this.actions.button(1, false);
      } else if (!cancelled && !o.longPressed && !o.moved && now - o.startTime < GESTURE.tapMaxMs) {
        if (this.mode === 'direct') this.actions.moveTo(o.startX, o.startY);
        this.click(1);
        // A second tap right after is a click too (double click), but
        // touching and dragging is a drag.
        this.lastTap = o.dragCandidate ? null : { time: now, x: o.startX, y: o.startY };
        return;
      }
      this.lastTap = null;
    }
    if (remaining.length === 0) this.waitAllUp = false;
  }

  /** Stop everything (stream ended, mode changed): releases a held button. */
  reset() {
    this.cancelLongPress();
    if (this.one?.dragging) this.actions.button(1, false);
    this.endTwo(false);
    this.one = null;
    this.two = null;
    this.lastTap = null;
    this.waitAllUp = false;
  }

  private endTwo(allowTap: boolean) {
    const tw = this.two;
    if (!tw) return;
    this.two = null;
    if (tw.mode === 'pinch') this.actions.pinchEnd();
    else if (allowTap && tw.mode === 'undecided' && this.timing.now() - tw.startTime < GESTURE.twoFingerTapMaxMs) {
      this.click(3);
      this.actions.haptic?.(40);
    }
  }

  private onLongPress() {
    this.longPressTimer = null;
    const o = this.one;
    if (!o || o.moved || o.dragCandidate) return;
    o.longPressed = true;
    if (this.mode === 'direct') this.actions.moveTo(o.startX, o.startY);
    this.click(3);
    this.actions.haptic?.(50);
  }

  private click(button: 1 | 3) {
    this.actions.button(button, true);
    this.timing.setTimeout(() => this.actions.button(button, false), GESTURE.clickHoldMs);
  }

  private cancelLongPress() {
    if (this.longPressTimer !== null) {
      this.timing.clearTimeout(this.longPressTimer);
      this.longPressTimer = null;
    }
  }
}
