//! One capture and one encoder per screen, shared by every viewer of it.
//!
//! As a RustDesk host does (rustdesk/src/server/video_service.rs: one video
//! service per screen, its frames sent to every connection subscribed to
//! it; rustdesk/src/server/video_qos.rs: the quality is the lowest any of
//! them asks for), several viewers of one screen see the same encoded
//! stream. A viewer gets the pipeline's events (`MediaEvent`) on a channel
//! of its own and sends its commands (`PipelineCommand`) on one of its own;
//! here they are put together:
//!
//! - key frame: asked of the encoder (every viewer gets it);
//! - bitrate, frame rate, frame limit: the lowest any viewer asks for, so
//!   the viewer with the slowest link sets the pace;
//! - stop: that viewer gets nothing more; the pipeline stops when no viewer
//!   is left (or every one stopped).
//!
//! A viewer joins a screen's running pipeline when it can decode what that
//! pipeline encodes; it then gets that pipeline's size, frame rate and codec,
//! not the ones it asked for. Otherwise it gets a pipeline of its own.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use lunaris_media::pipeline::{MediaEvent, MediaPipeline, PipelineCommand};
use lunaris_media::types::StreamConfig;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Events waiting for one viewer; beyond them its video frames are dropped
/// (and a key frame asked for, which every viewer then gets).
const VIEWER_EVENTS: usize = 64;
/// Commands waiting from one viewer.
const VIEWER_COMMANDS: usize = 16;
/// At most one key frame asked for a viewer whose frames were dropped, per
/// this interval (as `KeyframeRequester` in bridge.rs).
const DROP_KEYFRAME_INTERVAL: Duration = Duration::from_millis(500);
/// How long a stopped pipeline may take to end before its task is aborted.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// The pipelines viewers can join, by screen. A viewer that cannot join
/// the screen's pipeline has one that is not listed here.
/// Lock order: this map, then a pipeline.
static SHARED: LazyLock<Mutex<HashMap<String, Listed>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A listed pipeline and its id (compared without locking it).
struct Listed {
    id: u64,
    pipeline: Arc<Mutex<Pipeline>>,
}

/// What one viewer of a pipeline asked for.
struct Viewer {
    events: mpsc::Sender<MediaEvent>,
    bitrate_kbps: Option<u32>,
    fps: Option<u32>,
    /// 0: no limit.
    frame_limit: Option<u32>,
    /// Its connection ended: nothing more is sent to it.
    stopped: bool,
    last_drop_keyframe: Option<Instant>,
}

/// What the encoder was last told, so a change is sent once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Settings {
    bitrate_kbps: u32,
    fps: u32,
    frame_limit: u32,
}

struct Pipeline {
    id: u64,
    screen: String,
    config: StreamConfig,
    commands: mpsc::Sender<PipelineCommand>,
    viewers: HashMap<u64, Viewer>,
    applied: Settings,
    /// Replayed to a viewer that joins later: which encoder runs, and the
    /// pointer (its image is only sent when it changes).
    encoder_started: Option<MediaEvent>,
    cursor_image: Option<MediaEvent>,
    cursor: Option<MediaEvent>,
    run: Option<tokio::task::JoinHandle<()>>,
    stopping: bool,
}

/// Leaves the pipeline when dropped (the viewer's session ended).
pub struct Membership {
    pipeline: Arc<Mutex<Pipeline>>,
    viewer: u64,
}

impl Drop for Membership {
    fn drop(&mut self) {
        let mut p = self.pipeline.lock().unwrap();
        p.viewers.remove(&self.viewer);
        info!("Viewer {} left the stream of screen {} ({} left)", self.viewer, p.screen, p.viewers.len());
        settle(&mut p);
    }
}

/// A viewer's link to a pipeline.
pub struct Joined {
    pub events: mpsc::Receiver<MediaEvent>,
    pub commands: mpsc::Sender<PipelineCommand>,
    /// What the pipeline encodes: the viewer's own request when it started
    /// it, the first viewer's when it joined.
    pub config: StreamConfig,
    /// It joined a pipeline that was already running.
    pub joined_running: bool,
    pub membership: Membership,
}

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Joins screen `screen`'s running pipeline when `can_join` accepts what it
/// encodes, else starts one with `wanted` (shared when the screen has none).
pub fn join(screen: &str, wanted: StreamConfig, can_join: impl Fn(&StreamConfig) -> bool) -> Joined {
    let mut shared = SHARED.lock().unwrap();
    if let Some(existing) = shared.get(screen).map(|listed| listed.pipeline.clone()) {
        let mut p = existing.lock().unwrap();
        if !p.stopping && can_join(&p.config) {
            let config = p.config.clone();
            let (events, commands, viewer) = add_viewer(&mut p, &existing);
            info!(
                "Viewer {} joined the running stream of screen {} ({}x{} {} fps {:?}; {} viewers)",
                viewer,
                screen,
                config.width,
                config.height,
                config.fps,
                config.codec,
                p.viewers.len()
            );
            drop(p);
            return Joined { events, commands, config, joined_running: true, membership: Membership { pipeline: existing, viewer } };
        }
        if !p.stopping {
            info!(
                "Display {} is streamed as {:?}{}, which this viewer cannot take: it gets a capture and encoder of its own",
                screen,
                p.config.codec,
                if p.config.yuv444 { " 4:4:4" } else { "" }
            );
        }
    }
    let (id, pipeline) = start(screen, wanted.clone());
    // Listed for later viewers unless the screen's listed one still runs.
    let listed_running = shared.get(screen).is_some_and(|listed| !listed.pipeline.lock().unwrap().stopping);
    if !listed_running {
        shared.insert(screen.to_string(), Listed { id, pipeline: pipeline.clone() });
    }
    drop(shared);
    let mut p = pipeline.lock().unwrap();
    let (events, commands, viewer) = add_viewer(&mut p, &pipeline);
    drop(p);
    Joined { events, commands, config: wanted, joined_running: false, membership: Membership { pipeline, viewer } }
}

