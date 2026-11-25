// Pointer lock and full screen, where the browser has them. Safari on an
// iPhone has neither (no Pointer Lock; full screen for videos only), and a
// call to a missing method throws: in the stream's clean-up that unmounted
// the whole page, leaving it black. Tests: tests/pageLocks.test.ts.

type LockDocument = Pick<Document, 'pointerLockElement' | 'fullscreenElement' | 'fullscreenEnabled'> & {
  exitPointerLock?: () => void;
  exitFullscreen?: () => Promise<void>;
};

/** Lets go of the pointer, when it is locked. */
export const exitPointerLock = (doc: LockDocument = document) => {
  if (doc.pointerLockElement && typeof doc.exitPointerLock === 'function') doc.exitPointerLock();
};

/** Whether this browser can show a page element full screen. */
export const canFullscreen = (doc: LockDocument = document): boolean =>
  !!doc.fullscreenEnabled;

/** Shows `el` full screen; rejects where the browser cannot. */
export const requestFullscreen = (el: Element): Promise<void> =>
  typeof el.requestFullscreen === 'function'
    ? el.requestFullscreen()
    : Promise.reject(new Error('Full screen is not available in this browser'));

/** Leaves full screen, when the page is full screen. */
export const exitFullscreen = (doc: LockDocument = document): Promise<void> =>
  doc.fullscreenElement && typeof doc.exitFullscreen === 'function' ? doc.exitFullscreen() : Promise.resolve();
