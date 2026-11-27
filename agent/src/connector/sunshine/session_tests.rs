//! Whole sessions of the connector against the host of `fake_host.rs`:
//! set-up (pairing), connect, video with loss and recovery, audio, every
//! command, and the ways a session ends.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;

use super::super::super::{Button, Codec, Command, Connector, Event, Session, SetupStep, Target, Viewer};
use super::super::fake_host::{EncryptionMode, FakeHost, InputSeen, State, DESKTOP_ID};
use super::super::http::App;
use super::super::video::tests::{make_frame, Lcg};
use super::super::video::VideoCodec;
use super::super::{SunshineConnector, SETUP_MESSAGE};
use super::*;

// ---------------------------------------------------------------------------
// The small parts
// ---------------------------------------------------------------------------

#[test]
fn resolutions_and_bitrates_as_people_write_them() {
    assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
    assert_eq!(parse_resolution(" 2560 X 1440 "), Some((2560, 1440)));
    assert_eq!(parse_resolution("1280×720"), Some((1280, 720)));
    assert_eq!(parse_resolution(""), Some((1920, 1080)));
    // "Height must not be odd"
    assert_eq!(parse_resolution("1366x769"), Some((1366, 768)));
    for bad in ["1920", "x1080", "1920x", "axb", "0x0", "20000x1000", "1920x1080x60", "-1920x1080"] {
        assert_eq!(parse_resolution(bad), None, "{bad}");
    }
    assert_eq!(parse_bitrate_kbps("20"), Some(20_000));
    assert_eq!(parse_bitrate_kbps(" 7.5 "), Some(7_500));
    assert_eq!(parse_bitrate_kbps("7,5"), Some(7_500));
    assert_eq!(parse_bitrate_kbps(""), Some(20_000));
    assert_eq!(parse_bitrate_kbps("0.1"), Some(500));
    assert_eq!(parse_bitrate_kbps("900"), Some(150_000));
    for bad in ["fast", "0", "-5", "NaN", "inf"] {
        assert_eq!(parse_bitrate_kbps(bad), None, "{bad}");
    }
}

#[test]
fn applications_by_name() {
    let apps = vec![
        App { id: 1, title: "Desktop".into(), hdr_supported: false },
        App { id: 2, title: "Steam Big Picture".into(), hdr_supported: false },
    ];
    assert_eq!(find_app(&apps, "desktop").unwrap().id, 1);
    assert_eq!(find_app(&apps, "").unwrap().id, 1);
    assert_eq!(find_app(&apps, " STEAM big picture ").unwrap().id, 2);
    assert_eq!(
        find_app(&apps, "Notepad").unwrap_err().to_string(),
        "The other computer's Sunshine has no application \"Notepad\". It has: \"Desktop\", \"Steam Big Picture\""
    );
    assert_eq!(find_app(&[], "Desktop").unwrap_err().to_string(), "The other computer's Sunshine lists no applications");
}