fn add_viewer(p: &mut Pipeline, pipeline: &Arc<Mutex<Pipeline>>) -> (mpsc::Receiver<MediaEvent>, mpsc::Sender<PipelineCommand>, u64) {
    let viewer = next_id();
    let (events_tx, events_rx) = mpsc::channel(VIEWER_EVENTS);
    for event in [&p.encoder_started, &p.cursor_image, &p.cursor].into_iter().flatten() {
        let _ = events_tx.try_send(event.clone());
    }
    p.viewers.insert(
        viewer,
        Viewer { events: events_tx, bitrate_kbps: None, fps: None, frame_limit: None, stopped: false, last_drop_keyframe: None },
    );
    let (commands_tx, mut commands_rx) = mpsc::channel::<PipelineCommand>(VIEWER_COMMANDS);
    let weak = Arc::downgrade(pipeline);
    tokio::spawn(async move {
        while let Some(command) = commands_rx.recv().await {
            let Some(pipeline) = weak.upgrade() else { break };
            let mut p = pipeline.lock().unwrap();
            command_of(&mut p, viewer, command);
        }
    });
    (events_rx, commands_tx, viewer)
}

/// Starts a pipeline for `screen`, its events handed out to its viewers.
fn start(screen: &str, config: StreamConfig) -> (u64, Arc<Mutex<Pipeline>>) {
    let (media, mut events, commands) = MediaPipeline::new(config.clone());
    let id = next_id();
    let display_for_run = screen.to_string();
    let run = tokio::spawn(async move {
        info!("Running lunaris-media pipeline...");
        if let Err(e) = media.run(&display_for_run).await {
            tracing::error!("Media pipeline exited with error: {:?}", e);
        }
    });
    let pipeline = Arc::new(Mutex::new(Pipeline {
        id,
        screen: screen.to_string(),
        applied: Settings { bitrate_kbps: config.bitrate_kbps, fps: config.fps, frame_limit: 0 },
        config,
        commands,
        viewers: HashMap::new(),
        encoder_started: None,
        cursor_image: None,
        cursor: None,
        run: Some(run),
        stopping: false,
    }));
    let for_events = pipeline.clone();
    let screen = screen.to_string();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let mut p = for_events.lock().unwrap();
            hand_out(&mut p, event);
        }
        // The pipeline ended: its viewers' event channels close with it.
        let mut shared = SHARED.lock().unwrap();
        if shared.get(&screen).is_some_and(|listed| listed.id == id) {
            shared.remove(&screen);
        }
        drop(shared);
        let mut p = for_events.lock().unwrap();
        p.stopping = true;
        p.viewers.clear();
        debug!("Pipeline {} of screen {} ended", id, screen);
    });
    (id, pipeline)
}

/// Sends an event of the pipeline to its viewers.
fn hand_out(p: &mut Pipeline, event: MediaEvent) {
    match &event {
        MediaEvent::EncoderStarted { .. } => p.encoder_started = Some(event.clone()),
        MediaEvent::CursorUpdate(cursor) => {
            if cursor.image.is_some() {
                p.cursor_image = Some(event.clone());
            }
            p.cursor = Some(event.clone());
        }
        _ => {}
    }
    let is_video = matches!(event, MediaEvent::VideoFrame(_));
    let now = Instant::now();
    let mut keyframe = false;
    for (id, viewer) in p.viewers.iter_mut().filter(|(_, v)| !v.stopped) {
        match viewer.events.try_send(event.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) if is_video => {
                // The next frames reference the one it missed.
                if viewer.last_drop_keyframe.is_none_or(|last| now.duration_since(last) >= DROP_KEYFRAME_INTERVAL) {
                    debug!("Viewer {} is behind: a frame was dropped for it, asking for a key frame", id);
                    viewer.last_drop_keyframe = Some(now);
                    keyframe = true;
                }
            }
            Err(_) => {}
        }
    }
    if keyframe {
        let _ = p.commands.try_send(PipelineCommand::RequestKeyframe);
    }
}

