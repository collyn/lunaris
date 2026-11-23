// Moves of a locked pointer (the "Lock Pointer" button, games).
//
// While the pointer is locked the viewer sends how far the mouse moved, not
// where it is: packet type 0 (`InboundPacket::MouseMove` in
// agent/src/input.rs), which the agent injects as a relative move
// (`SendInput` without MOUSEEVENTF_ABSOLUTE on Windows). Games that read the
// mouse themselves in a match (League of Legends, shooters) take only relative
// moves: an absolute position sends their pointer to an edge of the screen.
// Moonlight sends a locked mouse the same way.
//
// Free of the DOM (tests: tests/relativePointer.test.ts, `npm run test:pointer`).

/** Largest move one packet carries (its fields are i16). */
export const MAX_STEP = 32767;

export interface RelativeStep {
  /** Whole pixels to send now (0 when nothing whole has built up). */
  dx: number;
  dy: number;
  /** What is left (fractions of a pixel, or beyond one packet) for the next send. */
  restX: number;
  restY: number;
}

const whole = (value: number): number => {
  const t = Math.trunc(value);
  return Math.max(-MAX_STEP, Math.min(MAX_STEP, t === 0 ? 0 : t)); // no -0
};

/**
 * Splits what the pointer moved since the last send into whole pixels to send
 * now and a rest kept for the next one: a high-resolution mouse reports
 * fractions of a pixel, and dropping them would slow slow moves down.
 */
export const takeRelativeStep = (accX: number, accY: number): RelativeStep => {
  const dx = whole(accX);
  const dy = whole(accY);
  return { dx, dy, restX: accX - dx, restY: accY - dy };
};

/** Packet type 0: u8 0, i16 dx, i16 dy, u32 seq (big-endian), 9 bytes. */
export const relativeMovePacket = (dx: number, dy: number, seq: number): ArrayBuffer => {
  const buffer = new ArrayBuffer(9);
  const view = new DataView(buffer);
  view.setUint8(0, 0);
  view.setInt16(1, dx, false);
  view.setInt16(3, dy, false);
  view.setUint32(5, seq >>> 0, false);
  return buffer;
};