#[test]
fn pings_as_the_host_expects_them() {
    // SS_PING: the 16 characters of X-SS-Ping-Payload, then a big-endian counter from 1.
    let payload = Some(*b"0123456789ABCDEF");
    let mut count = 0;
    assert_eq!(ping_datagram(&payload, &mut count), b"0123456789ABCDEF\x00\x00\x00\x01");
    assert_eq!(ping_datagram(&payload, &mut count), b"0123456789ABCDEF\x00\x00\x00\x02");
    // No payload from the host (an old one): the four letters.
    let mut count = 0;
    assert_eq!(ping_datagram(&None, &mut count), b"PING");
    assert_eq!(ping_datagram(&Some([0; 16]), &mut count), b"PING");
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

const WAIT: Duration = Duration::from_secs(10);

fn target(host: &FakeHost, values: &BTreeMap<String, String>) -> Target {
    let mut all = values.clone();
    all.insert("host".into(), "127.0.0.1".into());
    all.insert("port".into(), host.http_port.to_string());
    all.entry("app".into()).or_insert_with(|| "Desktop".into());
    all.entry("resolution".into()).or_insert_with(|| "1280x720".into());
    all.entry("bitrate".into()).or_insert_with(|| "20".into());
    Target { connector: "sunshine".into(), values: all }
}

/// A target whose pairing the host already knows (without the exchange).
fn paired_target(host: &FakeHost) -> Target {
    let (identity, server_cert) = host.pair_directly("00000000000000C3");
    let mut values = BTreeMap::new();
    values.insert("client_cert".to_string(), identity.client_cert_pem);
    values.insert("client_key".to_string(), identity.client_key_pem);
    values.insert("server_cert".to_string(), server_cert);
    values.insert("unique_id".to_string(), identity.unique_id);
    target(host, &values)
}

fn viewer(codecs: &[Codec]) -> Viewer {
    Viewer { codecs: codecs.to_vec(), fps: 60, width: 0, height: 0, bitrate_kbps: 0 }
}

async fn next_event(session: &mut Session) -> Event {
    tokio::time::timeout(WAIT, session.events.recv()).await.expect("an event in time").expect("the session still runs")
}

/// The next picture: (codec, key, data); sound in between is skipped.
async fn next_video(session: &mut Session) -> (Codec, bool, Bytes) {
    loop {
        match next_event(session).await {
            Event::Video { codec, key, capture_time_ms, data } => {
                assert!(capture_time_ms.is_some());
                return (codec, key, data);
            }
            // The round trip to the host, whenever it has settled.
            Event::Audio { .. } | Event::Latency { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// The session's end: its reason. Pictures and sound before it are skipped.
async fn closed(session: &mut Session) -> String {
    loop {
        match next_event(session).await {
            Event::Closed { reason } => {
                // "The end; nothing follows."
                assert!(tokio::time::timeout(WAIT, session.events.recv()).await.unwrap().is_none());
                return reason;
            }
            Event::Video { .. } | Event::Audio { .. } | Event::Latency { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn both_pings(st: &State) -> bool {
    st.stream.session.as_ref().is_some_and(|s| s.video_peer.is_some() && s.audio_peer.is_some())
}

/// Connects and waits until the host knows where to send.
async fn connected(host: &FakeHost, connector: &SunshineConnector, target: &Target, viewer: &Viewer) -> Session {
    let mut session = connector.connect(target, viewer).await.expect("a session");
    // The pointer is part of Sunshine's picture for the whole session: said once, first.
    assert_eq!(next_event(&mut session).await, Event::CursorAlwaysEmbedded);
    // Then the one display, in the size of the stream (pointer positions are in its pixels).
    match next_event(&mut session).await {
        Event::Displays { displays, current: 0 } if displays.len() == 1 => {
            let asked = super::parse_resolution(target.get("resolution")).unwrap();
            assert_eq!((displays[0].width, displays[0].height), asked);
        }
        other => panic!("unexpected {other:?}"),
    }
    host.wait("the UDP pings", both_pings).await;
    session
}

fn requests_of(host: &FakeHost, path: &str) -> usize {
    host.state.lock().unwrap().requests.iter().filter(|r| r.as_str() == path).count()
}

#[tokio::test]
async fn set_up_then_a_whole_session() {
    let host = FakeHost::start().await;
    let connector = Arc::new(SunshineConnector::with_no_input(false));

    // --- Set-up: one step with the PIN; the person types it at the host. ---
    let (steps_tx, mut steps) = mpsc::channel::<SetupStep>(4);
    let setup = {
        let (connector, target) = (connector.clone(), target(&host, &BTreeMap::new()));
        tokio::spawn(async move { connector.setup(&target, steps_tx).await })
    };
    let step = tokio::time::timeout(Duration::from_secs(120), steps.recv()).await.unwrap().expect("a set-up step");
    assert_eq!(step.message, SETUP_MESSAGE);
    let pin = step.code.expect("the PIN");
    assert!(pin.len() == 4 && pin.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(host.wait_pending().await, "Lunaris");
    host.enter_pin(&pin);
    let values = setup.await.unwrap().expect("pairing");
    assert_eq!(values.keys().map(String::as_str).collect::<Vec<_>>(), ["client_cert", "client_key", "server_cert", "unique_id"]);
    assert!(steps.recv().await.is_none(), "one step only");
    assert_eq!(host.paired_count(), 1);

    // --- Connect: H.265 is the first of the viewer's codecs the host has. ---
    let target = target(&host, &values);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::Vp9, Codec::H265, Codec::H264])).await;
    {
        let st = host.state.lock().unwrap();
        assert_eq!(st.running_app, DESKTOP_ID);
        let args = st.launch_args.as_ref().unwrap();
        assert_eq!((args["mode"].as_str(), args["appid"].as_str(), args["corever"].as_str()), ("1280x720x60", DESKTOP_ID.to_string().as_str(), "1"));
        let s = st.stream.session.as_ref().unwrap();
        assert_eq!((s.config.width, s.config.height, s.config.framerate, s.config.video_format), (1280, 720, 60, 1));
        assert_eq!(s.config.configured_bitrate_kbps, 20_000);
        assert!(st.stream.rtsp.iter().all(|r| r.encrypted));
        assert_eq!(st.stream.control_connects, 1);
        // Start A is the IDR request.
        assert_eq!(st.stream.idr_requests, 1);
    }
    // The pings go on, counting (VideoStream.c VideoPingThreadProc).
    host.wait("a second video ping", |st| st.stream.last_video_ping_seq >= 2).await;

    // --- Video: frames arrive as sent; loss asks for an IDR frame and recovers. ---
    let mut rng = Lcg(7);
    let codec = VideoCodec::H265;
    let key_frame = make_frame(codec, true, 30_000, &mut rng);
    host.send_video(&key_frame, true, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H265, true, Bytes::from(key_frame)));
    let frame = make_frame(codec, false, 5_000, &mut rng);
    host.send_video(&frame, false, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H265, false, Bytes::from(frame)));
    // One packet lost: the parity brings it back, and the host is told for its log.
    let frame = make_frame(codec, false, 5_000, &mut rng);
    let datagrams = host.send_video(&frame, false, &[1]).await;
    assert_eq!(datagrams, 6, "4 data and 2 parity packets of 1392 bytes (the host is on the local link here)");
    assert_eq!(next_video(&mut session).await, (Codec::H265, false, Bytes::from(frame)));
    host.wait("the FEC status of the recovered block", |st| !st.stream.fec_status.is_empty()).await;
    // Three of six lost: gone. The next whole frame shows it; the host is
    // asked for an IDR frame, and what comes before that frame is not shown.
    let lost = make_frame(codec, false, 5_000, &mut rng);
    host.send_video(&lost, false, &[0, 1, 2]).await;
    let after = make_frame(codec, false, 2_000, &mut rng);
    host.send_video(&after, false, &[]).await;
    host.wait("the IDR request after the loss", |st| st.stream.idr_requests == 2).await;
    let key_frame = make_frame(codec, true, 30_000, &mut rng);
    host.send_video(&key_frame, true, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H265, true, Bytes::from(key_frame)));
    let frame = make_frame(codec, false, 5_000, &mut rng);
    host.send_video(&frame, false, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H265, false, Bytes::from(frame)));

    // --- Audio: what queued up before this side listened is dropped (until the
    // socket was idle once), then Opus packets come in order, two channels. ---
    host.send_audio_block(1, 96).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut sent: Vec<Vec<u8>> = Vec::new();
    for seed in 2..8u8 {
        sent.extend(host.send_audio_block(seed.wrapping_mul(40), 96).await);
    }
    let mut heard: Vec<Vec<u8>> = Vec::new();
    while heard.len() < 16 {
        match next_event(&mut session).await {
            Event::Audio { channels, data } => {
                assert_eq!(channels, 2);
                heard.push(data.to_vec());
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    let first = sent.iter().position(|p| *p == heard[0]).expect("a packet that was sent");
    assert_eq!(heard[..], sent[first..first + heard.len()], "in order, nothing missing");

    // --- Commands. ---
    let commands = [
        Command::PointerMove { x: 640, y: 360 },
        Command::PointerMoveRelative { dx: 5, dy: -3 },
        Command::PointerButton { button: Button::Left, down: true },
        Command::PointerButton { button: Button::Left, down: false },
        Command::Wheel { dx: 1, dy: -2 },
        Command::Key { position: 42, down: true },
        Command::Key { position: 30, down: true },
        Command::Key { position: 30, down: false },
        Command::Key { position: 42, down: false },
        Command::Text { text: "héllo wörld".into() },
        // A controller plugged in (arrival + state), then unplugged.
        Command::Gamepad {
            id: 1,
            kind: 1,
            state: Some(crate::gamepad::PadState { buttons: crate::gamepad::buttons::B, lt: 0, rt: 200, lx: 0, ly: 1000, rx: 0, ry: 0 }),
        },
        Command::Gamepad { id: 1, kind: 0, state: None },
        // Ignored: GameStream fixes these at the start and has no clipboard.
        Command::Quality { fps: 30, bitrate_kbps: 5000 },
        Command::SwitchDisplay { index: 1 },
        Command::Clipboard { text: "not sent".into() },
        Command::RequestKeyFrame,
    ];
    for command in commands {
        session.commands.send(command).await.unwrap();
    }
    host.wait("the input and the key frame request", |st| st.stream.input.len() == 14 && st.stream.idr_requests == 3).await;
    {
        let st = host.state.lock().unwrap();
        let (controller, others): (Vec<InputSeen>, Vec<InputSeen>) = st
            .stream
            .input
            .iter()
            .cloned()
            .partition(|i| matches!(i, InputSeen::Controller { .. } | InputSeen::ControllerArrival { .. }));
        assert_eq!(
            controller,
            vec![
                InputSeen::ControllerArrival { number: 1, kind: 1, capabilities: 1, buttons: 0xF7FF },
                InputSeen::Controller { number: 1, active_mask: 0b10, buttons: 0x2000, lt: 0, rt: 200, sticks: [0, -1000, 0, 0] },
                InputSeen::Controller { number: 1, active_mask: 0, buttons: 0, lt: 0, rt: 0, sticks: [0; 4] },
            ]
        );
        let (keyboard, mouse): (Vec<InputSeen>, Vec<InputSeen>) =
            others.into_iter().partition(|i| matches!(i, InputSeen::Key { .. } | InputSeen::Text(_)));
        assert_eq!(
            mouse,
            vec![
                // The reference size is the stream's (less one, as Moonlight sends it).
                InputSeen::MoveAbsolute { x: 640, y: 360, width: 1279, height: 719 },
                InputSeen::MoveRelative { dx: 5, dy: -3 },
                InputSeen::Button { button: 1, release: false },
                InputSeen::Button { button: 1, release: true },
                InputSeen::Scroll { amount: -240 },
                InputSeen::HScroll { amount: 120 },
            ]
        );
        assert_eq!(
            keyboard,
            vec![
                InputSeen::Key { vk: 0xA0, release: false, modifiers: 1, flags: 0 },
                InputSeen::Key { vk: 0x41, release: false, modifiers: 1, flags: 0 },
                InputSeen::Key { vk: 0x41, release: true, modifiers: 1, flags: 0 },
                InputSeen::Key { vk: 0xA0, release: true, modifiers: 0, flags: 0 },
                InputSeen::Text("héllo wörld".into()),
            ]
        );
        assert_eq!(st.stream.malformed_input, 0);
        // Input travels on the mouse and keyboard channels (controller 1 on
        // 0x11), the IDR request on the urgent one.
        for message in &st.stream.control {
            match message.ptype {
                0x0206 => assert!(matches!(message.channel, 2 | 3 | 0x11)),
                0x0302 if message.seq != 0 => assert_eq!(message.channel, 1),
                _ => assert_eq!(message.channel, 0),
            }
        }
        assert_eq!(st.stream.control_rejected, 0);
        assert_eq!(st.cancels, 0);
    }

    // --- Close: the control stream says goodbye, the application is ended,
    // the host is free again; then the session says it is over. ---
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
    let st = host.state.lock().unwrap();
    assert_eq!(st.stream.control_disconnects, 1);
    assert!(st.stream.session.is_none());
    assert_eq!((st.cancels, st.running_app), (1, 0));
    let launch = st.requests.iter().position(|r| r == "https /launch").unwrap();
    let cancel = st.requests.iter().position(|r| r == "https /cancel").unwrap();
    assert!(launch < cancel);
    // HostClient::cancel looks afterwards whether the host is free.
    assert_eq!(st.requests.last().map(String::as_str), Some("https /serverinfo"));
    assert!(!st.requests.iter().any(|r| r == "https /resume"));
}

#[tokio::test]
async fn an_application_that_already_ran_is_joined_and_left_running() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    // Somebody (another Moonlight) started it before.
    host.state.lock().unwrap().running_app = DESKTOP_ID;
    let connector = SunshineConnector::with_no_input(false);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    assert_eq!(requests_of(&host, "https /resume"), 1);
    assert_eq!(requests_of(&host, "https /launch"), 0);
    // H.264 because the viewer has nothing else.
    let frame = make_frame(VideoCodec::H264, true, 9_000, &mut Lcg(3));
    host.send_video(&frame, true, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H264, true, Bytes::from(frame)));

    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
    let st = host.state.lock().unwrap();
    assert_eq!(st.stream.control_disconnects, 1);
    // Not ours to end.
    assert_eq!((st.cancels, st.running_app), (0, DESKTOP_ID));
    assert!(!st.requests.iter().any(|r| r == "https /cancel"));
}

/// The round trip measured on the control stream's datagrams reaches the
/// agent as `Event::Latency` (control.rs `RoundTrips`, `round_trip_to_tell`).
#[tokio::test]
async fn the_round_trip_to_the_host_is_told() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(false);
    let started = std::time::Instant::now();
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    // A picture, so that the session does not end for the lack of one.
    let frame = make_frame(VideoCodec::H264, true, 4_000, &mut Lcg(3));
    host.send_video(&frame, true, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H264, true, Bytes::from(frame)));
    // What comes within four seconds: the first within two (no waiting for
    // ENet's own figure to come down from 500 ms), each on the loopback.
    let mut told = Vec::new();
    while started.elapsed() < Duration::from_secs(4) {
        match tokio::time::timeout(Duration::from_millis(200), session.events.recv()).await {
            Ok(Some(Event::Latency { ms })) => {
                if told.is_empty() {
                    assert!(started.elapsed() < Duration::from_millis(2_500), "first told after {:?}", started.elapsed());
                }
                told.push(ms);
            }
            Ok(Some(Event::Audio { .. } | Event::Video { .. })) | Err(_) => {}
            Ok(other) => panic!("unexpected {other:?}"),
        }
    }
    println!("round trips to the fake Sunshine host on the loopback: {:?} ms", told);
    assert!(!told.is_empty());
    assert!(told.iter().all(|ms| (0.0..20.0).contains(ms)), "{told:?}");
}

#[tokio::test]
async fn dropping_the_session_ends_it_at_the_host() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(false);
    let session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    assert_eq!(host.state.lock().unwrap().running_app, DESKTOP_ID);
    drop(session);
    host.wait("the goodbye and the cancel after a drop", |st| st.stream.control_disconnects == 1 && st.cancels == 1 && st.running_app == 0).await;
}

#[tokio::test]
async fn the_test_switch_keeps_all_input_back() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(true);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    for command in [
        Command::PointerMove { x: 10, y: 10 },
        Command::PointerMoveRelative { dx: 1, dy: 1 },
        Command::PointerButton { button: Button::Right, down: true },
        Command::PointerButton { button: Button::Right, down: false },
        Command::Wheel { dx: 1, dy: 1 },
        Command::Key { position: 30, down: true },
        Command::Key { position: 30, down: false },
        Command::Text { text: "nothing of this".into() },
        // Not input: this one must still work, and it comes after the others.
        Command::RequestKeyFrame,
    ] {
        session.commands.send(command).await.unwrap();
    }
    host.wait("the key frame request", |st| st.stream.idr_requests == 2).await;
    // A few more pings, so that anything sent after the request would be here too.
    let pings = host.state.lock().unwrap().stream.periodic_pings;
    host.wait("three more pings", move |st| st.stream.periodic_pings >= pings + 3).await;
    {
        let st = host.state.lock().unwrap();
        assert!(st.stream.input.is_empty());
        assert_eq!(st.stream.malformed_input, 0);
        assert!(st.stream.control.iter().all(|m| m.ptype != 0x0206), "no input message of any kind");
        assert!(st.stream.control.iter().all(|m| matches!(m.ptype, 0x0302 | 0x0307 | 0x0200)));
    }
    // Closing works as always.
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
    assert_eq!(host.state.lock().unwrap().cancels, 1);
}