fn command_of(p: &mut Pipeline, viewer: u64, command: PipelineCommand) {
    if p.stopping {
        return;
    }
    let Some(v) = p.viewers.get_mut(&viewer) else { return };
    match command {
        PipelineCommand::RequestKeyframe => {
            if !v.stopped {
                let _ = p.commands.try_send(PipelineCommand::RequestKeyframe);
            }
            return;
        }
        PipelineCommand::SetBitrate(kbps) => v.bitrate_kbps = Some(kbps),
        PipelineCommand::SetFps(fps) => v.fps = Some(fps),
        PipelineCommand::SetFrameLimit(limit) => v.frame_limit = Some(limit),
        PipelineCommand::Stop => {
            if !v.stopped {
                info!("Viewer {} of screen {} stopped", viewer, p.screen);
            }
            v.stopped = true;
        }
    }
    settle(p);
}

/// What the viewers still watching ask for together: the lowest of each.
fn settings(config: &StreamConfig, viewers: &HashMap<u64, Viewer>) -> Settings {
    let watching = || viewers.values().filter(|v| !v.stopped);
    Settings {
        bitrate_kbps: watching().filter_map(|v| v.bitrate_kbps).min().unwrap_or(config.bitrate_kbps),
        fps: watching().filter_map(|v| v.fps).min().unwrap_or(config.fps),
        frame_limit: watching().filter_map(|v| v.frame_limit).filter(|l| *l > 0).min().unwrap_or(0),
    }
}

/// After a viewer came, went or asked for something: stops the pipeline when
/// nobody watches, else tells the encoder what changed.
fn settle(p: &mut Pipeline) {
    if p.stopping {
        return;
    }
    if p.viewers.values().all(|v| v.stopped) {
        stop(p);
        return;
    }
    let wanted = settings(&p.config, &p.viewers);
    let applied = p.applied;
    if wanted.bitrate_kbps != applied.bitrate_kbps {
        let _ = p.commands.try_send(PipelineCommand::SetBitrate(wanted.bitrate_kbps));
    }
    if wanted.fps != applied.fps {
        let _ = p.commands.try_send(PipelineCommand::SetFps(wanted.fps));
    }
    if wanted.frame_limit != applied.frame_limit {
        let _ = p.commands.try_send(PipelineCommand::SetFrameLimit(wanted.frame_limit));
    }
    p.applied = wanted;
}

fn stop(p: &mut Pipeline) {
    p.stopping = true;
    info!("Nobody watches screen {} any more: stopping its pipeline", p.screen);
    // A pipeline is held here, so the map is only tried (lock order). When
    // it is busy the pipeline stays listed as stopping, which no viewer
    // joins, until its events end (start) or a new one takes its place (join).
    if let Ok(mut shared) = SHARED.try_lock() {
        if shared.get(&p.screen).is_some_and(|listed| listed.id == p.id) {
            shared.remove(&p.screen);
        }
    }
    let commands = p.commands.clone();
    let run = p.run.take();
    tokio::spawn(async move {
        let _ = commands.send(PipelineCommand::Stop).await;
        if let Some(run) = run {
            let abort = run.abort_handle();
            if tokio::time::timeout(STOP_GRACE, run).await.is_err() {
                warn!("The media pipeline did not stop within {:?}: aborting it", STOP_GRACE);
                abort.abort();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer(bitrate: Option<u32>, fps: Option<u32>, limit: Option<u32>, stopped: bool) -> Viewer {
        let (events, _) = mpsc::channel(1);
        Viewer { events, bitrate_kbps: bitrate, fps, frame_limit: limit, stopped, last_drop_keyframe: None }
    }

    fn config() -> StreamConfig {
        StreamConfig { bitrate_kbps: 20_000, fps: 60, ..StreamConfig::default() }
    }

    #[test]
    fn the_slowest_viewer_sets_the_pace() {
        let mut viewers = HashMap::new();
        viewers.insert(1, viewer(Some(15_000), None, Some(0), false));
        viewers.insert(2, viewer(Some(8_000), Some(30), Some(45), false));
        viewers.insert(3, viewer(Some(1_000), Some(10), Some(5), true));
        assert_eq!(settings(&config(), &viewers), Settings { bitrate_kbps: 8_000, fps: 30, frame_limit: 45 });
    }

    #[test]
    fn without_requests_the_pipeline_keeps_its_own_settings() {
        let mut viewers = HashMap::new();
        viewers.insert(1, viewer(None, None, Some(0), false));
        assert_eq!(settings(&config(), &viewers), Settings { bitrate_kbps: 20_000, fps: 60, frame_limit: 0 });
    }
}
