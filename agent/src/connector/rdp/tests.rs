//! The RDP connector against the fake server (fake_server.rs): the set-up
//! pins the certificate, a session connects with NLA or TLS, the picture,
//! the pointer and the input go where they should, the end lets go of
//! everything; wrong passwords and changed certificates fail clearly.

use std::collections::BTreeMap;
use std::time::Duration;

use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::input::mouse::PointerFlags;
use ironrdp_pdu::input::mouse_x::PointerXFlags;
use ironrdp_pdu::nego::SecurityProtocol;
use tokio::sync::mpsc;

use super::fake_server::{Config, FakeRdp, Out};
use super::{tls, RdpConnector, PIN_KEY};
use crate::connector::raw_video::EncoderChoice;
use crate::connector::{Button, Codec, Command, Connector, DisplayInfo, Event, Session, Target, Viewer};

fn connector() -> RdpConnector {
    RdpConnector { encoders: EncoderChoice::Software }
}

fn target(port: u16, password: &str, security: &str) -> Target {
    let mut values = BTreeMap::new();
    values.insert("host".to_string(), "127.0.0.1".to_string());
    values.insert("port".to_string(), port.to_string());
    values.insert("username".to_string(), "alice".to_string());
    values.insert("password".to_string(), password.to_string());
    values.insert("security".to_string(), security.to_string());
    values.insert("resolution".to_string(), "auto".to_string());
    Target { connector: "rdp".into(), values }
}

fn viewer() -> Viewer {
    Viewer { codecs: vec![Codec::H264], fps: 30, width: 640, height: 480, bitrate_kbps: 2000 }
}

fn config(security: SecurityProtocol, connections: usize) -> Config {
    Config { security, username: "alice".into(), password: "pw-1".into(), size: (640, 480), connections, clipboard: true }
}

async fn next(session: &mut Session) -> Event {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(15), session.events.recv()).await.expect("an event in time").expect("the session is open");
        if !matches!(event, Event::Latency { .. }) {
            return event;
        }
    }
}

/// The next event that is not a picture.
async fn next_other(session: &mut Session) -> Event {
    loop {
        match next(session).await {
            Event::Video { .. } => {}
            other => return other,
        }
    }
}

/// The set-up against `server`: the target with what it returned.
async fn set_up(server: &FakeRdp, password: &str, security: &str) -> Target {
    let mut target = target(server.port, password, security);
    let (steps_tx, mut steps) = mpsc::channel(4);
    let kept = connector().setup(&target, steps_tx).await.unwrap();
    assert_eq!(steps.recv().await.unwrap().message, super::SETUP_MESSAGE);
    assert_eq!(kept.get(PIN_KEY), Some(&tls::fingerprint(&server.cert_der)));
    target.values.extend(kept);
    target
}

#[test]
fn description() {
    let d = connector().describe();
    assert_eq!(d.id, "rdp");
    let fields: Vec<(&str, &str, &str, bool)> = d.fields.iter().map(|f| (f.key.as_str(), f.kind.as_str(), f.default.as_str(), f.required)).collect();
    assert_eq!(
        fields,
        vec![
            ("host", "text", "", true),
            ("port", "port", "3389", false),
            ("username", "text", "", false),
            ("password", "secret", "", false),
            ("domain", "text", "", false),
            ("security", "choice", "nla", false),
            ("resolution", "text", "auto", false),
        ]
    );
    assert!(d.capabilities.setup && d.capabilities.input && d.capabilities.clipboard && d.capabilities.cursor);
    assert!(crate::connector::builtin().iter().any(|c| c.describe().id == "rdp"));
}

#[test]
fn sizes() {
    use super::session::{parse_size, Options};
    assert_eq!(parse_size("1920x1080"), Some((1920, 1080)));
    assert_eq!(parse_size(" 1280 X 720 "), Some((1280, 720)));
    assert_eq!(parse_size("100x100"), None);
    let mut t = target(1, "", "nla");
    let options = Options::of(&t).unwrap();
    // The viewer's size, fitted (an odd width becomes even).
    let v = Viewer { width: 1365, height: 767, ..viewer() };
    let s = options.desktop_size(&v);
    assert_eq!((s.width, s.height), (1364, 767));
    // No wish: 1920x1080.
    let s = options.desktop_size(&Viewer { width: 0, height: 0, ..viewer() });
    assert_eq!((s.width, s.height), (1920, 1080));
    t.values.insert("resolution".into(), "1600x900".into());
    let s = Options::of(&t).unwrap().desktop_size(&v);
    assert_eq!((s.width, s.height), (1600, 900));
    t.values.insert("resolution".into(), "big".into());
    assert!(Options::of(&t).is_err());
    t.values.insert("security".into(), "rc4".into());
    assert!(Options::of(&t).is_err());
}

