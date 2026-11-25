// The game mouse of a Windows computer (agent/src/game_mouse): a USB mouse
// its agent plugs in with usbip-win2, which games with a kernel anti-cheat
// (League of Legends, Valorant) accept while they drop injected input. The
// computer's owner installs usbip-win2; its settings say whether it is there
// and how to get it. Pure (tests: tests/gameMouse.test.ts).
import type { AgentBuild } from './agentUpdate.ts';

export const USBIP_WIN2_RELEASES = 'https://github.com/vadimgrn/usbip-win2/releases/latest';

/** The "Game mouse" row of a computer's settings (English keys, translated
 *  by the caller); `null`: no row (not Windows, or nothing known). */
export interface GameMouseInfo {
  value: string;
  note: string;
  link?: { href: string; label: string };
}

export const gameMouseInfo = (agent: AgentBuild | null | undefined): GameMouseInfo | null => {
  if (!agent || agent.os !== 'windows') return null;
  switch (agent.game_mouse) {
    case 'usbip':
      return {
        value: 'Ready (usbip-win2)',
        note: 'Games with an anti-cheat (League of Legends, Valorant) take the mouse while the pointer is locked (Lock Pointer).',
      };
    case 'none':
      return {
        value: 'Not installed',
        note: 'Games with an anti-cheat (League of Legends, Valorant) ignore the remote mouse. Install usbip-win2 on that computer and restart its agent, then lock the pointer (Lock Pointer) to play.',
        link: { href: USBIP_WIN2_RELEASES, label: 'Download usbip-win2' },
      };
    default:
      // An agent before 0.7.8 does not say.
      return {
        value: 'Needs agent 0.7.8 or newer',
        note: 'Update the agent to use a mouse that games with an anti-cheat accept.',
      };
  }
};

/** Whether the viewer turned the game mouse on for this computer (kept per
 *  computer on this device; off until chosen). */
export const gameMouseKey = (hostId: string) => `lunaris_game_mouse:${hostId}`;

export const readGameMouseOn = (hostId: string): boolean => {
  try { return localStorage.getItem(gameMouseKey(hostId)) === 'true'; } catch { return false; }
};

export const saveGameMouseOn = (hostId: string, on: boolean) => {
  try { localStorage.setItem(gameMouseKey(hostId), String(on)); } catch { /* kept for this session only */ }
};

/** Mouse packet type 5 (agent/src/input.rs `GameMouse`): turns the agent's
 *  USB game mouse on or off. Agents before 0.7.8 ignore it. */
export const gameMousePacket = (on: boolean): ArrayBuffer => {
  const buffer = new ArrayBuffer(2);
  const view = new DataView(buffer);
  view.setUint8(0, 5);
  view.setUint8(1, on ? 1 : 0);
  return buffer;
};