#[tokio::test]
async fn the_host_ends_the_session() {
    // Before any picture: something is wrong at the host.
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(false);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    host.send_control(0x010b, &[0, 0, 0, 0, 0, 0, 1, 0, 1, 0]); // rumble: ignored
    host.send_control(0x0109, &0x8003_0023u32.to_be_bytes());
    assert_eq!(
        closed(&mut session).await,
        "The other computer ended the session before it sent a picture. Its screen may be off or locked."
    );
    // The application was ours: it is ended all the same.
    assert_eq!(host.state.lock().unwrap().cancels, 1);

    // After pictures: an ordinary end.
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    let frame = make_frame(VideoCodec::H264, true, 4_000, &mut Lcg(9));
    host.send_video(&frame, true, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H264, true, Bytes::from(frame)));
    host.send_control(0x0109, &0x8003_0023u32.to_be_bytes());
    assert_eq!(closed(&mut session).await, "The session was ended at the other computer");

    // The host goes away without a word (its session is cancelled by someone
    // else: the ENet peer is dropped).
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    host.state.lock().unwrap().stream.session.as_mut().unwrap().stop();
    assert_eq!(closed(&mut session).await, "The connection to the other computer was lost");
}

#[tokio::test]
async fn encrypted_video_and_the_hosts_demands() {
    for mode in [EncryptionMode::Opportunistic, EncryptionMode::Mandatory] {
        let host = FakeHost::start().await;
        host.state.lock().unwrap().stream.encryption_mode = mode;
        let target = paired_target(&host);
        let connector = SunshineConnector::with_no_input(false);
        let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264, Codec::H265])).await;
        {
            let st = host.state.lock().unwrap();
            let config = &st.stream.session.as_ref().unwrap().config;
            // Everything the host can encrypt is encrypted.
            assert_eq!(config.encryption_flags_enabled, 7, "{mode:?}");
            // 1392 (the host is on the local link here) less the 32 bytes of the encryption header.
            assert_eq!(config.packetsize, 1360);
        }
        let mut rng = Lcg(11);
        let key_frame = make_frame(VideoCodec::H264, true, 20_000, &mut rng);
        host.send_video(&key_frame, true, &[]).await;
        assert_eq!(next_video(&mut session).await, (Codec::H264, true, Bytes::from(key_frame)));
        let frame = make_frame(VideoCodec::H264, false, 3_000, &mut rng);
        host.send_video(&frame, false, &[0]).await;
        assert_eq!(next_video(&mut session).await, (Codec::H264, false, Bytes::from(frame)));
        session.commands.send(Command::Close).await.unwrap();
        assert_eq!(closed(&mut session).await, "The session was closed");
    }
}

