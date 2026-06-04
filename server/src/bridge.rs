use std::sync::Arc;

use tracing::info;

use webrtc::peer_connection::RTCPeerConnection;

pub struct BridgeSession {
    pub peer_connection: Arc<RTCPeerConnection>,
    pub webtransport_port: Option<u16>,
    pub webtransport_cert_hash: Option<String>,
    pub _webtransport_endpoint:
        Option<Arc<wtransport::Endpoint<wtransport::endpoint::endpoint_side::Server>>>,
    pub webtransport_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for BridgeSession {
    fn drop(&mut self) {
        info!("BridgeSession is being dropped...");
        if let Some(shutdown_tx) = self.webtransport_shutdown.take() {
            info!("Sending shutdown signal to WebTransport accept loop...");
            let _ = shutdown_tx.send(());
        }
        if let Some(_endpoint) = self._webtransport_endpoint.take() {
            info!("Dropping WebTransport endpoint reference...");
        }
    }
}

// NOTE: The server-side bridge (agent-less Moonlight→WebRTC) has been removed.
// Streaming now requires an agent to be connected. The agent handles the
// capture pipeline and sends media directly via WebRTC.
//
// The BridgeSession struct and WebRTC setup are kept for potential future use
// in server-mediated relay scenarios.