#[tokio::test]
async fn a_whole_session_with_nla() {
    let server = FakeRdp::start(config(SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX, 2)).await;
    let target = set_up(&server, "pw-1", "nla").await;
    let mut session = connector().connect(&target, &viewer()).await.unwrap();
    server.wait("the session", |log| log.accepted).await;

    // The display (the size asked for: the viewer's), then a key frame.
    let (mut displays, mut key) = (false, false);
    while !(displays && key) {
        match next(&mut session).await {
            Event::Displays { displays: d, current: 0 } => {
                assert_eq!(d, vec![DisplayInfo { name: "Display".into(), width: 640, height: 480 }]);
                displays = true;
            }
            Event::Video { codec: Codec::H264, key: true, .. } => key = true,
            other => panic!("{other:?}"),
        }
    }

    // A bitmap (planar, several tiles): a new picture follows.
    let bgrx: Vec<u8> = (0..130 * 70).flat_map(|i| [(i % 251) as u8, (i % 13) as u8 * 19, 200, 0]).collect();
    server.out.send(Out::Bitmap { x: 10, y: 20, w: 130, h: 70, bgrx }).unwrap();
    loop {
        if let Event::Video { key: false, .. } = next(&mut session).await {
            break;
        }
    }

    // The pointer: a shape, a position, hidden, the default one.
    let rgba: Vec<u8> = (0..4 * 4).flat_map(|i| if i % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 0, 0] }).collect();
    server.out.send(Out::Pointer { w: 4, h: 4, hot_x: 1, hot_y: 2, rgba }).unwrap();
    match next_other(&mut session).await {
        Event::CursorShape { width: 4, height: 4, hot_x: 1, hot_y: 2, rgba } => {
            assert_eq!(rgba.len(), 64);
            // Half the pixels shown, half transparent, as sent.
            assert_eq!(rgba.chunks(4).filter(|p| p[3] == 255).count(), 8);
            assert_eq!(rgba.chunks(4).filter(|p| p[3] == 0).count(), 8);
        }
        other => panic!("{other:?}"),
    }
    server.out.send(Out::PointerPosition { x: 33, y: 44 }).unwrap();
    assert_eq!(next_other(&mut session).await, Event::CursorPosition { x: 33, y: 44, visible: true });
    server.out.send(Out::HidePointer).unwrap();
    assert!(matches!(next_other(&mut session).await, Event::CursorPosition { visible: false, .. }));
    server.out.send(Out::DefaultPointer).unwrap();
    assert!(matches!(next_other(&mut session).await, Event::CursorShape { width: 11, height: 16, hot_x: 0, hot_y: 0, .. }));
    assert!(matches!(next_other(&mut session).await, Event::CursorPosition { visible: true, .. }));

    // Input.
    for command in [
        Command::Key { position: 30, down: true },
        Command::Key { position: 30, down: false },
        Command::Key { position: 103, down: true },
        Command::Key { position: 103, down: false },
        Command::Text { text: "é".into() },
        Command::PointerMove { x: 100, y: 50 },
        Command::PointerButton { button: Button::Left, down: true },
        Command::PointerButton { button: Button::Left, down: false },
        Command::PointerButton { button: Button::Back, down: true },
        Command::PointerButton { button: Button::Back, down: false },
        Command::Wheel { dx: 0, dy: 1 },
        Command::Wheel { dx: 0, dy: -1 },
        Command::Wheel { dx: 1, dy: 0 },
        Command::PointerMove { x: 9000, y: -5 },
        Command::Key { position: 29, down: true },
    ] {
        session.commands.send(command).await.unwrap();
    }
    server.wait("the input", |log| log.input.len() >= 15).await;
    let input = server.log.lock().unwrap().input.clone();
    let mouse = |flags: PointerFlags, units: i16, x: u16, y: u16| {
        FastPathInputEvent::MouseEvent(ironrdp_pdu::input::MousePdu { flags, number_of_wheel_rotation_units: units, x_position: x, y_position: y })
    };
    assert_eq!(
        input,
        vec![
            FastPathInputEvent::KeyboardEvent(KeyboardFlags::empty(), 0x1e),
            FastPathInputEvent::KeyboardEvent(KeyboardFlags::RELEASE, 0x1e),
            FastPathInputEvent::KeyboardEvent(KeyboardFlags::EXTENDED, 0x48),
            FastPathInputEvent::KeyboardEvent(KeyboardFlags::RELEASE | KeyboardFlags::EXTENDED, 0x48),
            FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::empty(), 0xe9),
            FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::RELEASE, 0xe9),
            mouse(PointerFlags::MOVE, 0, 100, 50),
            mouse(PointerFlags::DOWN | PointerFlags::LEFT_BUTTON, 0, 100, 50),
            mouse(PointerFlags::LEFT_BUTTON, 0, 100, 50),
            FastPathInputEvent::MouseEventEx(ironrdp_pdu::input::MouseXPdu { flags: PointerXFlags::DOWN | PointerXFlags::BUTTON1, x_position: 100, y_position: 50 }),
            FastPathInputEvent::MouseEventEx(ironrdp_pdu::input::MouseXPdu { flags: PointerXFlags::BUTTON1, x_position: 100, y_position: 50 }),
            // MS-RDPBCGR 2.2.8.1.1.3.1.1.3: the rotation is in the low 9
            // bits of the flags (WHEEL_NEGATIVE the sign), as decoded.
            mouse(PointerFlags::VERTICAL_WHEEL | PointerFlags::from_bits_retain(0x78), 120, 100, 50),
            mouse(PointerFlags::VERTICAL_WHEEL | PointerFlags::WHEEL_NEGATIVE | PointerFlags::from_bits_retain(0x88), -120, 100, 50),
            mouse(PointerFlags::HORIZONTAL_WHEEL | PointerFlags::from_bits_retain(0x78), 120, 100, 50),
            mouse(PointerFlags::MOVE, 0, 639, 0),
            FastPathInputEvent::KeyboardEvent(KeyboardFlags::empty(), 0x1d),
        ]
    );

    // Clipboard text both ways (CLIPRDR, CF_UNICODETEXT).
    session.commands.send(Command::Clipboard { text: "Tiếng Việt ✓".into() }).await.unwrap();
    server.wait("the client's clipboard text", |log| log.clipboard_from_client.is_some()).await;
    assert_eq!(server.log.lock().unwrap().clipboard_from_client.as_deref(), Some("Tiếng Việt ✓"));
    server.out.send(Out::Clipboard("xin chào".into())).unwrap();
    assert_eq!(next_other(&mut session).await, Event::Clipboard { text: "xin chào".into() });

    // The end: the held key is let go, then the Shutdown Request.
    session.commands.send(Command::Close).await.unwrap();
    loop {
        if let Event::Closed { reason } = next(&mut session).await {
            assert_eq!(reason, "Closed");
            break;
        }
    }
    server.wait("the shutdown request", |log| log.shutdown_requested).await;
    let last = server.log.lock().unwrap().input.last().cloned();
    assert_eq!(last, Some(FastPathInputEvent::KeyboardEvent(KeyboardFlags::RELEASE, 0x1d)));
}

