//! Routes signaling between the agent's session logic and its clients.
//!
//! Clients reach the agent either through the Lunaris server or directly
//! (`direct.rs`). The session logic only sees one stream of
//! [`SignalingMessage`]s addressed with a client connection id, exactly as the
//! server forwards them, and answers with [`AgentMessage`]s addressed to that
//! id. The hub sends each answer to the direct client that owns the id, or to
//! the server otherwise, and for direct clients does the id rewriting the
//! server does for its own clients.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use common::{AgentMessage, ServerToClientMessage, SignalingMessage};
use tokio::sync::mpsc;
use tracing::debug;

pub struct Hub {
    agent_id: String,
    direct_clients: RwLock<HashMap<String, mpsc::UnboundedSender<ServerToClientMessage>>>,
    server: RwLock<Option<mpsc::UnboundedSender<AgentMessage>>>,
    /// Servers that connected to this agent (`/ws/server` of direct.rs),
    /// spoken to like the server: link id -> its outbound queue.
    links: RwLock<HashMap<String, mpsc::UnboundedSender<AgentMessage>>>,
    /// The session logic's id of a client of a link (`link_client_id`) ->
    /// (the link it came through, the client's id on that link's server).
    link_clients: RwLock<HashMap<String, (String, String)>>,
}

impl Hub {
    pub fn new(agent_id: String) -> Arc<Self> {
        Arc::new(Self {
            agent_id,
            direct_clients: RwLock::new(HashMap::new()),
            server: RwLock::new(None),
            links: RwLock::new(HashMap::new()),
            link_clients: RwLock::new(HashMap::new()),
        })
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn add_link(&self, link_id: String, tx: mpsc::UnboundedSender<AgentMessage>) {
        self.links.write().unwrap().insert(link_id, tx);
    }

    /// Forgets a link; returns the client connections that came through it.
    pub fn remove_link(&self, link_id: &str) -> Vec<String> {
        self.links.write().unwrap().remove(link_id);
        let mut clients = self.link_clients.write().unwrap();
        let gone: Vec<String> = clients.iter().filter(|(_, (l, _))| l == link_id).map(|(c, _)| c.clone()).collect();
        for c in &gone {
            clients.remove(c);
        }
        gone
    }

    /// Replies to `id` (the session logic's id of a link's client,
    /// `link_client_id`) go back through `link_id`, addressed to
    /// `client_id`, that client's id on the link's server.
    pub fn link_client(&self, id: &str, client_id: &str, link_id: &str) {
        self.link_clients.write().unwrap().insert(id.to_string(), (link_id.to_string(), client_id.to_string()));
    }

    /// The outbound queue of the current server connection, or None while
    /// disconnected.
    pub fn set_server(&self, tx: Option<mpsc::UnboundedSender<AgentMessage>>) {
        *self.server.write().unwrap() = tx;
    }

    pub fn add_direct_client(&self, conn_id: String, tx: mpsc::UnboundedSender<ServerToClientMessage>) {
        self.direct_clients.write().unwrap().insert(conn_id, tx);
    }

    pub fn remove_direct_client(&self, conn_id: &str) {
        self.direct_clients.write().unwrap().remove(conn_id);
    }

    /// Delivers one message from the session logic.
    pub fn route(&self, msg: AgentMessage) {
        let AgentMessage::Signaling(sig) = msg else {
            self.send_to_server(msg);
            return;
        };
        match target_of(&sig).map(str::to_owned) {
            Some(target) => {
                let direct_tx = self.direct_clients.read().unwrap().get(&target).cloned();
                match direct_tx {
                    Some(tx) => {
                        // Direct clients address the agent by its id, like the
                        // server rewrites agent replies for its clients.
                        let _ = tx.send(ServerToClientMessage::Signaling(with_target(sig, &self.agent_id)));
                    }
                    None => {
                        let link = self.link_clients.read().unwrap().get(&target).cloned();
                        let link_tx = link.and_then(|(l, client)| self.links.read().unwrap().get(&l).cloned().map(|tx| (tx, client)));
                        match link_tx {
                            Some((tx, client)) => {
                                // Addressed as that server knows the client.
                                let _ = tx.send(AgentMessage::Signaling(with_target(sig, &client)));
                            }
                            None if is_link_client_id(&target) => {
                                // A link that is gone: its clients' replies go nowhere.
                                debug!("Dropping a reply for {}: its server link is gone", target);
                            }
                            None => self.send_to_server(AgentMessage::Signaling(sig)),
                        }
                    }
                }
            }
            None => {
                // Not addressed (Error): tell everyone who might be waiting.
                for tx in self.direct_clients.read().unwrap().values() {
                    let _ = tx.send(ServerToClientMessage::Signaling(sig.clone()));
                }
                for tx in self.links.read().unwrap().values() {
                    let _ = tx.send(AgentMessage::Signaling(sig.clone()));
                }
                self.send_to_server(AgentMessage::Signaling(sig));
            }
        }
    }

    fn send_to_server(&self, msg: AgentMessage) {
        match self.server.read().unwrap().as_ref() {
            Some(tx) => {
                let _ = tx.send(msg);
            }
            None => debug!("Not connected to the server; dropping {:?}", msg),
        }
    }
}

/// The session logic's id for client `client` of the server link `link_id`
/// (direct.rs `serve_link`): ids from different links (and from the server
/// or direct clients, which never contain this separator after a link id)
/// cannot collide, so one link cannot take another's client over.
pub fn link_client_id(link_id: &str, client: &str) -> String {
    format!("{}{}{}", link_id, LINK_CLIENT_SEPARATOR, client)
}

const LINK_CLIENT_SEPARATOR: char = '/';

fn is_link_client_id(id: &str) -> bool {
    id.starts_with("link-") && id.contains(LINK_CLIENT_SEPARATOR)
}

/// Starts the task that routes the session logic's replies; returns the
/// sender the session logic writes to.
pub fn spawn_router(hub: Arc<Hub>) -> (mpsc::UnboundedSender<AgentMessage>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentMessage>();
    let task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            hub.route(msg);
        }
    });
    (tx, task)
}