#[tokio::test]
async fn what_stops_a_session_before_it_starts() {
    let connector = SunshineConnector::with_no_input(false);
    let error = |result: anyhow::Result<Session>| result.err().expect("an error").to_string();

    // Never paired: no values from a set-up.
    let host = FakeHost::start().await;
    let unpaired = target(&host, &BTreeMap::new());
    assert_eq!(error(connector.connect(&unpaired, &viewer(&[Codec::H264])).await), "This computer is not paired yet. Remove it and add it again.");
    assert!(host.state.lock().unwrap().requests.is_empty(), "not even asked");

    // Paired once, but the host has forgotten this client.
    let good = paired_target(&host);
    host.state.lock().unwrap().paired.clear();
    assert_eq!(error(connector.connect(&good, &viewer(&[Codec::H264])).await), "This computer does not accept the pairing. Pair with it again.");
    assert_eq!(requests_of(&host, "https /launch"), 0);

    let host = FakeHost::start().await;
    let good = paired_target(&host);
    // What the person wrote wrong.
    let with = |key: &str, value: &str| {
        let mut t = good.clone();
        t.values.insert(key.into(), value.into());
        t
    };
    assert_eq!(
        error(connector.connect(&with("app", "Notepad"), &viewer(&[Codec::H264])).await),
        "The other computer's Sunshine has no application \"Notepad\". It has: \"Desktop\", \"Steam & Big Picture\""
    );
    assert_eq!(
        error(connector.connect(&with("resolution", "big"), &viewer(&[Codec::H264])).await),
        "\"big\" is not a resolution. Write it like 1920x1080."
    );
    assert_eq!(
        error(connector.connect(&with("bitrate", "fast"), &viewer(&[Codec::H264])).await),
        "\"fast\" is not a bitrate. Write the megabits per second, like 20."
    );
    assert_eq!(error(connector.connect(&with("port", "http"), &viewer(&[Codec::H264])).await), "\"http\" is not a port");
    // A viewer that shows nothing Sunshine sends.
    assert_eq!(
        error(connector.connect(&good, &viewer(&[Codec::Vp8, Codec::Vp9])).await),
        "Sunshine sends H.264, H.265 or AV1 video, and this viewer shows none of them"
    );
    assert_eq!(requests_of(&host, "https /launch"), 0);
    assert_eq!(host.state.lock().unwrap().cancels, 0);

    // After the launch: the host turns out not to have the viewer's only
    // codec. The application that was started is ended again.
    assert_eq!(
        error(connector.connect(&good, &viewer(&[Codec::Av1])).await),
        "The other computer offers no video format that this viewer can show"
    );
    {
        let st = host.state.lock().unwrap();
        assert_eq!(requests_of_state(&st, "https /launch"), 1);
        assert_eq!((st.cancels, st.running_app), (1, 0));
        assert!(st.stream.session.is_none());
    }

    // Nobody there.
    let nobody = {
        let mut t = good.clone();
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        t.values.insert("port".into(), free.to_string());
        t
    };
    assert_eq!(error(connector.connect(&nobody, &viewer(&[Codec::H264])).await), "Could not reach the computer");
}

fn requests_of_state(st: &State, path: &str) -> usize {
    st.requests.iter().filter(|r| r.as_str() == path).count()
}

#[tokio::test]
async fn giving_up_while_connecting_still_frees_the_host() {
    // The control port does not answer (a firewall): connect() would wait ten
    // seconds; whoever asked gives up after one. The launch stood, so the
    // host is told to end the application.
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    host.state.lock().unwrap().stream.control_port = {
        // A UDP port nobody reads.
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        std::mem::forget(socket);
        port
    };
    let connector = SunshineConnector::with_no_input(false);
    let attempt = tokio::time::timeout(Duration::from_secs(1), connector.connect(&target, &viewer(&[Codec::H264]))).await;
    assert!(attempt.is_err(), "still connecting after a second");
    host.wait("the cancel after giving up", |st| st.cancels == 1 && st.running_app == 0).await;
}

/// Found by the end-to-end run: every few sessions the control stream took
/// half a second longer to connect. ENet's CONNECT went out through
/// `try_send_to` on a socket the runtime did not yet know to be writable,
/// was dropped unseen, and was only sent again at ENet's first time-out
/// (500 ms). With several runtime threads, as in the agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connecting_never_waits_for_enets_first_retransmission() {
    let host = FakeHost::start().await;
    let connector = SunshineConnector::with_no_input(false);
    let target = paired_target(&host);
    let mut took = Vec::new();
    for _ in 0..10 {
        let before = std::time::Instant::now();
        let mut session = connector.connect(&target, &viewer(&[Codec::H264])).await.expect("a session");
        took.push(before.elapsed().as_millis());
        session.commands.send(Command::Close).await.unwrap();
        loop {
            if let Event::Closed { .. } = next_event(&mut session).await {
                break;
            }
        }
    }
    assert!(took.iter().all(|ms| *ms < 400), "connecting took (ms): {took:?}");
    assert_eq!(host.state.lock().unwrap().stream.control_connects, 10);
}

