//! Pictures for connectors whose protocol sends pixels, not video (RDP
//! bitmaps, VNC rectangles): a whole picture kept here (BGRA), changed one
//! rectangle at a time by the protocol, and encoded with the agent's own
//! encoders (`lunaris_media::encode`, the same ones that encode this
//! computer's screen) into what [`Event::Video`] carries.
//!
//! - The codec is the first of the viewer's (`Viewer::codecs`, preferred
//!   first) that a hardware encoder of this computer opens; else H.264 in
//!   software (libx264), when the viewer takes H.264; else the first codec
//!   that opens at all. Software H.265 and AV1 are not tried before the
//!   hardware ones of every codec: a software AV1 encoder takes about 1 GB
//!   (the note on `EncoderConfig::hardware_only`).
//! - The encoder exists for one session: opened when the first picture is
//!   there, shut when the session ends (dropping [`RawVideo`]). Nothing is
//!   probed when the agent starts.
//! - A picture is encoded when something changed, at most at the viewer's
//!   frame rate; when nothing changed, the same picture again once a
//!   second (so that the viewer's decoder and the agent's statistics see a
//!   live stream); a key frame when asked (`request_key_frame`).
//! - Sizes: 4:2:0 encoders need an even width and height. An odd one is
//!   padded by one column / row (a copy of the last one); the displays told
//!   to the viewer keep the computer's own size. A new size (an RDP
//!   reactivation, a VNC DesktopSize) shuts the encoder, opens one for the
//!   new size and tells the viewer the new display (`Event::Displays`)
//!   before the first picture of that size.
//!
//! The encoding runs on a thread of its own; the protocol's task only
//! copies rectangles into the picture under a lock.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use lunaris_media::capture::gpu_buffer::GpuBuffer;
use lunaris_media::encode::{create_encoder, EncoderConfig, VideoEncoder, KEYFRAME_INTERVAL_ON_DEMAND};
use lunaris_media::types::{FrameType, HwAccelType, PixelFormat, VideoCodec};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{Codec, DisplayInfo, Event, Viewer};

/// A picture is sent at least this often, changed or not.
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// What is asked of the encoder when the viewer has no wish (kbit/s).
pub const DEFAULT_BITRATE_KBPS: u32 = 10_000;
/// Frame rate when the viewer has no wish.
pub const DEFAULT_FPS: u32 = 30;
/// A changed picture is encoded once nothing more has been drawn for this
/// long: a protocol draws one screen's change as many rectangles in a row
/// (RDP's bitmap updates come in rows of 64x64 tiles, top to bottom), and a
/// picture taken in between shows the screen half old, half new, so the
/// change seems to sweep down the screen.
pub const SETTLE: Duration = Duration::from_millis(6);
/// ... but a picture that keeps changing is still sent this long after its
/// first change at the latest (or one frame interval, if that is longer),
/// so moving pictures keep moving.
pub const SETTLE_MAX: Duration = Duration::from_millis(40);

/// Which encoders may be tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderChoice {
    /// Hardware first, software H.264 after (the rule above).
    Auto,
    /// Software only (tests: the same result on every computer).
    Software,
}

/// The whole picture as the protocol last drew it.
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// BGRA (B, G, R, X), `width * 4` bytes a row.
    pub data: Vec<u8>,
    dirty: bool,
    /// When it was last drawn on, and when the first change not sent yet
    /// was made (`None`: no change since the last picture sent).
    changed_at: Option<Instant>,
    dirty_since: Option<Instant>,
}

impl Framebuffer {
    fn new(width: u32, height: u32) -> Framebuffer {
        Framebuffer { width, height, data: vec![0; width as usize * height as usize * 4], dirty: true, changed_at: None, dirty_since: None }
    }

    #[cfg(test)]
    pub fn new_for_tests(width: u32, height: u32) -> Framebuffer {
        Framebuffer::new(width, height)
    }

    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    /// Copies `pixels` (BGRA, `w * 4` bytes a row) to (x, y); what falls
    /// outside the picture is left out.
    pub fn put_bgra(&mut self, x: u32, y: u32, w: u32, h: u32, pixels: &[u8]) {
        let (fw, fh) = (self.width, self.height);
        if x >= fw || y >= fh {
            return;
        }
        let cw = w.min(fw - x) as usize;
        let ch = h.min(fh - y) as usize;
        let src_stride = w as usize * 4;
        let stride = self.stride();
        for row in 0..ch {
            let src = row * src_stride;
            if src + cw * 4 > pixels.len() {
                break;
            }
            let dst = (y as usize + row) * stride + x as usize * 4;
            self.data[dst..dst + cw * 4].copy_from_slice(&pixels[src..src + cw * 4]);
        }
        self.touch();
    }

