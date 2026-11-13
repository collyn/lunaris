use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Playback gain (f32 bits): 0 when sound is off, else volume / 100. Set by
/// the app's Audio settings; read by the output callback.
static AUDIO_GAIN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000); // 1.0

/// Turns the stream's sound on or off and sets its volume (0-100).
pub fn set_output(enabled: bool, volume: u32) {
    let gain = if enabled { volume.min(100) as f32 / 100.0 } else { 0.0 };
    AUDIO_GAIN.store(gain.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Whether sessions ask the remote computer to record its microphone too.
static RECORD_MICROPHONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_record_microphone(on: bool) {
    RECORD_MICROPHONE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn record_microphone() -> bool {
    RECORD_MICROPHONE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn setup_audio() -> Option<mpsc::UnboundedSender<Vec<f32>>> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let config = device.default_output_config().ok()?;

    let config_channels = config.channels();
    let config_sample_rate = config.sample_rate();
    info!(
        "Initializing CPAL audio output device: {} channels, {} Hz",
        config_channels, config_sample_rate
    );

    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<f32>>();
    let target_channels = config_channels as usize;
    let mut playout = Playout::new(config_sample_rate);

    let stream = device
        .build_output_stream(
            config.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                while let Ok(samples) = rx.try_recv() {
                    playout.push(&samples);
                }
                let gain = f32::from_bits(AUDIO_GAIN.load(std::sync::atomic::Ordering::Relaxed));
                for frame in data.chunks_mut(target_channels) {
                    let (left, right) = playout.next_frame();
                    let (left, right) = (left * gain, right * gain);
                    match frame.len() {
                        1 => frame[0] = (left + right) * 0.5,
                        _ => {
                            frame[0] = left;
                            frame[1] = right;
                            for extra in &mut frame[2..] {
                                *extra = 0.0;
                            }
                        }
                    }
                }
            },
            |err| warn!("an error occurred on cpal stream: {}", err),
            None,
        )
        .ok()?;

    if stream.play().is_ok() {
        Box::leak(Box::new(stream));
        Some(tx)
    } else {
        None
    }
}

/// The stream's sound (stereo, 48 kHz, as it arrives in Opus packets) played
/// at the output device's rate. Keeps a small buffer so a late packet doesn't
/// leave a gap (a gap followed by sound is heard as a crackle), keeps the
/// resampling position between output callbacks (dropping it made a jump
/// every callback when the device isn't at 48 kHz), and drops sound when the
/// buffer grows so the delay doesn't creep up.
struct Playout {
    /// Interleaved stereo frames not played yet.
    buffer: std::collections::VecDeque<(f32, f32)>,
    /// Position between buffer[0] and buffer[1].
    phase: f64,
    /// Input frames per output frame.
    step: f64,
    /// Filling up again after running dry: play silence until `start_frames`.
    buffering: bool,
    start_frames: usize,
    max_frames: usize,
}

const STREAM_RATE: f64 = 48_000.0;

impl Playout {
    fn new(output_rate: u32) -> Self {
        Self {
            buffer: std::collections::VecDeque::new(),
            phase: 0.0,
            step: STREAM_RATE / f64::from(output_rate.max(1)),
            buffering: true,
            // 40 ms before starting (two Opus packets of jitter), and never
            // more than 200 ms behind.
            start_frames: (STREAM_RATE * 0.040) as usize,
            max_frames: (STREAM_RATE * 0.200) as usize,
        }
    }

    fn push(&mut self, interleaved: &[f32]) {
        self.buffer
            .extend(interleaved.chunks_exact(2).map(|lr| (lr[0], lr[1])));
        if self.buffer.len() > self.max_frames {
            // Behind by too much (e.g. after a stall): skip back to the
            // starting delay.
            let excess = self.buffer.len() - self.start_frames;
            self.buffer.drain(..excess);
            self.phase = 0.0;
        }
    }

    fn next_frame(&mut self) -> (f32, f32) {
        if self.buffering {
            if self.buffer.len() < self.start_frames {
                return (0.0, 0.0);
            }
            self.buffering = false;
        }
        // Two frames are needed to interpolate.
        if self.buffer.len() < 2 {
            self.buffering = true;
            return (0.0, 0.0);
        }
        let (l1, r1) = self.buffer[0];
        let (l2, r2) = self.buffer[1];
        let t = self.phase as f32;
        let out = (l1 + (l2 - l1) * t, r1 + (r2 - r1) * t);
        self.phase += self.step;
        while self.phase >= 1.0 && !self.buffer.is_empty() {
            self.buffer.pop_front();
            self.phase -= 1.0;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, start: usize) -> Vec<f32> {
        (start..start + frames)
            .flat_map(|i| {
                let v = (i as f32 * 0.01).sin();
                [v, v]
            })
            .collect()
    }

    #[test]
    fn waits_for_the_starting_delay() {
        let mut p = Playout::new(48_000);
        p.push(&tone(100, 50));
        assert_eq!(p.next_frame(), (0.0, 0.0));
        p.push(&tone(2000, 150));
        assert_ne!(p.next_frame(), (0.0, 0.0));
    }

    #[test]
    fn resampling_is_continuous_across_callbacks() {
        // 44.1 kHz output: each output step must move the same distance in
        // the input whatever the callback size, so the output is smooth.
        let mut p = Playout::new(44_100);
        p.push(&tone(9_000, 0));
        let mut out = Vec::new();
        for callback in [441usize, 512, 300, 1024, 7] {
            for _ in 0..callback {
                out.push(p.next_frame().0);
            }
        }
        let biggest_jump = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        // The tone moves at most 0.01 * 48/44.1 per output frame.
        assert!(biggest_jump < 0.0115, "jump {}", biggest_jump);
    }

    #[test]
    fn drops_sound_when_too_far_behind() {
        let mut p = Playout::new(48_000);
        p.push(&tone(48_000, 0));
        assert!(p.buffer.len() <= p.max_frames);
    }
}