// ---------------------------------------------------------------------------
// The viewer's wishes, the pointer, and what must hold whoever answers
// ---------------------------------------------------------------------------

#[test]
fn the_viewers_size_and_bitrate_within_the_computers() {
    // No wish: the computer's own.
    assert_eq!(stream_size((1920, 1080), (0, 0)), (1920, 1080));
    assert_eq!(stream_size((1920, 1080), (1280, 0)), (1920, 1080));
    // A smaller wish is followed, a larger one is not.
    assert_eq!(stream_size((1920, 1080), (1280, 720)), (1280, 720));
    assert_eq!(stream_size((1920, 1080), (1920, 1080)), (1920, 1080));
    assert_eq!(stream_size((1920, 1080), (3840, 2160)), (1920, 1080));
    assert_eq!(stream_size((1920, 1080), (2560, 1080)), (1920, 810));
    // Upright (a phone): made to fit, its proportions kept, even numbers.
    assert_eq!(stream_size((1920, 1080), (1080, 1920)), (608, 1080));
    assert_eq!(stream_size((1920, 1080), (853, 481)), (852, 480));
    assert_eq!(stream_size((1920, 1080), (10, 10)), (64, 64));
    assert_eq!(stream_bitrate(20_000, 0), 20_000);
    assert_eq!(stream_bitrate(20_000, 8_000), 8_000);
    assert_eq!(stream_bitrate(20_000, 50_000), 20_000);
    assert_eq!(stream_bitrate(20_000, 100), 500);
    assert_eq!(parse_pointer(""), Some(false));
    assert_eq!(parse_pointer(" Picture "), Some(false));
    assert_eq!(parse_pointer("own"), Some(true));
    assert_eq!(parse_pointer("OWN"), Some(true));
    assert_eq!(parse_pointer("both"), None);
}

#[tokio::test]
async fn the_host_is_asked_for_what_the_viewer_wants() {
    let host = FakeHost::start().await;
    let mut target = paired_target(&host);
    target.values.insert("resolution".into(), "1920x1080".into());
    target.values.insert("bitrate".into(), "20".into());
    let connector = SunshineConnector::with_no_input(true);
    let phone = Viewer { codecs: vec![Codec::H264], fps: 30, width: 1280, height: 720, bitrate_kbps: 6_000 };
    let mut session = connector.connect(&target, &phone).await.expect("a session");
    assert_eq!(next_event(&mut session).await, Event::CursorAlwaysEmbedded);
    // The display the viewer's pointer positions are scaled to is the stream's.
    match next_event(&mut session).await {
        Event::Displays { displays, current: 0 } => assert_eq!((displays[0].width, displays[0].height), (1280, 720)),
        other => panic!("unexpected {other:?}"),
    }
    {
        let st = host.state.lock().unwrap();
        assert_eq!(st.launch_args.as_ref().unwrap()["mode"], "1280x720x30");
        let config = &st.stream.session.as_ref().unwrap().config;
        assert_eq!((config.width, config.height, config.framerate), (1280, 720, 30));
        assert_eq!(config.configured_bitrate_kbps, 6_000);
    }
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
}

#[test]
fn application_titles_of_the_host_are_shown_tidy_and_short() {
    let mut apps: Vec<App> = (0..300).map(|i| App { id: i, title: format!("App {i}"), hdr_supported: false }).collect();
    apps[0].title = format!("Evil\n\u{1b}[31m{}", "x".repeat(5000));
    let text = find_app(&apps, "Notepad\r\nmore").unwrap_err().to_string();
    assert!(!text.chars().any(|c| c.is_control()), "one line, printable");
    assert!(text.len() < 2500, "{} bytes", text.len());
    assert!(text.contains("and 280 more"));
    assert!(text.contains("\"Evil"));
}

#[test]
fn only_the_hosts_own_port_and_address_count_as_the_host() {
    let host: IpAddr = "192.168.1.5".parse().unwrap();
    let from = |text: &str| -> SocketAddr { text.parse().unwrap() };
    // Another port is never the host's, whatever the address.
    let mut origin = Origin::new(host, 47998);
    assert!(!origin.takes(&from("192.168.1.5:47999")));
    assert!(!origin.takes(&from("192.168.1.9:50000")));
    assert!(origin.takes(&from("192.168.1.5:47998")));
    assert!(origin.takes(&from("[::ffff:192.168.1.5]:47998")));
    // Once the host was heard from its own address, nobody else is taken.
    assert!(!origin.takes(&from("192.168.1.6:47998")));
    // A host that answers from another of its addresses (its routes choose):
    // the first one that sends from the right port, and only that one...
    let mut origin = Origin::new(host, 47998);
    assert!(origin.takes(&from("10.8.0.5:47998")));
    assert!(origin.takes(&from("10.8.0.5:47998")));
    assert!(!origin.takes(&from("10.8.0.6:47998")));
    // ...and the address that was asked always counts, and then alone.
    assert!(origin.takes(&from("192.168.1.5:47998")));
    assert!(!origin.takes(&from("10.8.0.5:47998")));
}

#[tokio::test]
async fn video_from_somebody_else_is_not_shown() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(true);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    let mut rng = Lcg(21);
    // A whole, well-formed key frame of this very session, but sent from
    // another socket than the host's video port.
    let forged = make_frame(VideoCodec::H264, true, 4_000, &mut rng);
    let (datagrams, peer) = {
        let mut st = host.state.lock().unwrap();
        let s = st.stream.session.as_mut().unwrap();
        (s.video.send(&forged, true).all(), s.video_peer.unwrap())
    };
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    for datagram in &datagrams {
        stranger.send_to(datagram, peer).await.unwrap();
    }
    assert!(tokio::time::timeout(Duration::from_millis(300), session.events.recv()).await.is_err(), "nothing is shown");
    // The host's own next key frame is.
    let real = make_frame(VideoCodec::H264, true, 4_000, &mut rng);
    host.send_video(&real, true, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H264, true, Bytes::from(real)));
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
}

