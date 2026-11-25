// Connecting again after a session was lost (2026-11-25: a dropped
// connection used to leave the page on "Disconnected" until the person
// went back and connected again).
//
// Lost means: the peer connection failed, stayed "disconnected" for a
// while, or the signaling connection closed before the picture came. The
// host ending the session, a refusal or an error the server names is not
// retried.

/** Tries after a loss, each after the delay at its index. */
// Together longer than an agent restarting (an update takes 10-20 s): the
// server answers at once while the host is away, so the waits are the time.
export const RECONNECT_DELAYS_MS: readonly number[] = [3000, 8000, 15000];
export const MAX_RECONNECTS = RECONNECT_DELAYS_MS.length;
/** A connection that holds this long counts as recovered: the next loss gets all tries again. */
export const STABLE_AFTER_MS = 20_000;
/** A try that shows no picture within this long failed (an offline host
 *  does not answer at all). */
export const CONNECT_TIMEOUT_MS = 15_000;
/** What the server says to a session whose host went away (signaling.rs end_host_sessions). */
export const HOST_GONE_MESSAGE = 'Host disconnected';
/** A peer connection "disconnected" this long counts as lost (it often recovers by itself sooner). */
export const DISCONNECTED_GRACE_MS = 5000;

/** The delay before try number `attempt` (0-based), or null when none is left. */
export const reconnectDelay = (attempt: number): number | null =>
  attempt >= 0 && attempt < RECONNECT_DELAYS_MS.length ? RECONNECT_DELAYS_MS[attempt] : null;

/** Whether the signaling connection closing loses the session: not while
 *  the picture still comes (the peer connection lives on; its own failure
 *  is noticed apart). */
export const signalingCloseLosesSession = (peerState: RTCPeerConnectionState | null | undefined): boolean =>
  peerState !== 'connected';
