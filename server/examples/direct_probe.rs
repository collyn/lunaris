//! End-to-end check of an agent's direct connections: a minimal client that
//! connects to `ws://<host>:<port>/ws/client`, queries the agent, starts a
//! session, answers the WebRTC offer and counts the video RTP packets that
//! arrive. Not shipped; for testing:
//!
//!   cargo run --release -p server --example direct_probe -- <host:port> <password> [seconds] [codec] [chroma]
//!
//! Exits non-zero when a step fails.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{
    ClientMessage, RtcIceCandidate, RtcSdpType, RtcSessionDescription, ServerToClientMessage,
    SignalingMessage,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_HEVC};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn step(name: &str, ok: bool, detail: impl std::fmt::Display) -> Result<()> {
    println!("[{}] {} — {}", if ok { "PASS" } else { "FAIL" }, name, detail);
    if ok {
        Ok(())
    } else {
        Err(format!("{} failed", name).into())
    }
}

async fn recv_until<F: Fn(&SignalingMessage) -> bool>(
    rx: &mut mpsc::UnboundedReceiver<SignalingMessage>,
    timeout: Duration,
    wanted: F,
) -> Option<SignalingMessage> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(msg)) if wanted(&msg) => return Some(msg),
            Ok(Some(_)) => continue,
            _ => return None,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: direct_probe <host:port> <password> [seconds] [codec] [chroma]");
        std::process::exit(2);
    }
    let addr = &args[1];
    let password = &args[2];
    let seconds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let codec = args.get(4).cloned().unwrap_or_else(|| "h264".into());
    let chroma = args.get(5).cloned();

    // 1. A wrong password is refused before the WebSocket opens.
    let bad = tokio_tungstenite::connect_async(format!("ws://{}/ws/client?token=wrong-password", addr)).await;
    let refused = match &bad {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => resp.status().as_u16() == 401,
        _ => false,
    };
    step("wrong password refused", refused, match &bad {
        Err(e) => e.to_string(),
        Ok(_) => "connection was accepted".into(),
    })?;

    // 2. Connect with the password.
    let t_connect = Instant::now();
    let url = format!("ws://{}/ws/client?token={}", addr, urlencoding::encode(password));
    let (ws, _) = tokio_tungstenite::connect_async(url).await?;
    step("connect", true, format!("{} ms", t_connect.elapsed().as_millis()))?;
    let (mut ws_write, mut ws_read) = ws.split();

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<SignalingMessage>();
    tokio::spawn(async move {
        while let Some(sig) = out_rx.recv().await {
            let json = serde_json::to_string(&ClientMessage::Signaling(sig)).unwrap();
            if ws_write.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    });
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<SignalingMessage>();
    tokio::spawn(async move {
        while let Some(Ok(frame)) = ws_read.next().await {
            if let Message::Text(text) = frame {
                match serde_json::from_str::<ServerToClientMessage>(&text) {
                    Ok(ServerToClientMessage::Signaling(sig)) => {
                        let _ = in_tx.send(sig);
                    }
                    Err(e) => eprintln!("unparsable message: {} ({})", e, text),
                }
            }
        }
    });

    // 3. Queries the clients make before streaming.
    let host = "direct".to_string();
    out_tx.send(SignalingMessage::GetCapabilities { target_id: host.clone() })?;
    let caps = recv_until(&mut in_rx, Duration::from_secs(10), |m| matches!(m, SignalingMessage::CapabilitiesResponse { .. })).await;
    match &caps {
        Some(SignalingMessage::CapabilitiesResponse { target_id, displays, encoders, host_os, .. }) => step(
            "capabilities",
            !displays.is_empty(),
            format!("from agent {}: {} display(s), encoders {:?}, os {:?}", target_id, displays.len(), encoders, host_os),
        )?,
        _ => step("capabilities", false, "no CapabilitiesResponse within 10 s")?,
    }
    out_tx.send(SignalingMessage::GetAppList { target_id: host.clone() })?;
    let apps = recv_until(&mut in_rx, Duration::from_secs(5), |m| matches!(m, SignalingMessage::AppListResponse { .. })).await;
    step("app list", apps.is_some(), format!("{:?}", apps.map(|a| match a {
        SignalingMessage::AppListResponse { apps, .. } => apps.iter().map(|a| a.title.clone()).collect::<Vec<_>>(),
        _ => vec![],
    })))?;

    // PROBE_QUERY_ONLY=1: stop here, like a client that only looked at the
    // display list (used to check it doesn't end another client's session).
    if std::env::var("PROBE_QUERY_ONLY").is_ok() {
        println!("ALL PASSED (queries only)");
        return Ok(());
    }

    // 4. Session: offer from the agent, answer from us, ICE both ways.
    let mut media = MediaEngine::default();
    for (mime, fmtp, pt) in [
        (MIME_TYPE_H264, "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42002a", 96u8),
        (MIME_TYPE_H264, "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=f4002a", 97),
        (MIME_TYPE_HEVC, "", 98),
    ] {
        media.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: mime.to_string(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: fmtp.to_string(),
                    rtcp_feedback: vec![],
                },
                payload_type: pt,
                ..Default::default()
            },
            RTPCodecType::Video,
        )?;
    }
    media.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media)?;
    let api = APIBuilder::new().with_media_engine(media).with_interceptor_registry(registry).build();
    let pc = Arc::new(api.new_peer_connection(RTCConfiguration::default()).await?);

    let packets = Arc::new(AtomicUsize::new(0));
    let first_packet: Arc<std::sync::Mutex<Option<Instant>>> = Arc::new(std::sync::Mutex::new(None));
    let codec_seen: Arc<std::sync::Mutex<String>> = Arc::new(std::sync::Mutex::new(String::new()));
    {
        let packets = packets.clone();
        let first_packet = first_packet.clone();
        let codec_seen = codec_seen.clone();
        pc.on_track(Box::new(move |track, _, _| {
            let packets = packets.clone();
            let first_packet = first_packet.clone();
            let codec_seen = codec_seen.clone();
            Box::pin(async move {
                let c = track.codec();
                *codec_seen.lock().unwrap() = format!("{} {}", c.capability.mime_type, c.capability.sdp_fmtp_line);
                if track.kind() != RTPCodecType::Video {
                    return;
                }
                while let Ok((_pkt, _)) = track.read_rtp().await {
                    first_packet.lock().unwrap().get_or_insert_with(Instant::now);
                    packets.fetch_add(1, Ordering::Relaxed);
                }
            })
        }));
    }
    {
        let out_tx = out_tx.clone();
        let host = host.clone();
        pc.on_ice_candidate(Box::new(move |cand: Option<RTCIceCandidate>| {
            let out_tx = out_tx.clone();
            let host = host.clone();
            Box::pin(async move {
                if let Some(json) = cand.and_then(|c| c.to_json().ok()) {
                    let _ = out_tx.send(SignalingMessage::IceCandidate {
                        target_id: host,
                        candidate: RtcIceCandidate {
                            candidate: json.candidate,
                            sdp_mid: json.sdp_mid,
                            sdp_mline_index: json.sdp_mline_index,
                            username_fragment: json.username_fragment,
                        },
                    });
                }
            })
        }));
    }

    let t_session = Instant::now();
    out_tx.send(SignalingMessage::RequestSession {
        host_id: host.clone(),
        width: Some(1280),
        height: Some(720),
        fps: Some(30),
        bitrate: Some(5000),
        codec: Some(codec.clone()),
        app_id: Some(0),
        encoder: None,
        display_id: None,
        virtual_display: None,
        chroma: chroma.clone(),
    })?;
    let offer = recv_until(&mut in_rx, Duration::from_secs(15), |m| {
        matches!(m, SignalingMessage::Sdp { sdp, .. } if matches!(sdp.ty, RtcSdpType::Offer)) || matches!(m, SignalingMessage::Error { .. })
    })
    .await;
    let offer_sdp = match offer {
        Some(SignalingMessage::Sdp { sdp, .. }) => sdp.sdp,
        Some(SignalingMessage::Error { message }) => return step("offer", false, message).map(|_| ()),
        _ => return step("offer", false, "no SDP offer within 15 s").map(|_| ()),
    };
    step("offer", true, format!("{} ms after RequestSession", t_session.elapsed().as_millis()))?;
    pc.set_remote_description(RTCSessionDescription::offer(offer_sdp)?).await?;
    let answer = pc.create_answer(None).await?;
    pc.set_local_description(answer.clone()).await?;
    out_tx.send(SignalingMessage::Sdp {
        target_id: host.clone(),
        sdp: RtcSessionDescription { ty: RtcSdpType::Answer, sdp: answer.sdp },
        ice_servers: None,
        webtransport_port: None,
        webtransport_cert_hash: None,
    })?;

    // Remote candidates, and the agent's EncoderStatus, until the deadline.
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut encoder_status = None;
    let mut next_report = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if Instant::now() >= next_report {
            println!("      t={:>2}s packets={}", t_session.elapsed().as_secs(), packets.load(Ordering::Relaxed));
            next_report += Duration::from_secs(2);
        }
        if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(200), in_rx.recv()).await {
            match msg {
                SignalingMessage::IceCandidate { candidate, .. } => {
                    let _ = pc
                        .add_ice_candidate(RTCIceCandidateInit {
                            candidate: candidate.candidate,
                            sdp_mid: candidate.sdp_mid,
                            sdp_mline_index: candidate.sdp_mline_index,
                            username_fragment: candidate.username_fragment,
                        })
                        .await;
                }
                SignalingMessage::EncoderStatus { encoder, hw_type, .. } => {
                    encoder_status = Some(format!("{} ({})", encoder, hw_type));
                }
                SignalingMessage::Error { message } => println!("agent error: {}", message),
                _ => {}
            }
        }
    }

    let count = packets.load(Ordering::Relaxed);
    let ttfp = first_packet.lock().unwrap().map(|t| t.duration_since(t_session).as_millis());
    let mut pair = String::from("?");
    for (_, report) in pc.get_stats().await.reports {
        if let webrtc::stats::StatsReportType::CandidatePair(p) = report {
            if p.nominated {
                pair = format!("{} <-> {}", p.local_candidate_id, p.remote_candidate_id);
            }
        }
    }
    println!(
        "      ice={:?} codec=\"{}\" encoder={:?} packets={} first_packet_after={:?} ms pair={}",
        pc.ice_connection_state(),
        codec_seen.lock().unwrap(),
        encoder_status,
        count,
        ttfp,
        pair
    );
    step("video received", count > 50, format!("{} RTP packets in {} s", count, seconds))?;

    // 5. End the session; the agent should accept a new one afterwards.
    out_tx.send(SignalingMessage::EndSession { target_id: host })?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = pc.close().await;
    println!("ALL PASSED");
    Ok(())
}