#[tokio::test]
async fn lost_sound_is_stood_in_for_so_that_the_clock_keeps_step() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(true);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    // Past what is dropped at the start, and one whole block to get in step.
    host.send_audio_block(1, 96).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    host.send_audio_block(2, 96).await;
    host.send_audio_block(3, 96).await;
    // A block of which only its last packet arrives (three of four packets
    // and both parity packets lost: beyond recovery), then a whole one.
    let (lone, peer) = {
        let mut st = host.state.lock().unwrap();
        let s = st.stream.session.as_mut().unwrap();
        let mut all = Vec::new();
        for i in 0..4u8 {
            let mut opus = vec![i; 96];
            opus[0] = 0xFC;
            all.extend(s.audio.send(&opus));
        }
        (all[3].clone(), s.audio_peer.unwrap())
    };
    host.audio_socket.send_to(&lone, peer).await.unwrap();
    let after = host.send_audio_block(9, 96).await;
    let mut heard: Vec<Vec<u8>> = Vec::new();
    while !heard.ends_with(&after) {
        match next_event(&mut session).await {
            Event::Audio { data, .. } => heard.push(data.to_vec()),
            other => panic!("unexpected {other:?}"),
        }
    }
    // Before the whole block: three packets of one byte (the stream's kind
    // of Opus packet with an empty frame: "conceal this one"), each as long
    // in time as a real one, and the one packet that did arrive.
    let n = heard.len();
    let block = &heard[n - 8..n - 4];
    assert_eq!(block[..3], [vec![0xFC], vec![0xFC], vec![0xFC]]);
    assert_eq!(block[3].len(), 96);
    assert_eq!(crate::connector::media::opus_duration(&[0xFC]), crate::connector::media::opus_duration(&after[0]));
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
}

#[tokio::test]
async fn the_viewers_own_pointer_takes_the_hosts_out_of_the_picture_and_puts_it_back() {
    let host = FakeHost::start().await;
    let mut target = paired_target(&host);
    target.values.insert("pointer".into(), "own".into());
    let connector = SunshineConnector::with_no_input(false);
    let mut session = connector.connect(&target, &viewer(&[Codec::H264])).await.expect("a session");
    // Not "the pointer is always in the picture": the viewer shows its own.
    assert!(matches!(next_event(&mut session).await, Event::Displays { .. }));
    host.wait("the UDP pings", both_pings).await;
    // Nothing is said to the host's pointer before the host is seen to send.
    assert!(host.state.lock().unwrap().stream.display_cursor);
    let mut rng = Lcg(5);
    let key_frame = make_frame(VideoCodec::H264, true, 3_000, &mut rng);
    host.send_video(&key_frame, true, &[]).await;
    next_video(&mut session).await;
    host.wait("Sunshine's shortcut, once", |st| st.stream.cursor_toggles == 1 && st.stream.input.len() == 8).await;
    {
        let st = host.state.lock().unwrap();
        assert!(!st.stream.display_cursor, "the host draws no pointer into its picture");
        // Ctrl, Alt, Shift, N down; N, Shift, Alt, Ctrl up; and no key is left held.
        let keys: Vec<(u8, bool)> = st.stream.input.iter().map(|i| match i {
            InputSeen::Key { vk, release, .. } => (*vk, *release),
            other => panic!("unexpected {other:?}"),
        }).collect();
        assert_eq!(keys, [(0xA2, false), (0xA4, false), (0xA0, false), (0x4E, false), (0x4E, true), (0xA0, true), (0xA4, true), (0xA2, true)]);
        assert_eq!(st.stream.shortcut_flags, 0);
    }
    // The end puts it back, for whoever looks at this host next.
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
    {
        let st = host.state.lock().unwrap();
        assert!(st.stream.display_cursor);
        assert_eq!((st.stream.cursor_toggles, st.cancels), (2, 1));
    }

    // With the test switch that keeps all input back, nothing can be said to
    // the host's pointer: it stays in the picture and the viewer is told so.
    let silent = SunshineConnector::with_no_input(true);
    let mut session = connected(&host, &silent, &target, &viewer(&[Codec::H264])).await;
    host.send_video(&key_frame, true, &[]).await;
    next_video(&mut session).await;
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
    assert_eq!(host.state.lock().unwrap().stream.cursor_toggles, 2);

    assert_eq!(
        connector.connect(&{ let mut t = target.clone(); t.values.insert("pointer".into(), "both".into()); t }, &viewer(&[Codec::H264])).await.err().unwrap().to_string(),
        "\"both\" is not a choice for the pointer. Write picture or own."
    );
}

#[tokio::test]
async fn a_pointer_left_out_when_the_connection_broke_is_put_back_by_the_next_session() {
    let host = FakeHost::start().await;
    let mut own = paired_target(&host);
    own.values.insert("pointer".into(), "own".into());
    let connector = SunshineConnector::with_no_input(false);
    let mut session = connector.connect(&own, &viewer(&[Codec::H264])).await.expect("a session");
    host.wait("the UDP pings", both_pings).await;
    let mut rng = Lcg(6);
    let key_frame = make_frame(VideoCodec::H264, true, 3_000, &mut rng);
    host.send_video(&key_frame, true, &[]).await;
    host.wait("the pointer out of the picture", |st| !st.stream.display_cursor).await;
    // The host ends the session itself: nothing more can be said to it on
    // the control stream, the pointer stays out.
    host.send_control(0x0109, &0x80030023u32.to_be_bytes());
    loop {
        if let Event::Closed { .. } = next_event(&mut session).await {
            break;
        }
    }
    while session.events.recv().await.is_some() {}
    assert!(!host.state.lock().unwrap().stream.display_cursor);

    // The next session, of a computer set to the pointer in the picture
    // (the same host), puts it back first.
    let mut picture = own.clone();
    picture.values.insert("pointer".into(), "picture".into());
    let mut session = connected(&host, &connector, &picture, &viewer(&[Codec::H264])).await;
    host.send_video(&key_frame, true, &[]).await;
    next_video(&mut session).await;
    host.wait("the pointer back in the picture", |st| st.stream.display_cursor && st.stream.cursor_toggles == 2).await;
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
    assert_eq!(host.state.lock().unwrap().stream.cursor_toggles, 2);
}

#[tokio::test]
async fn the_second_connect_asks_nothing_over_plain_http() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(true);
    for round in 0..2 {
        let before = host.state.lock().unwrap().requests.len();
        let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
        let mut requests = host.state.lock().unwrap().requests[before..].to_vec();
        requests.sort();
        if round == 0 {
            // The HTTPS port is asked for once (libgamestream load_server_status).
            assert_eq!(requests, ["http /serverinfo", "https /applist", "https /launch", "https /serverinfo"]);
        } else {
            assert_eq!(requests, ["https /applist", "https /launch", "https /serverinfo"]);
        }
        session.commands.send(Command::Close).await.unwrap();
        assert_eq!(closed(&mut session).await, "The session was closed");
    }
}

#[tokio::test]
async fn another_certificate_at_the_address_is_not_called_unpaired() {
    let host = FakeHost::start().await;
    let mut target = paired_target(&host);
    target.values.insert("server_cert".into(), super::super::http::tests::OPENSSL_CERT.to_string());
    let connector = SunshineConnector::with_no_input(true);
    let error = connector.connect(&target, &viewer(&[Codec::H264])).await.err().expect("no session").to_string();
    assert!(error.contains("is not the one that was paired"), "{error}");
    assert!(!error.contains("does not accept the pairing"));
    // Nothing was launched, and nothing but serverinfo was asked.
    let st = host.state.lock().unwrap();
    assert_eq!((st.running_app, st.cancels), (0, 0));
    assert!(st.requests.iter().all(|r| r.ends_with("/serverinfo")), "{:?}", st.requests);
}

