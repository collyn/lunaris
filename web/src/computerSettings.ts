// A Lunaris computer's own settings as the server answers them
// (GET/PUT /api/admin/hosts/{id}/settings, server/src/host_settings.rs).
// Pure functions (tests/computerSettings.test.ts).

/** As the agent tells them (common::AgentSettingsMsg). */
export type ComputerSettings = {
  remote_changes: boolean;
  direct_enabled: boolean;
  direct_port: number;
  has_password: boolean;
  approve_mode: string;
  server_sessions: string;
  allow_clipboard: boolean;
  allow_file_transfer: boolean;
  file_access: string;
  rustdesk_enabled: boolean;
  rustdesk_port: number;
  rustdesk_file_transfer: boolean;
  wayland_capture: string;
  input_ready: boolean;
  version: string;
  os: string;
  linked_by_address?: boolean;
};

/** The fields that can be changed (common::AgentSettingsChange). */
const CHANGEABLE = [
  'direct_enabled', 'direct_port', 'approve_mode', 'server_sessions', 'allow_clipboard', 'allow_file_transfer',
  'file_access', 'rustdesk_enabled', 'rustdesk_port', 'rustdesk_file_transfer', 'wayland_capture',
] as const;
type Changeable = typeof CHANGEABLE[number];

/** What differs between the settings loaded and the draft: the body of the PUT. */
export const settingsChanges = (loaded: ComputerSettings, draft: ComputerSettings): Partial<ComputerSettings> => {
  const out: Partial<ComputerSettings> = {};
  for (const key of CHANGEABLE) {
    if (draft[key] !== loaded[key]) (out as Record<Changeable, unknown>)[key] = draft[key];
  }
  return out;
};

/** Whether a port typed can be one (the server and the agent check it again). */
export const portProblem = (port: number): boolean => !(Number.isInteger(port) && port >= 1024 && port <= 65535);


/** The shortest access password the agent takes when it is set from here
 *  (agent/src/pairing.rs); the longest is 128. */
export const MIN_ACCESS_PASSWORD = 12;

/** An access password typed in "Set a password": 12 to 128 characters, no
 *  spaces at either end. */
export const accessPasswordOk = (password: string): boolean =>
  password.length >= MIN_ACCESS_PASSWORD && password.length <= 128 && password.trim() === password;