    /// Fills a rectangle with one BGRA pixel.
    pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]) {
        let (fw, fh) = (self.width, self.height);
        if x >= fw || y >= fh {
            return;
        }
        let cw = w.min(fw - x) as usize;
        let ch = h.min(fh - y) as usize;
        let stride = self.stride();
        for row in 0..ch {
            let start = (y as usize + row) * stride + x as usize * 4;
            for px in self.data[start..start + cw * 4].chunks_exact_mut(4) {
                px.copy_from_slice(&bgra);
            }
        }
        self.touch();
    }

    /// Copies a rectangle from (sx, sy) to (dx, dy) within the picture
    /// (overlapping is allowed: the source is read whole first).
    pub fn copy_rect(&mut self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        let (fw, fh) = (self.width, self.height);
        if sx >= fw || sy >= fh || dx >= fw || dy >= fh {
            return;
        }
        let w = w.min(fw - sx).min(fw - dx) as usize;
        let h = h.min(fh - sy).min(fh - dy) as usize;
        let stride = self.stride();
        let mut tmp = vec![0u8; w * 4 * h];
        for row in 0..h {
            let src = (sy as usize + row) * stride + sx as usize * 4;
            tmp[row * w * 4..(row + 1) * w * 4].copy_from_slice(&self.data[src..src + w * 4]);
        }
        for row in 0..h {
            let dst = (dy as usize + row) * stride + dx as usize * 4;
            self.data[dst..dst + w * 4].copy_from_slice(&tmp[row * w * 4..(row + 1) * w * 4]);
        }
        self.touch();
    }

    /// The protocol drew the whole picture another way (its own decoder's
    /// buffer was copied in): mark it changed.
    pub fn mark_dirty(&mut self) {
        self.touch();
    }

    fn touch(&mut self) {
        let now = Instant::now();
        self.dirty = true;
        self.changed_at = Some(now);
        self.dirty_since.get_or_insert(now);
    }

    /// How much longer to wait before the change is encoded (zero: now).
    /// See [`SETTLE`].
    fn settle_wait(&self, now: Instant, interval: Duration) -> Duration {
        let (Some(changed), Some(since)) = (self.changed_at, self.dirty_since) else { return Duration::ZERO };
        let quiet = (changed + SETTLE).saturating_duration_since(now);
        let latest = (since + SETTLE_MAX.max(interval)).saturating_duration_since(now);
        quiet.min(latest)
    }

    /// The picture was taken for encoding.
    fn taken(&mut self) {
        self.dirty = false;
        self.dirty_since = None;
    }
}

struct Shared {
    fb: Mutex<Option<Framebuffer>>,
    wake: Condvar,
    key_frame: AtomicBool,
    stop: AtomicBool,
    fps: AtomicU32,
    bitrate_kbps: AtomicU32,
}

/// The encoding side of one session. Dropping it stops the thread and shuts
/// the encoder.
pub struct RawVideo {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What a session measured of its encoding (for the log and the tests).
#[derive(Clone, Debug, Default)]
pub struct EncodeStats {
    pub encoder: String,
    pub frames: u64,
    pub encode_us_total: u64,
}

impl RawVideo {
    /// Starts the encoding thread for `viewer`; pictures go to `events`.
    /// `label` names the connector in the log.
    pub fn start(viewer: &Viewer, events: mpsc::Sender<Event>, label: &'static str, choice: EncoderChoice) -> RawVideo {
        let shared = Arc::new(Shared {
            fb: Mutex::new(None),
            wake: Condvar::new(),
            key_frame: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            fps: AtomicU32::new(if viewer.fps == 0 { DEFAULT_FPS } else { viewer.fps }),
            bitrate_kbps: AtomicU32::new(if viewer.bitrate_kbps == 0 { DEFAULT_BITRATE_KBPS } else { viewer.bitrate_kbps }),
        });
        let codecs = viewer.codecs.clone();
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name(format!("{label}-encode"))
            .spawn(move || encode_loop(thread_shared, codecs, events, label, choice))
            .ok();
        RawVideo { shared, thread }
    }