#[tokio::test]
async fn the_programs_end_ends_what_was_started_at_the_host() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(true);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H264])).await;
    assert_eq!(host.state.lock().unwrap().running_app, DESKTOP_ID);
    // What the signal's handler does, for this test's host only (the
    // signal itself is sent in `a_signal_that_ends_the_program_...`).
    assert_eq!(end_at_exit(Some(&format!("127.0.0.1:{}", host.http_port))).await, 1);
    {
        let st = host.state.lock().unwrap();
        assert_eq!((st.cancels, st.running_app), (1, 0));
    }
    // Not a second time when the session then ends.
    session.commands.send(Command::Close).await.unwrap();
    loop {
        match tokio::time::timeout(WAIT, session.events.recv()).await.unwrap() {
            None => break,
            Some(_) => {}
        }
    }
    assert_eq!(host.state.lock().unwrap().cancels, 1);
}

/// The other half of the next test: run by it as a program of its own,
/// which connects to the host named in the environment and then waits to
/// be ended. Does nothing when run by itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn exit_child() {
    let Ok(port) = std::env::var("SUNSHINE_EXIT_CHILD_PORT") else { return };
    let mut values = BTreeMap::new();
    for name in ["client_cert", "client_key", "server_cert", "unique_id"] {
        values.insert(name.to_string(), std::env::var(format!("SUNSHINE_EXIT_CHILD_{}", name.to_uppercase())).unwrap());
    }
    values.insert("host".into(), "127.0.0.1".into());
    values.insert("port".into(), port);
    values.insert("resolution".into(), "1280x720".into());
    let target = Target { connector: "sunshine".into(), values };
    let connector = SunshineConnector::with_no_input(true);
    let _session = connector.connect(&target, &viewer(&[Codec::H264])).await.expect("a session");
    tokio::time::sleep(Duration::from_secs(60)).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_that_ends_the_program_ends_the_application_at_the_host_first() {
    use std::os::unix::process::ExitStatusExt;
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child.args(["--ignored", "--exact", "connector::sunshine::session::tests::exit_child", "--nocapture"]);
    child.env("SUNSHINE_EXIT_CHILD_PORT", host.http_port.to_string());
    for name in ["client_cert", "client_key", "server_cert", "unique_id"] {
        child.env(format!("SUNSHINE_EXIT_CHILD_{}", name.to_uppercase()), target.get(name));
    }
    child.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = child.spawn().expect("the test program as a child");
    host.wait("the child's session", |st| st.running_app == DESKTOP_ID && both_pings(st)).await;
    assert_eq!(host.state.lock().unwrap().cancels, 0);
    // SAFETY: a signal to the child this test started.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let status = tokio::task::spawn_blocking(move || child.wait().unwrap()).await.unwrap();
    // The program ended by the signal, as it would have without the hook...
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    // ...and the host is free: the application was ended first.
    let st = host.state.lock().unwrap();
    assert_eq!((st.cancels, st.running_app), (1, 0));
}

#[test]
fn the_application_is_ended_with_the_hosts_last_session() {
    let key = "counting.test:1";
    let identity = super::super::http::tests::test_identity();
    let client = Arc::new(HostClient::new("counting.test", 1, identity, None).unwrap());
    // One session started the application, a second joined it.
    with_host(key, |state| {
        state.sessions = 2;
        state.launches = 1;
        state.client = Some(client.clone());
    });
    assert!(leave_host(key).is_none(), "one session is still there");
    assert!(leave_host(key).is_some(), "the last one ends the application");
    // Sessions that only joined what somebody else started leave it running.
    with_host(key, |state| {
        state.sessions = 1;
        state.client = Some(client.clone());
    });
    assert!(leave_host(key).is_none());
    assert!(with_host(key, |state| state.sessions == 0 && state.launches == 0 && state.client.is_none()));
}

/// Found in real use (a Windows host over a tunnel): every key frame the
/// viewer asked for in the middle of a session was followed by 7 to 17
/// seconds of a standing picture. The host's key frame (a burst of some 150
/// datagrams) was lost on the way; the receiver saw that at the next
/// picture and asked again, but its request was thrown away for coming
/// within 200 ms of the viewer's, and it asked again only after 120 dropped
/// pictures (4.2 s at the 28.5 pictures a second of that session), each time
/// with the same fate when that key frame was lost too.
#[tokio::test]
async fn a_key_frame_lost_right_after_the_viewer_asked_is_asked_for_again_at_once() {
    let host = FakeHost::start().await;
    let target = paired_target(&host);
    let connector = SunshineConnector::with_no_input(true);
    let mut session = connected(&host, &connector, &target, &viewer(&[Codec::H265])).await;
    let mut rng = Lcg(77);
    let codec = VideoCodec::H265;
    let first = make_frame(codec, true, 30_000, &mut rng);
    host.send_video(&first, true, &[]).await;
    assert_eq!(next_video(&mut session).await.1, true);
    let asked_before = host.state.lock().unwrap().stream.idr_requests;

    // The viewer asks (a PLI): the host answers with a key frame as large as
    // the ones of that session, 130 kB, and too much of it is lost.
    session.commands.send(Command::RequestKeyFrame).await.unwrap();
    host.wait("the viewer's request", |st| st.stream.idr_requests == asked_before + 1).await;
    let big = make_frame(codec, true, 130_000, &mut rng);
    let lost: Vec<usize> = (40..80).collect();
    let datagrams = host.send_video(&big, true, &lost).await;
    assert!(datagrams > 100 && datagrams < 212, "one FEC block of {datagrams} datagrams");
    // The next picture shows the gap: the receiver asks, a few milliseconds
    // after the viewer did, and the request reaches the host without waiting
    // for 120 pictures to be dropped.
    let asked = std::time::Instant::now();
    let after = make_frame(codec, false, 19_000, &mut rng);
    host.send_video(&after, false, &[]).await;
    host.wait("the receiver's own request", |st| st.stream.idr_requests == asked_before + 2).await;
    assert!(asked.elapsed() < Duration::from_millis(1000), "asked again after {:?}", asked.elapsed());
    // That key frame arrives, and the picture goes on with it.
    let again = make_frame(codec, true, 130_000, &mut rng);
    host.send_video(&again, true, &[]).await;
    assert_eq!(next_video(&mut session).await, (Codec::H265, true, Bytes::from(again)));
    session.commands.send(Command::Close).await.unwrap();
    assert_eq!(closed(&mut session).await, "The session was closed");
}

