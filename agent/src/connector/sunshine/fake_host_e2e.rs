//! The fake host as a process of its own, for an end-to-end run through the
//! real server, agent and a browser (test-only code, like the rest of
//! `fake_host.rs`):
//!
//! `FAKE_SUNSHINE_DIR=<dir> FAKE_SUNSHINE_HTTP_PORT=57989 FAKE_SUNSHINE_VIDEO=<clip.h264>
//!  FAKE_SUNSHINE_AUDIO=<clip.opuspackets> cargo test -p agent --lib serve_for_e2e -- --ignored --nocapture`
//!
//! It listens on 127.0.0.1 on Sunshine's port family around the HTTP port
//! (`FixedPorts::family`), and is driven by files in `<dir>`:
//! - `pin`: the PIN "typed at the host" (nvhttp.cpp pin); taken when a pairing
//!   request waits, then removed. Never printed or written anywhere.
//! - `loss`: `within N` (the next N frames lose one packet each, which the
//!   parity makes good) or `beyond N` (the next N frames that are not key
//!   frames lose more packets than they have parity); removed when read.
//! - `stop`: the host ends.
//! - `status.json` (written by the host every 100 ms): what it holds and what
//!   it saw, as counts and kinds only (no keys, no PIN, no key codes).
//!
//! While a session runs it sends the clip's pictures at the frame rate the
//! client asked for (stream.cpp videoBroadcastThread), in a loop, from a key
//! frame whenever the client asks for one (IDX_REQUEST_IDR_FRAME), and the
//! Opus packets one per packet duration (audioBroadcastThread).
//!
//! The clip: H.264 Annex B with an access unit delimiter in front of every
//! picture (x264 `aud=1`), starting with a key frame. The sound: Opus
//! packets, each behind its length as two bytes (little-endian).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::*;

struct Clip {
    /// (key frame, access unit)
    frames: Vec<(bool, Vec<u8>)>,
}

impl Clip {
    fn load(path: &Path) -> Clip {
        let data = std::fs::read(path).expect("the video clip");
        const AUD: [u8; 5] = [0, 0, 0, 1, 0x09];
        let starts: Vec<usize> = data.windows(5).enumerate().filter(|(_, w)| *w == AUD).map(|(i, _)| i).collect();
        assert!(starts.first() == Some(&0), "the clip must start with an access unit delimiter (x264 aud=1)");
        let mut frames = Vec::new();
        for (n, start) in starts.iter().enumerate() {
            let end = starts.get(n + 1).copied().unwrap_or(data.len());
            let unit = data[*start..end].to_vec();
            // A slice of an IDR picture (nal_unit_type 5) makes it a key frame.
            let key = unit.windows(4).any(|w| w[0] == 0 && w[1] == 0 && w[2] == 1 && w[3] & 0x1F == 5);
            frames.push((key, unit));
        }
        assert!(frames.first().is_some_and(|f| f.0), "the clip must start with a key frame");
        Clip { frames }
    }

    /// The first key frame at or after `from`, the clip's start when there is none.
    fn key_frame_from(&self, from: usize) -> usize {
        (from..self.frames.len()).find(|i| self.frames[*i].0).unwrap_or(0)
    }
}

fn load_opus(path: &Path) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).expect("the audio clip");
    let mut packets = Vec::new();
    let mut at = 0;
    while at + 2 <= data.len() {
        let len = u16::from_le_bytes([data[at], data[at + 1]]) as usize;
        at += 2;
        packets.push(data[at..at + len].to_vec());
        at += len;
    }
    assert!(!packets.is_empty(), "the audio clip holds no packet");
    packets
}

#[derive(Default)]
struct Sent {
    frames: AtomicU64,
    key_frames: AtomicU64,
    /// Times the sender went to a key frame because the client asked.
    idr_restarts: AtomicU64,
    datagrams: AtomicU64,
    audio_packets: AtomicU64,
    loss_within_frames: AtomicU64,
    loss_beyond_frames: AtomicU64,
    datagrams_dropped: AtomicU64,
}

#[derive(Default)]
struct LossPlan {
    within: u32,
    beyond: u32,
}

fn env_port(name: &str, default: u16) -> u16 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

