// Stream setting choices, the same as the desktop app's
// (client-qml/qml/StreamOptions.qml): keep the two in step.

export const RESOLUTIONS = [
  { value: '2160p', label: '4K', width: 3840, height: 2160 },
  { value: '1440p', label: '2K', width: 2560, height: 1440 },
  { value: '1200p', label: '1200p', width: 1920, height: 1200 },
  { value: '1080p', label: '1080p (FHD)', width: 1920, height: 1080 },
  { value: '900p', label: '900p', width: 1600, height: 900 },
  { value: '768p', label: '768p', width: 1366, height: 768 },
  { value: '720p', label: '720p (HD)', width: 1280, height: 720 },
  { value: '540p', label: '540p', width: 960, height: 540 },
] as const;

export const DEFAULT_RESOLUTION = '1080p';

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

/** The part of a decoded frame that is the picture. H.265 encoders round
 *  the size up to their block size (1920x1080 is coded as 1920x1088) and
 *  say what to crop; Chrome's D3D11 H.265 decoder on Windows (AMD) ignores
 *  that and hands out the extra lines, which are green. With the picture's
 *  size (the agent's EncoderStatus, else the size asked for) only that is
 *  shown: when the frame is at most 64 lines or columns larger (one block
 *  of padding), otherwise it is taken as it is. */
export const shownPictureSize = (
  decodedWidth: number,
  decodedHeight: number,
  picture: { width: number; height: number } | null,
): { width: number; height: number } => {
  const decoded = { width: decodedWidth, height: decodedHeight };
  if (!picture || picture.width <= 0 || picture.height <= 0) return decoded;
  const extraW = decodedWidth - picture.width;
  const extraH = decodedHeight - picture.height;
  if (extraW < 0 || extraH < 0 || extraW > 64 || extraH > 64) return decoded;
  // The colour of the last line (column) is blended with the padding's (one
  // colour sample covers two lines in 4:2:0), green too: two lines (columns)
  // more are left out where something is cropped (measured in Chrome on
  // Windows: line 1079 of 1080 stayed green with 8 lines cropped).
  return {
    width: extraW > 0 ? picture.width - PICTURE_EDGE : picture.width,
    height: extraH > 0 ? picture.height - PICTURE_EDGE : picture.height,
  };
};

/** Lines (columns) left out beside a cropped padding (see shownPictureSize). */
export const PICTURE_EDGE = 2;

/** The reference of a pointer position (the scale its x and y are on, as
 *  the host is told it). Positions are fractions of the shown picture on a
 *  4096 scale; where shownPictureSize cropped, the shown part is smaller
 *  than the host's picture and the scale is that much larger, so the host
 *  puts the pointer where it is shown. */
export const pointerReference = (
  decodedWidth: number,
  decodedHeight: number,
  picture: { width: number; height: number } | null,
): { width: number; height: number } => {
  const shown = shownPictureSize(decodedWidth, decodedHeight, picture);
  const scale = (shownSide: number, decodedSide: number, pictureSide: number) =>
    shownSide > 0 && shownSide < decodedSide ? Math.round((4096 * pictureSide) / shownSide) : 4096;
  if (!picture) return { width: 4096, height: 4096 };
  return {
    width: scale(shown.width, decodedWidth, picture.width),
    height: scale(shown.height, decodedHeight, picture.height),
  };
};

export const DEFAULT_BITRATE = 15000;

export const bitrateLabel = (kbps: number) => `${kbps / 1000} Mbps`;
export const mouseQueueLabel = (bytes: number) => (bytes >= 16384 ? `${bytes / 1024} KB` : `${bytes} B`);

export const resolutionSize = (value: string): { width: number; height: number } => {
  const preset = RESOLUTIONS.find(r => r.value === value);
  if (preset) return { width: preset.width, height: preset.height };
  const m = /^(\d+)x(\d+)$/.exec(value);
  return m ? { width: Number(m[1]), height: Number(m[2]) } : { width: 1920, height: 1080 };
};

/** "1920x1080" for a resolution value (the format deep links use). */
export const resolutionString = (value: string): string => {
  const { width, height } = resolutionSize(value);
  return `${width}x${height}`;
};

// ---- Viewer settings (this device; applied right away, not per computer) ----

export type DisplayMode = 'fit' | 'fill' | 'stretch' | 'native';

export const DISPLAY_MODES: { value: DisplayMode; label: string }[] = [
  { value: 'fit', label: 'Fit window' },
  { value: 'fill', label: 'Fill window' },
  { value: 'stretch', label: 'Stretch' },
  { value: 'native', label: 'Original size' },
];

export const readDisplayMode = (): DisplayMode => {
  const v = localStorage.getItem('lunaris_display_mode');
  return v === 'fill' || v === 'stretch' || v === 'native' ? v : 'fit';
};

/** CSS object-fit for a display mode (video and canvas elements). */
export const objectFitFor = (mode: DisplayMode): 'contain' | 'cover' | 'fill' | 'none' =>
  mode === 'fill' ? 'cover' : mode === 'stretch' ? 'fill' : mode === 'native' ? 'none' : 'contain';

export const readVolume = (): number => {
  const v = Number(localStorage.getItem('lunaris_stream_volume'));
  return Number.isFinite(v) && localStorage.getItem('lunaris_stream_volume') !== null ? Math.max(0, Math.min(100, v)) : 100;
};
