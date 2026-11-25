// The page notices when the server has a newer build of it and reloads.
//
// index.html is served with "no-cache", but phones rarely load it again: a
// tab left in the background, a page restored from the back/forward cache or
// a web app on the home screen keeps running the code it started with, for
// days. So the page asks for index.html again when it comes back to the
// front (and every few minutes while it is shown), compares the script it
// names with its own, and reloads when they differ — never during a session.

/** The fingerprinted entry script ("assets/index-AbC123.js") an HTML page names, or null. */
export function entryScript(html: string): string | null {
  const m = /<script[^>]*\bsrc="([^"]*assets\/[^"]+\.js)"/i.exec(html);
  return m ? m[1].replace(/^.*?(assets\/)/, '$1') : null;
}

/** Whether the server's page is another build than this one. Unknown either way → no. */
export const isNewerBuild = (mine: string | null, servers: string | null): boolean =>
  !!mine && !!servers && mine !== servers;

/** This page's own entry script, read from the document it was started from. */
export function ownEntryScript(doc: Document): string | null {
  for (const s of Array.from(doc.querySelectorAll('script[src]'))) {
    const found = entryScript(`<script src="${s.getAttribute('src') ?? ''}">`);
    if (found) return found;
  }
  return null;
}

const CHECK_EVERY_MS = 5 * 60 * 1000;
/** Coming back to the front more often than this asks the server only once. */
const MIN_GAP_MS = 15 * 1000;

/**
 * Starts watching for a newer build. `busy()` is true while a session is open:
 * then the reload waits, and `whenIdle` (returned) should be called when the
 * session ends. Returns a function that stops watching.
 */
export function watchForNewBuild(busy: () => boolean): { stop: () => void; whenIdle: () => void } {
  const mine = ownEntryScript(document);
  // Development (vite serves /src/main.tsx): nothing to compare.
  if (!mine) return { stop: () => {}, whenIdle: () => {} };

  let pending = false;
  let lastCheck = 0;
  let stopped = false;

  const reloadIfIdle = () => {
    if (pending && !busy() && !stopped) window.location.reload();
  };

  const check = async () => {
    if (stopped || pending || document.visibilityState !== 'visible') return;
    const now = Date.now();
    if (now - lastCheck < MIN_GAP_MS) return;
    lastCheck = now;
    try {
      const res = await fetch(`./index.html?fresh=${now}`, { cache: 'no-store' });
      if (!res.ok) return;
      if (isNewerBuild(mine, entryScript(await res.text()))) {
        pending = true;
        console.info('[Lunaris] a newer page is on the server; reloading when no session is open');
        reloadIfIdle();
      }
    } catch {
      // Offline or the server restarting: ask again later.
    }
  };

  const onVisible = () => { if (document.visibilityState === 'visible') void check(); };
  const onPageShow = (e: PageTransitionEvent) => { if (e.persisted) { lastCheck = 0; void check(); } };
  document.addEventListener('visibilitychange', onVisible);
  window.addEventListener('pageshow', onPageShow);
  window.addEventListener('focus', onVisible);
  const timer = window.setInterval(() => void check(), CHECK_EVERY_MS);

  return {
    stop: () => {
      stopped = true;
      document.removeEventListener('visibilitychange', onVisible);
      window.removeEventListener('pageshow', onPageShow);
      window.removeEventListener('focus', onVisible);
      window.clearInterval(timer);
    },
    whenIdle: reloadIfIdle,
  };
}