/// Not a check: prints how long connecting takes against the host in this
/// process (no network in between: what is left is this side's work and the
/// number of requests), and how long ending a session takes. Run with
/// `--ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_connect_and_end() {
    let host = FakeHost::start().await;
    let connector = SunshineConnector::with_no_input(true);
    let target = paired_target(&host);
    let mut rng = Lcg(3);
    let key_frame = make_frame(VideoCodec::H264, true, 30_000, &mut rng);
    let (mut connects, mut firsts, mut ends) = (Vec::new(), Vec::new(), Vec::new());
    let mut requests = 0usize;
    for round in 0..20 {
        let before = host.state.lock().unwrap().requests.len();
        let started = std::time::Instant::now();
        let mut session = connector.connect(&target, &viewer(&[Codec::H264])).await.expect("a session");
        connects.push(started.elapsed());
        host.wait("the UDP pings", both_pings).await;
        host.send_video(&key_frame, true, &[]).await;
        loop {
            if let Event::Video { .. } = next_event(&mut session).await {
                break;
            }
        }
        firsts.push(started.elapsed());
        requests = host.state.lock().unwrap().requests.len() - before;
        let ending = std::time::Instant::now();
        session.commands.send(Command::Close).await.unwrap();
        loop {
            match tokio::time::timeout(WAIT, session.events.recv()).await.unwrap() {
                Some(Event::Closed { .. }) | None => break,
                Some(_) => {}
            }
        }
        host.wait("the host free again", |st| st.running_app == 0 && st.cancels as usize == round + 1).await;
        ends.push(ending.elapsed());
    }
    let ms = |all: &mut Vec<Duration>| {
        all.sort();
        (all[all.len() / 2].as_secs_f64() * 1e3, all[all.len() - 1].as_secs_f64() * 1e3)
    };
    let (c, f, e) = (ms(&mut connects), ms(&mut firsts), ms(&mut ends));
    println!(
        "BENCH connect: median {:.1} ms (max {:.1}); with the first picture {:.1} ms (max {:.1}); end until the host is free {:.1} ms (max {:.1}); HTTP requests for one connect: {requests}",
        c.0, c.1, f.0, f.1, e.0, e.1
    );
}

/// Not a check of this program alone: one short session with a real
/// Sunshine, receiving only. Run by hand with the path of an identity file
/// (`host`, `port`, `client_cert`, `client_key`, `server_cert`, `unique_id`)
/// in SUNSHINE_LIVE_IDENTITY, `--ignored --nocapture`. No input of any kind
/// is sent (the connector is made with the switch that keeps it back, and
/// the pointer is left as the host has it); the application is ended at the
/// end. SUNSHINE_LIVE_SECONDS (12), SUNSHINE_LIVE_SIZE (WxH), SUNSHINE_LIVE_KBPS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn live_receive_only() {
    struct Lines;
    impl log::Log for Lines {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            // The receivers' own lines (a block given up, a key frame needed) too.
            metadata.target().contains("sunshine")
                && (metadata.level() <= log::Level::Info || metadata.target().ends_with("video") || metadata.target().ends_with("session"))
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                println!("LOG {}", record.args());
            }
        }
        fn flush(&self) {}
    }
    static LINES: Lines = Lines;
    let Ok(path) = std::env::var("SUNSHINE_LIVE_IDENTITY") else { return };
    let _ = log::set_logger(&LINES);
    log::set_max_level(log::LevelFilter::Debug);
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("the identity file")).expect("JSON");
    let mut values = BTreeMap::new();
    for key in ["host", "port", "client_cert", "client_key", "server_cert", "unique_id"] {
        values.insert(key.to_string(), json[key].as_str().unwrap_or_default().to_string());
    }
    values.insert("pointer".into(), "picture".into());
    let target = Target { connector: "sunshine".into(), values };
    let seconds: u64 = std::env::var("SUNSHINE_LIVE_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let (width, height) = std::env::var("SUNSHINE_LIVE_SIZE").ok().and_then(|v| parse_resolution(&v)).unwrap_or((0, 0));
    let bitrate_kbps = std::env::var("SUNSHINE_LIVE_KBPS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let viewer = Viewer { codecs: vec![Codec::H264], fps: 60, width, height, bitrate_kbps };
    let connector = SunshineConnector::with_no_input(true);

    let started = std::time::Instant::now();
    let mut session = match connector.connect(&target, &viewer).await {
        Ok(session) => session,
        Err(e) => {
            println!("LIVE connect failed after {} ms: {e}", started.elapsed().as_millis());
            return;
        }
    };
    println!("LIVE connect: {} ms", started.elapsed().as_millis());
    let (mut frames, mut keys, mut bytes, mut sound, mut stood_in) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut first_picture, mut first_sound) = (None, None);
    let mut gaps: Vec<f64> = Vec::new();
    let mut last = None;
    let until = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let mut reason = None;
    // SUNSHINE_LIVE_IDR_MS: a key frame is asked for this often (a control
    // message, as a viewer's PLI makes it; not input).
    let idr_every = std::env::var("SUNSHINE_LIVE_IDR_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms >= 500);
    let mut idr_tick = tokio::time::interval(Duration::from_millis(idr_every.unwrap_or(3_600_000)));
    idr_tick.tick().await;
    let mut asked: Option<std::time::Instant> = None;
    loop {
        let event = tokio::select! {
            event = tokio::time::timeout_at(until, session.events.recv()) => match event {
                Ok(event) => event,
                Err(_) => break,
            },
            _ = idr_tick.tick(), if idr_every.is_some() => {
                if let Some(at) = asked {
                    println!("LIVE key frame asked {} ms ago has not come; asking again", at.elapsed().as_millis());
                }
                asked = Some(std::time::Instant::now());
                let _ = session.commands.send(Command::RequestKeyFrame).await;
                continue;
            }
        };
        match event {
            Some(Event::Video { key, data, .. }) => {
                let now = std::time::Instant::now();
                if key {
                    match asked.take() {
                        Some(at) => println!("LIVE key frame {} ms after it was asked for, {} bytes; longest gap between pictures so far {:.0} ms", at.elapsed().as_millis(), data.len(), gaps.iter().cloned().fold(0.0, f64::max)),
                        None => println!("LIVE key frame nobody here asked for, {} bytes", data.len()),
                    }
                }
                first_picture.get_or_insert(started.elapsed());
                if let Some(at) = last.replace(now) {
                    gaps.push(now.duration_since(at).as_secs_f64() * 1e3);
                }
                frames += 1;
                keys += key as u64;
                bytes += data.len() as u64;
            }
            Some(Event::Audio { data, .. }) => {
                first_sound.get_or_insert(started.elapsed());
                sound += 1;
                stood_in += (data.len() == 1) as u64;
            }
            Some(Event::Closed { reason: why }) => {
                reason = Some(why);
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    let ending = std::time::Instant::now();
    let _ = session.commands.send(Command::Close).await;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(20), session.events.recv()).await {
        if let Event::Closed { reason: why } = event {
            reason.get_or_insert(why);
        }
    }
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |q: f64| gaps.get(((gaps.len() as f64 - 1.0) * q) as usize).copied().unwrap_or(0.0);
    println!(
        "LIVE first picture {:?} ms, first sound {:?} ms; {frames} pictures ({keys} key), {:.2} Mbit/s; between pictures median {:.1} ms, p99 {:.1} ms, longest {:.1} ms; sound {sound} packets, {stood_in} of them stand-ins for lost ones; ended ({}) in {} ms",
        first_picture.map(|d| d.as_millis()),
        first_sound.map(|d| d.as_millis()),
        bytes as f64 * 8.0 / seconds as f64 / 1e6,
        at(0.5),
        at(0.99),
        at(1.0),
        reason.unwrap_or_default(),
        ending.elapsed().as_millis(),
    );
}