async fn video_sender(host: Arc<FakeHost>, clip: Arc<Clip>, sent: Arc<Sent>, loss: Arc<Mutex<LossPlan>>) {
    let mut next = tokio::time::Instant::now();
    let mut position = 0usize;
    let mut session_number = 0u32;
    let mut idr_seen = 0u32;
    loop {
        tokio::time::sleep_until(next).await;
        let now = tokio::time::Instant::now();
        let ready = {
            let mut st = host.state.lock().unwrap();
            let stream = &mut st.stream;
            let (started, requests) = (stream.sessions_started, stream.idr_requests);
            match stream.session.as_mut() {
                Some(session) if !session.stopping && session.video_peer.is_some() => {
                    if started != session_number {
                        // A new session starts with a key frame.
                        session_number = started;
                        idr_seen = requests;
                        position = 0;
                        next = now;
                    } else if requests != idr_seen {
                        // "idr_events->raise(true)": the next picture is a key frame.
                        idr_seen = requests;
                        if !clip.frames[position].0 {
                            position = clip.key_frame_from(position);
                            sent.idr_restarts.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    let (key, unit) = &clip.frames[position];
                    let frame = session.video.send(unit, *key);
                    let fps = session.config.framerate.clamp(1, 240) as u32;
                    // The packets of this frame that the way to the client loses.
                    let mut lose = 0usize;
                    if let Some((data_shards, datagrams)) = frame.blocks.first() {
                        let parity = datagrams.len() - data_shards;
                        let mut plan = loss.lock().unwrap();
                        if plan.beyond > 0 && !*key {
                            plan.beyond -= 1;
                            lose = (parity + 1).min(datagrams.len());
                            sent.loss_beyond_frames.fetch_add(1, Ordering::Relaxed);
                        } else if plan.within > 0 && parity >= 1 {
                            plan.within -= 1;
                            lose = 1;
                            sent.loss_within_frames.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Some((frame.all(), session.video_peer.unwrap(), *key, lose, fps, stream.path_mtu))
                }
                _ => None,
            }
        };
        let Some((datagrams, peer, key, lose, fps, path_mtu)) = ready else {
            session_number = if host.state.lock().unwrap().stream.session.is_none() { 0 } else { session_number };
            next = now + Duration::from_millis(5);
            continue;
        };
        for (index, datagram) in datagrams.iter().enumerate() {
            if index < lose || datagram.len() + 28 > path_mtu {
                sent.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let _ = host.video_socket.send_to(datagram, peer).await;
            sent.datagrams.fetch_add(1, Ordering::Relaxed);
        }
        sent.frames.fetch_add(1, Ordering::Relaxed);
        sent.key_frames.fetch_add(key as u64, Ordering::Relaxed);
        position = (position + 1) % clip.frames.len();
        next += Duration::from_secs(1) / fps;
        // Never a burst to catch up after a stall.
        if next + Duration::from_millis(100) < tokio::time::Instant::now() {
            next = tokio::time::Instant::now();
        }
    }
}

async fn audio_sender(host: Arc<FakeHost>, packets: Arc<Vec<Vec<u8>>>, sent: Arc<Sent>) {
    let mut next = tokio::time::Instant::now();
    let mut position = 0usize;
    loop {
        tokio::time::sleep_until(next).await;
        let ready = {
            let mut st = host.state.lock().unwrap();
            match st.stream.session.as_mut() {
                Some(session) if !session.stopping && session.audio_peer.is_some() => {
                    let duration = session.config.audio_packet_duration.clamp(1, 100) as u64;
                    Some((session.audio.send(&packets[position]), session.audio_peer.unwrap(), duration))
                }
                _ => None,
            }
        };
        let Some((datagrams, peer, duration)) = ready else {
            next = tokio::time::Instant::now() + Duration::from_millis(5);
            continue;
        };
        for datagram in &datagrams {
            let _ = host.audio_socket.send_to(datagram, peer).await;
        }
        sent.audio_packets.fetch_add(1, Ordering::Relaxed);
        position = (position + 1) % packets.len();
        next += Duration::from_millis(duration);
        if next + Duration::from_millis(100) < tokio::time::Instant::now() {
            next = tokio::time::Instant::now();
        }
    }
}

fn input_kind(seen: &InputSeen) -> &'static str {
    match seen {
        InputSeen::MoveRelative { .. } => "mouse_move_relative",
        InputSeen::MoveAbsolute { .. } => "mouse_move_absolute",
        InputSeen::Button { .. } => "mouse_button",
        InputSeen::Scroll { .. } => "scroll",
        InputSeen::HScroll { .. } => "hscroll",
        InputSeen::Key { .. } => "key",
        InputSeen::Text(_) => "text",
        InputSeen::ControllerArrival { .. } => "controller_arrival",
        InputSeen::Controller { .. } => "controller",
        InputSeen::Other { .. } => "other",
    }
}

/// What the host holds and saw: counts and kinds only.
fn status(host: &FakeHost, sent: &Sent, pins_entered: u32, last_config: &mut Option<serde_json::Value>, clip: &serde_json::Value) -> serde_json::Value {
    let st = host.state.lock().unwrap();
    let mut requests: BTreeMap<String, u32> = BTreeMap::new();
    for request in &st.requests {
        *requests.entry(request.clone()).or_default() += 1;
    }
    let mut input: BTreeMap<&str, u32> = BTreeMap::new();
    for seen in &st.stream.input {
        *input.entry(input_kind(seen)).or_default() += 1;
    }
    let mut rtsp: BTreeMap<String, u32> = BTreeMap::new();
    for seen in &st.stream.rtsp {
        *rtsp.entry(seen.method.clone()).or_default() += 1;
    }
    if let Some(session) = &st.stream.session {
        let c = &session.config;
        *last_config = Some(serde_json::json!({
            "width": c.width, "height": c.height, "framerate": c.framerate, "packetsize": c.packetsize,
            "video_format": c.video_format, "bitrate_kbps": c.bitrate, "encryption_flags_enabled": c.encryption_flags_enabled,
            "audio_packet_duration": c.audio_packet_duration, "audio_channels": c.audio_channels,
            "min_required_fec_packets": c.min_required_fec_packets, "rtsp_encrypted": session.launch.rtsp_encrypted,
        }));
    }
    // Of the launch only what is no secret (never rikey / rikeyid).
    let launch: BTreeMap<&str, &str> = st
        .launch_args
        .as_ref()
        .map(|args| {
            ["mode", "appid", "sops", "localAudioPlayMode", "corever", "surroundAudioInfo"]
                .into_iter()
                .filter_map(|k| args.get(k).map(|v| (k, v.as_str())))
                .collect()
        })
        .unwrap_or_default();
    let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
    serde_json::json!({
        "pairing_sessions": st.sessions.len(),
        "pairing_waiting_for_pin": st.sessions.values().filter(|s| s.phase == Phase::None).count(),
        "paired": st.paired.len(),
        "pins_entered": pins_entered,
        "waiting_connection_closed": st.waiting_connection_closed,
        "running_app": st.running_app != 0,
        "state": if st.running_app != 0 { "BUSY" } else { "FREE" },
        "cancels": st.cancels,
        "requests": requests,
        "launch": launch,
        "stream": {
            "session": st.stream.session.is_some(),
            "launch_pending": st.stream.launch.is_some(),
            "sessions_started": st.stream.sessions_started,
            "config": last_config,
            "rtsp": rtsp,
            "control_connects": st.stream.control_connects,
            "control_disconnects": st.stream.control_disconnects,
            "control_refused": st.stream.control_refused,
            "control_rejected": st.stream.control_rejected,
            "control_messages": st.stream.control.len(),
            "idr_requests": st.stream.idr_requests,
            "periodic_pings": st.stream.periodic_pings,
            "video_pings": st.stream.video_pings,
            "audio_pings": st.stream.audio_pings,
            "fec_status": st.stream.fec_status.len(),
            "malformed_input": st.stream.malformed_input,
            "input": input,
            "input_total": st.stream.input.len(),
            "video_too_big": st.stream.video_too_big,
        },
        "sent": {
            "frames": n(&sent.frames), "key_frames": n(&sent.key_frames), "idr_restarts": n(&sent.idr_restarts),
            "datagrams": n(&sent.datagrams), "audio_packets": n(&sent.audio_packets),
            "loss_within_frames": n(&sent.loss_within_frames), "loss_beyond_frames": n(&sent.loss_beyond_frames),
            "datagrams_dropped": n(&sent.datagrams_dropped),
        },
        "clip": clip,
    })
}

fn write_status(dir: &Path, status: &serde_json::Value) {
    let tmp = dir.join("status.json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(status).unwrap()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("status.json"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a stand-alone fake host for the end-to-end harness; needs FAKE_SUNSHINE_DIR"]
async fn serve_for_e2e() {
    let Ok(dir) = std::env::var("FAKE_SUNSHINE_DIR") else {
        eprintln!("FAKE_SUNSHINE_DIR is not set: nothing to serve");
        return;
    };
    let dir = PathBuf::from(dir);
    let http = env_port("FAKE_SUNSHINE_HTTP_PORT", 57989);
    // The ports of a real Sunshine with default settings are never taken.
    assert!(!(47900..=48100).contains(&http), "this port family belongs to a real Sunshine");
    let seconds: u64 = std::env::var("FAKE_SUNSHINE_SECONDS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(900);
    let clip = Arc::new(Clip::load(Path::new(&std::env::var("FAKE_SUNSHINE_VIDEO").expect("FAKE_SUNSHINE_VIDEO"))));
    let opus = Arc::new(load_opus(Path::new(&std::env::var("FAKE_SUNSHINE_AUDIO").expect("FAKE_SUNSHINE_AUDIO"))));
    let clip_facts = serde_json::json!({
        "frames": clip.frames.len(),
        "key_frames": clip.frames.iter().filter(|f| f.0).count(),
        "audio_packets": opus.len(),
        "audio_packet_bytes_min": opus.iter().map(Vec::len).min(),
        "audio_packet_bytes_max": opus.iter().map(Vec::len).max(),
    });

    let ports = FixedPorts::family(http);
    let host = Arc::new(FakeHost::start_on(ports).await);
    // What a Sunshine with H.264 only says (the clip is H.264).
    {
        let mut st = host.state.lock().unwrap();
        st.stream.hevc = false;
        st.stream.av1 = false;
        if std::env::var("FAKE_SUNSHINE_ENCRYPTION").as_deref() == Ok("opportunistic") {
            st.stream.encryption_mode = EncryptionMode::Opportunistic;
        }
    }
    let sent = Arc::new(Sent::default());
    let loss = Arc::new(Mutex::new(LossPlan::default()));
    let senders = [
        tokio::spawn(video_sender(host.clone(), clip.clone(), sent.clone(), loss.clone())),
        tokio::spawn(audio_sender(host.clone(), opus.clone(), sent.clone())),
    ];
    println!(
        "fake host: http {} https {} rtsp {} video {} control {} audio {} (127.0.0.1); {} pictures, {} sound packets",
        ports.http,
        ports.https,
        ports.rtsp,
        ports.video,
        ports.control,
        ports.audio,
        clip.frames.len(),
        opus.len()
    );

    let started = Instant::now();
    let mut pins_entered = 0u32;
    let mut last_config = None;
    let (pin_file, loss_file, stop_file) = (dir.join("pin"), dir.join("loss"), dir.join("stop"));
    while !stop_file.exists() && started.elapsed().as_secs() < seconds {
        // The person types the PIN at the host.
        let waiting = host.state.lock().unwrap().sessions.values().any(|s| s.phase == Phase::None);
        if waiting {
            if let Ok(pin) = std::fs::read_to_string(&pin_file) {
                let _ = std::fs::remove_file(&pin_file);
                host.enter_pin(pin.trim());
                pins_entered += 1;
            }
        }
        if let Ok(text) = std::fs::read_to_string(&loss_file) {
            let _ = std::fs::remove_file(&loss_file);
            let mut words = text.split_whitespace();
            let (kind, count) = (words.next().unwrap_or(""), words.next().and_then(|n| n.parse().ok()).unwrap_or(1));
            let mut plan = loss.lock().unwrap();
            match kind {
                "within" => plan.within = count,
                "beyond" => plan.beyond = count,
                _ => {}
            }
        }
        write_status(&dir, &status(&host, &sent, pins_entered, &mut last_config, &clip_facts));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    write_status(&dir, &status(&host, &sent, pins_entered, &mut last_config, &clip_facts));
    for sender in senders {
        sender.abort();
    }
    println!("fake host: stopped after {} s", started.elapsed().as_secs());
}