/// The client connection (or host) a message is addressed to.
pub fn target_of(sig: &SignalingMessage) -> Option<&str> {
    use SignalingMessage::*;
    match sig {
        RequestSession { host_id, .. } | RequestFileSession { host_id, .. } => Some(host_id),
        IncomingSession { client_id, .. } | IncomingFileSession { client_id, .. } => Some(client_id),
        Sdp { target_id, .. }
        | IceCandidate { target_id, .. }
        | EndSession { target_id }
        | GetAppList { target_id }
        | AppListResponse { target_id, .. }
        | StopActiveStream { target_id }
        | StopActiveStreamResponse { target_id, .. }
        | GetCapabilities { target_id }
        | CapabilitiesResponse { target_id, .. }
        | EncoderStatus { target_id, .. }
        | SourceStatus { target_id, .. }
        | AccessRequest { target_id, .. } => Some(target_id),
        Error { .. } => None,
    }
}

/// The same message addressed to `target`.
pub fn with_target(mut sig: SignalingMessage, target: &str) -> SignalingMessage {
    use SignalingMessage::*;
    match &mut sig {
        RequestSession { host_id, .. } | RequestFileSession { host_id, .. } => *host_id = target.to_string(),
        IncomingSession { client_id, .. } | IncomingFileSession { client_id, .. } => *client_id = target.to_string(),
        Sdp { target_id, .. }
        | IceCandidate { target_id, .. }
        | EndSession { target_id }
        | GetAppList { target_id }
        | AppListResponse { target_id, .. }
        | StopActiveStream { target_id }
        | StopActiveStreamResponse { target_id, .. }
        | GetCapabilities { target_id }
        | CapabilitiesResponse { target_id, .. }
        | EncoderStatus { target_id, .. }
        | SourceStatus { target_id, .. }
        | AccessRequest { target_id, .. } => *target_id = target.to_string(),
        Error { .. } => {}
    }
    sig
}