#[tokio::test]
async fn tls_with_the_logon_of_the_server() {
    let server = FakeRdp::start(config(SecurityProtocol::SSL, 2)).await;
    let target = set_up(&server, "pw-1", "tls").await;
    let mut session = connector().connect(&target, &viewer()).await.unwrap();
    assert!(matches!(next(&mut session).await, Event::Displays { .. }));
    assert!(matches!(next(&mut session).await, Event::Video { key: true, .. }));
    drop(session);
}

#[tokio::test]
async fn a_wrong_password_is_said_so() {
    let server = FakeRdp::start(config(SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX, 2)).await;
    let target = set_up(&server, "wrong", "nla").await;
    let error = connector().connect(&target, &viewer()).await.err().unwrap().to_string();
    assert!(error.contains("password is not correct"), "{error}");
    assert!(!error.contains("wrong"), "{error}");
}

#[tokio::test]
async fn another_certificate_is_refused() {
    let server = FakeRdp::start(config(SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX, 1)).await;
    let mut target = target(server.port, "pw-1", "nla");
    target.values.insert(PIN_KEY.into(), "00".repeat(32));
    let error = connector().connect(&target, &viewer()).await.err().unwrap().to_string();
    assert!(error.contains("is not the one it had when it was added"), "{error}");
    assert!(error.contains(&tls::display_fingerprint(&tls::fingerprint(&server.cert_der))), "{error}");
    // Nothing was sent after TLS: no credentials reached the server.
    assert!(!server.log.lock().unwrap().accepted);
    assert_eq!(server.log.lock().unwrap().tls_connections, 0);
}

