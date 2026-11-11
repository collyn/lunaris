// Stream setting choices, the same as the desktop app's
// (client-qml/qml/StreamOptions.qml): keep the two in step.

export const RESOLUTIONS = [
  { value: '1080p', label: '1080p', width: 1920, height: 1080 },
  { value: '720p', label: '720p', width: 1280, height: 720 },
  { value: '540p', label: '540p', width: 960, height: 540 },
] as const;

export const FPS_OPTIONS = [240, 144, 120, 90, 75, 60, 30];

/** kbps */
export const BITRATES = [2000, 5000, 8000, 10000, 15000, 20000, 30000, 40000, 50000, 75000, 100000, 150000];

export const CODECS = [
  { value: 'auto', label: 'Auto' },
  { value: 'h264', label: 'H.264' },
  { value: 'h265', label: 'H.265' },
  { value: 'av1', label: 'AV1' },
] as const;

/** 4:4:4 keeps coloured text sharp; H.264 only (encoded by NVENC or the CPU). */
export const CHROMAS = [
  { value: '420', label: '4:2:0' },
  { value: '444', label: '4:4:4' },
] as const;

/** Bytes of mouse movement allowed to queue before old moves are dropped. */
export const MOUSE_QUEUES = [0, 64, 256, 1024, 4096, 16384];

export const DEFAULT_CODEC = 'auto';
export const DEFAULT_BITRATE = 15000;

export const bitrateLabel = (kbps: number) => `${kbps / 1000} Mbps`;
export const mouseQueueLabel = (bytes: number) => (bytes >= 16384 ? `${bytes / 1024} KB` : `${bytes} B`);

export const resolutionSize = (value: string): { width: number; height: number } => {
  const preset = RESOLUTIONS.find(r => r.value === value);
  if (preset) return { width: preset.width, height: preset.height };
  const m = /^(\d+)x(\d+)$/.exec(value);
  return m ? { width: Number(m[1]), height: Number(m[2]) } : { width: 1920, height: 1080 };
};