/// Turns a message from a direct client into what the session logic expects,
/// as the server does in `handle_client_signaling`: a session request becomes
/// an `IncomingSession` and everything is addressed with the client's
/// connection id. Messages a client may not send yield None.
pub fn from_direct_client(sig: SignalingMessage, conn_id: &str) -> Option<SignalingMessage> {
    use SignalingMessage::*;
    match sig {
        RequestSession {
            host_id: _,
            width,
            height,
            fps,
            bitrate,
            codec,
            app_id,
            encoder,
            display_id,
            chroma,
            encoder_csc_mode,
            microphone,
            source_id,
            decode_codecs,
        } => Some(IncomingSession {
            client_id: conn_id.to_string(),
            width,
            height,
            fps,
            bitrate,
            codec,
            app_id,
            encoder,
            display_id,
            chroma,
            encoder_csc_mode,
            microphone,
            source_id,
            decode_codecs,
            ice_servers: None,
            requester: None,
        }),
        RequestFileSession { source_id, .. } => Some(IncomingFileSession {
            client_id: conn_id.to_string(),
            ice_servers: None,
            requester: None,
            source_id,
        }),
        Sdp {
            sdp,
            webtransport_port,
            webtransport_cert_hash,
            ..
        } => Some(Sdp {
            target_id: conn_id.to_string(),
            sdp,
            ice_servers: None,
            webtransport_port,
            webtransport_cert_hash,
            webtransport_token: None,
        }),
        sig @ (IceCandidate { .. }
        | EndSession { .. }
        | GetAppList { .. }
        | StopActiveStream { .. }
        | GetCapabilities { .. }) => Some(with_target(sig, conn_id)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{RtcIceCandidate, RtcSdpType, RtcSessionDescription};

    fn ice(target: &str) -> SignalingMessage {
        SignalingMessage::IceCandidate {
            target_id: target.into(),
            candidate: RtcIceCandidate {
                candidate: "candidate:1 1 udp 1 10.0.0.1 5000 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
                username_fragment: None,
            },
        }
    }

    #[test]
    fn a_direct_file_session_keeps_its_source() {
        let incoming = |source_id: Option<String>| from_direct_client(SignalingMessage::RequestFileSession { host_id: "direct".into(), source_id }, "conn-7");
        assert!(matches!(
            incoming(Some("den-pc".into())),
            Some(SignalingMessage::IncomingFileSession { client_id, source_id: Some(source), .. }) if client_id == "conn-7" && source == "den-pc"
        ));
        assert!(matches!(incoming(None), Some(SignalingMessage::IncomingFileSession { source_id: None, .. })));
    }

    #[test]
    fn session_request_becomes_incoming_session_for_the_connection() {
        let req = SignalingMessage::RequestSession {
            host_id: "whatever".into(),
            width: Some(1920),
            height: Some(1080),
            fps: Some(60),
            bitrate: Some(20000),
            codec: Some("h264".into()),
            app_id: None,
            encoder: None,
            display_id: None,
            chroma: Some("444".into()),
            encoder_csc_mode: Some(5),
            microphone: Some(true),
            source_id: Some("office-pc".into()),
            decode_codecs: Some(vec!["h264".into(), "vp8".into()]),
        };
        match from_direct_client(req, "direct-1") {
            Some(SignalingMessage::IncomingSession { client_id, width, chroma, encoder_csc_mode, microphone, ice_servers, source_id, decode_codecs, .. }) => {
                assert_eq!(source_id.as_deref(), Some("office-pc"));
                assert_eq!(decode_codecs, Some(vec!["h264".to_string(), "vp8".to_string()]));
                assert_eq!(microphone, Some(true));
                assert_eq!(client_id, "direct-1");
                assert_eq!(width, Some(1920));
                assert_eq!(chroma.as_deref(), Some("444"));
                assert_eq!(encoder_csc_mode, Some(5));
                assert!(ice_servers.is_none());
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn client_messages_are_addressed_with_the_connection_id() {
        let answer = SignalingMessage::Sdp {
            target_id: "host-id".into(),
            sdp: RtcSessionDescription { ty: RtcSdpType::Answer, sdp: "v=0".into() },
            ice_servers: None,
            webtransport_port: None,
            webtransport_cert_hash: None,
            webtransport_token: None,
        };
        let out = from_direct_client(answer, "direct-1").unwrap();
        assert_eq!(target_of(&out), Some("direct-1"));
        assert_eq!(target_of(&from_direct_client(ice("host-id"), "direct-1").unwrap()), Some("direct-1"));
        // A client can't impersonate the agent.
        let agent_reply = SignalingMessage::AppListResponse { target_id: "x".into(), apps: vec![], current_game_id: 0 };
        assert!(from_direct_client(agent_reply, "direct-1").is_none());
    }

    #[tokio::test]
    async fn replies_go_to_the_direct_client_or_the_server() {
        let hub = Hub::new("AGENT".into());
        let (direct_tx, mut direct_rx) = mpsc::unbounded_channel();
        let (server_tx, mut server_rx) = mpsc::unbounded_channel();
        hub.add_direct_client("direct-1".into(), direct_tx);
        hub.set_server(Some(server_tx));

        hub.route(AgentMessage::Signaling(ice("direct-1")));
        match direct_rx.try_recv().unwrap() {
            ServerToClientMessage::Signaling(sig) => assert_eq!(target_of(&sig), Some("AGENT")),
        }
        assert!(server_rx.try_recv().is_err());

        hub.route(AgentMessage::Signaling(ice("server-conn")));
        match server_rx.try_recv().unwrap() {
            AgentMessage::Signaling(sig) => assert_eq!(target_of(&sig), Some("server-conn")),
            other => panic!("unexpected {:?}", other),
        }
        assert!(direct_rx.try_recv().is_err());

        hub.route(AgentMessage::Signaling(SignalingMessage::Error { message: "x".into() }));
        assert!(direct_rx.try_recv().is_ok());
        assert!(server_rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn replies_to_link_clients_go_back_through_their_link() {
        let hub = Hub::new("AGENT".into());
        let (server_tx, mut server_rx) = mpsc::unbounded_channel();
        let (link_tx, mut link_rx) = mpsc::unbounded_channel();
        hub.set_server(Some(server_tx));
        hub.add_link("link-1".into(), link_tx);
        let id = link_client_id("link-1", "conn-a");
        hub.link_client(&id, "conn-a", "link-1");

        hub.route(AgentMessage::Signaling(ice(&id)));
        match link_rx.try_recv().unwrap() {
            // As that server knows the client (it rewrites the target itself).
            AgentMessage::Signaling(sig) => assert_eq!(target_of(&sig), Some("conn-a")),
            other => panic!("unexpected {:?}", other),
        }
        assert!(server_rx.try_recv().is_err());
        // The server's own client of that id is not the link's.
        hub.route(AgentMessage::Signaling(ice("conn-a")));
        assert!(server_rx.try_recv().is_ok() && link_rx.try_recv().is_err());

        assert_eq!(hub.remove_link("link-1"), vec![id.clone()]);
        // Gone with its link: neither the server nor anyone else hears it.
        hub.route(AgentMessage::Signaling(ice(&id)));
        assert!(server_rx.try_recv().is_err());
    }

    /// A link naming another link's client (or the server's) gets its own
    /// id for it: replies to the victim never reach the attacker.
    #[tokio::test]
    async fn a_link_cannot_take_over_another_ones_client() {
        let hub = Hub::new("AGENT".into());
        let (server_tx, mut server_rx) = mpsc::unbounded_channel();
        let (victim_tx, mut victim_rx) = mpsc::unbounded_channel();
        let (attacker_tx, mut attacker_rx) = mpsc::unbounded_channel();
        hub.set_server(Some(server_tx));
        hub.add_link("link-v".into(), victim_tx);
        hub.add_link("link-a".into(), attacker_tx);
        let victim = link_client_id("link-v", "c1");
        hub.link_client(&victim, "c1", "link-v");
        // The attacker uses the same client id, and the victim's full id.
        hub.link_client(&link_client_id("link-a", "c1"), "c1", "link-a");
        hub.link_client(&link_client_id("link-a", &victim), &victim, "link-a");
        hub.link_client(&link_client_id("link-a", "server-conn"), "server-conn", "link-a");

        hub.route(AgentMessage::Signaling(ice(&victim)));
        assert!(victim_rx.try_recv().is_ok());
        hub.route(AgentMessage::Signaling(ice("server-conn")));
        assert!(server_rx.try_recv().is_ok());
        assert!(attacker_rx.try_recv().is_err());
        assert_ne!(link_client_id("link-a", "c1"), victim);
    }
}