    /// Sets the picture's size (the first time, or a new one): the picture
    /// is black until drawn.
    pub fn resize(&self, width: u32, height: u32) {
        let mut fb = self.shared.fb.lock().unwrap();
        match fb.as_ref() {
            Some(old) if old.width == width && old.height == height => {}
            _ => *fb = Some(Framebuffer::new(width, height)),
        }
        drop(fb);
        self.shared.wake.notify_one();
    }

    /// Changes the picture (nothing happens before the first `resize`).
    pub fn update<R>(&self, change: impl FnOnce(&mut Framebuffer) -> R) -> Option<R> {
        let mut fb = self.shared.fb.lock().unwrap();
        let result = fb.as_mut().map(change);
        drop(fb);
        self.shared.wake.notify_one();
        result
    }

    pub fn request_key_frame(&self) {
        self.shared.key_frame.store(true, Ordering::Relaxed);
        self.shared.wake.notify_one();
    }

    /// The viewer's wish; 0 = unchanged.
    pub fn set_quality(&self, fps: u32, bitrate_kbps: u32) {
        if fps != 0 {
            self.shared.fps.store(fps, Ordering::Relaxed);
        }
        if bitrate_kbps != 0 {
            self.shared.bitrate_kbps.store(bitrate_kbps, Ordering::Relaxed);
        }
        self.shared.wake.notify_one();
    }