#[tokio::test]
async fn no_pinned_certificate_no_session() {
    let server = FakeRdp::start(config(SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX, 1)).await;
    let error = connector().connect(&target(server.port, "pw-1", "nla"), &viewer()).await.err().unwrap().to_string();
    assert!(error.contains("no pinned certificate"), "{error}");
}

/// Against a real RDP computer, for measuring by hand: connects to the
/// source `LUNARIS_RDP_LIVE_SOURCE` of the agent config `LUNARIS_RDP_LIVE`,
/// only watches for `LUNARIS_RDP_LIVE_SECS` seconds (default 15; no key,
/// pointer or clipboard is sent), then closes. Logs whether the graphics
/// pipeline was used and the frames that came. `LUNARIS_RDP_GFX=0` compares
/// with bitmap updates.
///
/// `cargo test -p agent --lib rdp::tests::live -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real RDP computer (LUNARIS_RDP_LIVE, LUNARIS_RDP_LIVE_SOURCE)"]
async fn live() {
    let _ = tracing_subscriber::fmt().with_env_filter("info").with_test_writer().try_init();
    let path = std::env::var("LUNARIS_RDP_LIVE").expect("LUNARIS_RDP_LIVE: an agent config");
    let id = std::env::var("LUNARIS_RDP_LIVE_SOURCE").expect("LUNARIS_RDP_LIVE_SOURCE: a source id");
    let secs: u64 = std::env::var("LUNARIS_RDP_LIVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
    let config = crate::pairing::load_config(&path).expect("config");
    let source = config.sources.into_iter().find(|s| s.id == id).expect("no such source");
    let viewer = Viewer { codecs: vec![Codec::H264], fps: 60, width: 1920, height: 1080, bitrate_kbps: 15_000 };
    let Session { mut events, commands } = RdpConnector::new().connect(&source.target, &viewer).await.expect("connect");
    let started = std::time::Instant::now();
    let end = started + Duration::from_secs(secs);
    let (mut frames, mut keys, mut bytes) = (0u64, 0u64, 0u64);
    let mut gaps: Vec<u128> = Vec::new();
    let mut last: Option<std::time::Instant> = None;
    loop {
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, events.recv()).await {
            Ok(Some(Event::Video { key, data, .. })) => {
                let now = std::time::Instant::now();
                if let Some(t) = last {
                    gaps.push(now.duration_since(t).as_millis());
                }
                last = Some(now);
                frames += 1;
                keys += key as u64;
                bytes += data.len() as u64;
            }
            Ok(Some(Event::Closed { reason })) => panic!("closed: {reason}"),
            Ok(Some(_)) => {}
            Ok(None) => panic!("the session ended"),
            Err(_) => break,
        }
    }
    let _ = commands.send(Command::Close).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    gaps.sort_unstable();
    let pct = |p: usize| gaps.get(gaps.len().saturating_sub(1) * p / 100).copied().unwrap_or(0);
    println!(
        "live RDP: {frames} frames ({keys} key) in {secs} s, {} KiB; gap ms median {} p95 {} max {}",
        bytes / 1024,
        pct(50),
        pct(95),
        gaps.last().copied().unwrap_or(0)
    );
}