    /// The frame rate asked for now.
    pub fn fps(&self) -> u32 {
        self.shared.fps.load(Ordering::Relaxed).max(1)
    }
}

impl Drop for RawVideo {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn media_codec(codec: Codec) -> Option<VideoCodec> {
    match codec {
        Codec::H264 => Some(VideoCodec::H264),
        Codec::H265 => Some(VideoCodec::H265),
        Codec::Av1 => Some(VideoCodec::AV1),
        Codec::Vp8 | Codec::Vp9 => None,
    }
}

fn even(value: u32) -> u32 {
    (value + 1) & !1
}

/// An open encoder and what it makes.
pub struct OpenEncoder {
    pub encoder: Box<dyn VideoEncoder>,
    pub codec: Codec,
    pub name: String,
    pub hardware: bool,
}

/// Opens an encoder for a `width` x `height` picture (already even) by the
/// rule in the module's notes.
pub fn open_encoder(
    codecs: &[Codec],
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
    choice: EncoderChoice,
) -> Result<OpenEncoder, String> {
    let wanted: Vec<(Codec, VideoCodec)> = codecs.iter().filter_map(|c| media_codec(*c).map(|m| (*c, m))).collect();
    if wanted.is_empty() {
        return Err("This viewer decodes no video this computer can encode (H.264, H.265 or AV1)".into());
    }
    let config = |codec: VideoCodec, hardware_only: bool, preferred_hw: Option<HwAccelType>| EncoderConfig {
        codec,
        width,
        height,
        fps,
        bitrate_kbps,
        low_latency: true,
        keyframe_interval: KEYFRAME_INTERVAL_ON_DEMAND,
        hardware_only,
        preferred_hw,
        ..EncoderConfig::default()
    };
    let try_open = |cfg: EncoderConfig, codec: Codec| -> Option<OpenEncoder> {
        let mut encoder = create_encoder().ok()?;
        match encoder.initialize(&cfg) {
            Ok(()) => {
                let info = encoder.encoder_info();
                Some(OpenEncoder { encoder, codec, name: info.name, hardware: info.hw_type != HwAccelType::Software })
            }
            Err(e) => {
                info!("Raw video: no {:?} encoder ({}hardware only): {}", codec, if cfg.hardware_only { "" } else { "not " }, e);
                None
            }
        }
    };
    if choice == EncoderChoice::Auto {
        for (codec, media) in &wanted {
            if let Some(open) = try_open(config(*media, true, None), *codec) {
                return Ok(open);
            }
        }
    }
    if let Some((codec, media)) = wanted.iter().find(|(c, _)| *c == Codec::H264) {
        if let Some(open) = try_open(config(*media, false, Some(HwAccelType::Software)), *codec) {
            return Ok(open);
        }
    }
    if choice == EncoderChoice::Auto {
        for (codec, media) in &wanted {
            if let Some(open) = try_open(config(*media, false, None), *codec) {
                return Ok(open);
            }
        }
    }
    Err("No video encoder of this computer opens for this viewer".into())
}

/// Copies `fb` into `out` (BGRA, `out_w` x `out_h`, both even and at least
/// the picture's size), repeating the last column / row into the padding.
pub fn copy_padded(fb: &Framebuffer, out: &mut Vec<u8>, out_w: u32, out_h: u32) {
    let (w, h) = (fb.width as usize, fb.height as usize);
    let (ow, oh) = (out_w as usize, out_h as usize);
    out.resize(ow * oh * 4, 0);
    if w == 0 || h == 0 {
        return;
    }
    for row in 0..oh {
        let src_row = row.min(h - 1);
        let src = &fb.data[src_row * w * 4..(src_row + 1) * w * 4];
        let dst = &mut out[row * ow * 4..(row + 1) * ow * 4];
        dst[..w * 4].copy_from_slice(src);
        for col in w..ow {
            let last = (w - 1) * 4;
            dst[col * 4..col * 4 + 4].copy_from_slice(&src[last..last + 4]);
        }
    }
}

fn release_memory() {
    // The allocator keeps freed encoder buffers otherwise (the note on
    // `EncoderConfig::hardware_only`); glibc gives them back with this.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

fn encode_loop(shared: Arc<Shared>, codecs: Vec<Codec>, events: mpsc::Sender<Event>, label: &'static str, choice: EncoderChoice) {
    let start = Instant::now();
    let mut open: Option<(OpenEncoder, u32, u32, u32, u32)> = None; // encoder, picture w, h, encoded w, h
    let mut buffer = GpuBuffer::CpuBuffer { data: Vec::new(), stride: 0, format: PixelFormat::BGRA, width: 0, height: 0 };
    let mut last_sent: Option<Instant> = None;
    let mut fps_now = 0u32;
    let mut bitrate_now = 0u32;
    let mut stats = EncodeStats::default();
    let mut report_at = Instant::now() + Duration::from_secs(10);
    let (mut window_frames, mut window_us) = (0u64, 0u64);
    'outer: loop {
        // Wait for something to do.
        let mut fb_guard = shared.fb.lock().unwrap();
        loop {
            if shared.stop.load(Ordering::Relaxed) {
                break 'outer;
            }
            let fps = shared.fps.load(Ordering::Relaxed).max(1);
            let interval = Duration::from_secs_f64(1.0 / fps as f64);
            let now = Instant::now();
            let due = last_sent.map(|t| now.duration_since(t) >= interval).unwrap_or(true);
            let heartbeat = last_sent.map(|t| now.duration_since(t) >= HEARTBEAT).unwrap_or(false);
            match fb_guard.as_ref() {
                Some(fb) if (fb.dirty && due && fb.settle_wait(now, interval).is_zero()) || heartbeat || (shared.key_frame.load(Ordering::Relaxed) && due) => break,
                _ => {}
            }
            // Sleep until the next frame is allowed or the heartbeat is due.
            let wait = match (fb_guard.as_ref(), last_sent) {
                (Some(fb), Some(t)) if fb.dirty => (t + interval).saturating_duration_since(now).max(fb.settle_wait(now, interval)),
                (Some(fb), None) if fb.dirty => fb.settle_wait(now, interval),
                (_, Some(t)) => (t + HEARTBEAT).saturating_duration_since(now),
                _ => HEARTBEAT,
            };
            fb_guard = shared.wake.wait_timeout(fb_guard, wait.max(Duration::from_millis(1))).unwrap().0;
        }
        let Some(fb) = fb_guard.as_mut() else { continue };
        let (w, h) = (fb.width, fb.height);
        let (ew, eh) = (even(w), even(h));
        let fps = shared.fps.load(Ordering::Relaxed).max(1);
        let bitrate = shared.bitrate_kbps.load(Ordering::Relaxed).max(100);

        // A new size: a new encoder, and the viewer hears of the display first.
        if open.as_ref().is_none_or(|(_, ow, oh, _, _)| (*ow, *oh) != (w, h)) {
            if let Some((mut old, ..)) = open.take() {
                old.encoder.shutdown();
                drop(old);
                release_memory();
            }
            if w == 0 || h == 0 {
                drop(fb_guard);
                continue;
            }
            drop(fb_guard);
            let opened = open_encoder(&codecs, ew, eh, fps, bitrate, choice);
            match opened {
                Ok(enc) => {
                    info!(
                        "{label}: encoding {}x{} (as {}x{}) with {} ({:?}, {}), {} fps, {} kbit/s",
                        w,
                        h,
                        ew,
                        eh,
                        enc.name,
                        enc.codec,
                        if enc.hardware { "hardware" } else { "software" },
                        fps,
                        bitrate
                    );
                    stats.encoder = enc.name.clone();
                    let displays = vec![DisplayInfo { name: "Display".into(), width: w, height: h }];
                    if events.blocking_send(Event::Displays { displays, current: 0 }).is_err() {
                        break 'outer;
                    }
                    open = Some((enc, w, h, ew, eh));
                    fps_now = fps;
                    bitrate_now = bitrate;
                    shared.key_frame.store(true, Ordering::Relaxed);
                }
                Err(reason) => {
                    warn!("{label}: {reason}");
                    let _ = events.blocking_send(Event::Closed { reason });
                    break 'outer;
                }
            }
            continue;
        }
        let Some((enc, _, _, ew, eh)) = open.as_mut() else { continue };
        if let GpuBuffer::CpuBuffer { data, stride, width, height, .. } = &mut buffer {
            copy_padded(fb, data, *ew, *eh);
            *stride = *ew * 4;
            *width = *ew;
            *height = *eh;
        }
        fb.taken();
        drop(fb_guard);

        if fps != fps_now {
            let _ = enc.encoder.set_fps(fps);
            fps_now = fps;
        }
        if bitrate != bitrate_now {
            let _ = enc.encoder.set_bitrate(bitrate);
            bitrate_now = bitrate;
        }
        if shared.key_frame.swap(false, Ordering::Relaxed) {
            enc.encoder.request_keyframe();
        }
        let now = Instant::now();
        let pts_us = now.duration_since(start).as_micros() as u64;
        let began = Instant::now();
        let frames = match enc.encoder.encode(&buffer, pts_us) {
            Ok(frames) => frames,
            Err(e) => {
                let reason = format!("The picture could not be encoded ({e})");
                warn!("{label}: {reason}");
                let _ = events.blocking_send(Event::Closed { reason });
                break 'outer;
            }
        };
        let took = began.elapsed().as_micros() as u64;
        stats.frames += 1;
        stats.encode_us_total += took;
        window_frames += 1;
        window_us += took;
        last_sent = Some(now);
        for frame in frames {
            let event = Event::Video {
                codec: enc.codec,
                key: frame.frame_type == FrameType::Key,
                capture_time_ms: Some(pts_us / 1000),
                data: Bytes::from(frame.data),
            };
            if events.blocking_send(event).is_err() {
                break 'outer;
            }
        }
        if Instant::now() >= report_at && window_frames > 0 {
            info!("{label}: {} frames in 10 s, {:.1} ms to encode one", window_frames, window_us as f64 / window_frames as f64 / 1000.0);
            (window_frames, window_us) = (0, 0);
            report_at = Instant::now() + Duration::from_secs(10);
        }
    }
    if let Some((mut enc, ..)) = open.take() {
        enc.encoder.shutdown();
        drop(enc);
        release_memory();
    }
    if stats.frames > 0 {
        info!(
            "{label}: encoder {} closed after {} frames ({:.1} ms each on average)",
            stats.encoder,
            stats.frames,
            stats.encode_us_total as f64 / stats.frames as f64 / 1000.0
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change is encoded once drawing has paused for SETTLE, or at the
    /// latest SETTLE_MAX (or a frame interval) after the change began.
    #[test]
    fn a_change_is_encoded_once_drawing_pauses() {
        let interval = Duration::from_millis(16);
        let mut fb = Framebuffer::new(4, 4);
        let t0 = Instant::now();
        // Nothing drawn since the last picture: no wait.
        fb.taken();
        assert_eq!(fb.settle_wait(t0, interval), Duration::ZERO);
        // Drawn just now: wait for the pause.
        fb.dirty = true;
        fb.dirty_since = Some(t0);
        fb.changed_at = Some(t0);
        assert_eq!(fb.settle_wait(t0, interval), SETTLE);
        assert_eq!(fb.settle_wait(t0 + SETTLE, interval), Duration::ZERO);
        // Still being drawn on (a row of tiles every 2 ms): no picture yet...
        fb.changed_at = Some(t0 + Duration::from_millis(30));
        assert!(!fb.settle_wait(t0 + Duration::from_millis(31), interval).is_zero());
        // ... until SETTLE_MAX after the first change, whatever comes.
        fb.changed_at = Some(t0 + SETTLE_MAX);
        assert_eq!(fb.settle_wait(t0 + SETTLE_MAX, interval), Duration::ZERO);
        // A slow frame rate waits up to its interval instead.
        let slow = Duration::from_millis(100);
        assert!(!fb.settle_wait(t0 + SETTLE_MAX, slow).is_zero());
        // Taken: the next change starts again.
        fb.taken();
        assert!(!fb.dirty && fb.dirty_since.is_none());
        fb.fill(0, 0, 1, 1, [1, 2, 3, 0]);
        assert!(fb.dirty && fb.dirty_since.is_some() && fb.changed_at.is_some());
    }

    #[test]
    fn rectangles_are_clipped_to_the_picture() {
        let mut fb = Framebuffer::new(4, 3);
        fb.dirty = false;
        fb.put_bgra(3, 2, 2, 2, &[1; 16]);
        assert!(fb.dirty);
        assert_eq!(&fb.data[(2 * 4 + 3) * 4..(2 * 4 + 3) * 4 + 4], &[1, 1, 1, 1]);
        assert_eq!(fb.data.iter().filter(|b| **b == 1).count(), 4);
        fb.fill(0, 0, 10, 1, [9, 8, 7, 0]);
        assert_eq!(&fb.data[12..16], &[9, 8, 7, 0]);
        fb.copy_rect(0, 0, 1, 1, 2, 1);
        assert_eq!(&fb.data[(4 + 1) * 4..(4 + 1) * 4 + 4], &[9, 8, 7, 0]);
    }

    #[test]
    fn odd_sizes_are_padded_with_the_last_pixels() {
        let mut fb = Framebuffer::new(3, 1);
        fb.put_bgra(0, 0, 3, 1, &[1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3]);
        let mut out = Vec::new();
        copy_padded(&fb, &mut out, even(3), even(1));
        assert_eq!(out.len(), 4 * 2 * 4);
        assert_eq!(&out[12..16], &[3, 3, 3, 3]);
        assert_eq!(&out[16..32], &out[0..16]);
    }

    #[test]
    fn only_the_codecs_the_agent_encodes() {
        assert_eq!(media_codec(Codec::Vp8), None);
        assert_eq!(media_codec(Codec::H264), Some(VideoCodec::H264));
        assert!(open_encoder(&[Codec::Vp9], 64, 64, 30, 1000, EncoderChoice::Software).is_err());
    }

    /// Measurement, not a check (run by hand): `LUNARIS_RAW_VIDEO_BENCH=nvenc|vaapi|software
    /// cargo test -p agent --lib raw_video::tests::bench -- --ignored --nocapture`.
    /// 1920x1080 H.264, 150 frames whose whole picture changes, paced at
    /// 30 fps: the time to copy and encode one, and the process's CPU time
    /// over the wall time.
    #[test]
    #[ignore]
    fn bench() {
        let kind = std::env::var("LUNARIS_RAW_VIDEO_BENCH").unwrap_or_else(|_| "software".into());
        let preferred = match kind.as_str() {
            "nvenc" => HwAccelType::Nvenc,
            "vaapi" => HwAccelType::Vaapi,
            _ => HwAccelType::Software,
        };
        let (w, h) = (1920u32, 1080u32);
        let config = EncoderConfig {
            codec: VideoCodec::H264,
            width: w,
            height: h,
            fps: 30,
            bitrate_kbps: 10_000,
            low_latency: true,
            keyframe_interval: KEYFRAME_INTERVAL_ON_DEMAND,
            preferred_hw: Some(preferred),
            hardware_only: preferred != HwAccelType::Software,
            ..EncoderConfig::default()
        };
        let mut encoder = create_encoder().unwrap();
        encoder.initialize(&config).unwrap();
        let info = encoder.encoder_info();
        let mut fb = Framebuffer::new(w, h);
        let mut buffer = GpuBuffer::CpuBuffer { data: Vec::new(), stride: w * 4, format: PixelFormat::BGRA, width: w, height: h };
        let cpu = || {
            let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
            unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
            usage.ru_utime.tv_sec as f64 + usage.ru_utime.tv_usec as f64 / 1e6 + usage.ru_stime.tv_sec as f64 + usage.ru_stime.tv_usec as f64 / 1e6
        };
        // Eight pictures of a moving gradient (every pixel changes from
        // one to the next), drawn before the clock starts.
        let pictures: Vec<Vec<u8>> = (0..8u32)
            .map(|i| {
                let mut p = vec![0u8; (w * h * 4) as usize];
                for (n, px) in p.chunks_exact_mut(4).enumerate() {
                    let (x, y) = ((n as u32 % w) + i * 8, n as u32 / w);
                    px.copy_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 0]);
                }
                p
            })
            .collect();
        let frames = 150u32;
        let (cpu0, wall0) = (cpu(), Instant::now());
        let mut encode_total = Duration::ZERO;
        let mut bytes = 0usize;
        for i in 0..frames {
            let due = wall0 + Duration::from_secs_f64(i as f64 / 30.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            fb.data.copy_from_slice(&pictures[i as usize % pictures.len()]);
            let began = Instant::now();
            if let GpuBuffer::CpuBuffer { data, .. } = &mut buffer {
                copy_padded(&fb, data, w, h);
            }
            for f in encoder.encode(&buffer, i as u64 * 33_333).unwrap() {
                bytes += f.data.len();
            }
            encode_total += began.elapsed();
        }
        let wall = wall0.elapsed().as_secs_f64();
        let cpu_used = cpu() - cpu0;
        println!(
            "BENCH {} ({:?}): {:.2} ms per 1080p frame (copy + encode), process CPU {:.0}% of one core at 30 fps over {:.1} s (includes one 8 MB picture copy per frame), {:.2} Mbit/s",
            info.name,
            info.hw_type,
            encode_total.as_secs_f64() * 1000.0 / frames as f64,
            cpu_used / wall * 100.0,
            wall,
            bytes as f64 * 8.0 / wall / 1e6
        );
        encoder.shutdown();
    }

    /// The whole path in software: a picture, a change, a key frame on
    /// request, the heartbeat, a new size.
    #[test]
    fn pictures_become_frames() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let viewer = Viewer { codecs: vec![Codec::H264], fps: 30, width: 0, height: 0, bitrate_kbps: 2000 };
        let video = RawVideo::start(&viewer, tx, "test", EncoderChoice::Software);
        video.resize(321, 241);
        video.update(|fb| fb.fill(0, 0, 100, 100, [255, 0, 0, 0]));
        let next = |rx: &mut mpsc::Receiver<Event>| rt.block_on(async { tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap() });
        assert_eq!(next(&mut rx), Event::Displays { displays: vec![DisplayInfo { name: "Display".into(), width: 321, height: 241 }], current: 0 });
        match next(&mut rx) {
            Event::Video { codec: Codec::H264, key: true, data, .. } => assert!(data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1])),
            other => panic!("{other:?}"),
        }
        // Nothing changes: the heartbeat brings one within about a second.
        let began = Instant::now();
        assert!(matches!(next(&mut rx), Event::Video { key: false, .. }));
        assert!(began.elapsed() >= Duration::from_millis(700), "{:?}", began.elapsed());
        video.request_key_frame();
        assert!(matches!(next(&mut rx), Event::Video { key: true, .. }));
        video.resize(200, 100);
        assert!(matches!(next(&mut rx), Event::Displays { displays, .. } if displays[0].width == 200 && displays[0].height == 100));
        assert!(matches!(next(&mut rx), Event::Video { key: true, .. }));
        drop(video);
    }
}
