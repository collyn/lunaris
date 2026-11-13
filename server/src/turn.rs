//! Built-in TURN relay.
//!
//! When the agent and the client can't reach each other directly (NAT on both
//! sides, a VPN interface the browser ignores, ...), WebRTC can still connect
//! through a relay. The server already is the one address both sides reach, so
//! it runs a TURN server (UDP, default port 3478) and hands each peer the relay
//! at the address that peer used to reach the server. Credentials are
//! short-lived (12 h) HMAC credentials from a per-process secret.
//!
//! Environment:
//!   LUNARIS_TURN=0              disable
//!   LUNARIS_TURN_PORT           listening port (default 3478)
//!   LUNARIS_TURN_RELAY_PORTS    relay port range (default 49160-49260)
//!   LUNARIS_TURN_PUBLIC_IP      address to advertise when the server is
//!                               behind 1:1 NAT (e.g. a cloud VM); otherwise
//!                               every local IPv4 address gets a listener.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use turn::auth::{generate_long_term_credentials, LongTermAuthHandler};
use turn::relay::relay_range::RelayAddressGeneratorRanges;
use turn::server::config::{ConnConfig, ServerConfig};
use turn::server::Server;
use webrtc_util::vnet::net::Net;

const REALM: &str = "lunaris";
const CREDENTIAL_LIFETIME: Duration = Duration::from_secs(12 * 3600);

pub struct TurnService {
    secret: String,
    port: u16,
    public_ip: Option<IpAddr>,
    /// Addresses with a listener (relay address = listening address).
    listener_ips: Vec<IpAddr>,
    _servers: Vec<Server>,
}

struct Settings {
    port: u16,
    relay_ports: (u16, u16),
    public_ip: Option<IpAddr>,
}

fn settings_from_env() -> Option<Settings> {
    let enabled = std::env::var("LUNARIS_TURN")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true);
    if !enabled {
        return None;
    }
    let port = std::env::var("LUNARIS_TURN_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(3478);
    let relay_ports = std::env::var("LUNARIS_TURN_RELAY_PORTS")
        .ok()
        .and_then(|v| parse_port_range(&v))
        .unwrap_or((49160, 49260));
    let public_ip = std::env::var("LUNARIS_TURN_PUBLIC_IP")
        .ok()
        .and_then(|v| v.trim().parse().ok());
    // In a container on a bridge network the local addresses are the
    // container's own, which peers can't reach: only relay when told the
    // host's address.
    if public_ip.is_none() && std::path::Path::new("/.dockerenv").exists() {
        warn!("TURN relay off: running in a container without LUNARIS_TURN_PUBLIC_IP (set it to this host's address and publish the UDP ports)");
        return None;
    }
    Some(Settings { port, relay_ports, public_ip })
}

fn parse_port_range(value: &str) -> Option<(u16, u16)> {
    let (a, b) = value.trim().split_once('-')?;
    let (min, max) = (a.trim().parse::<u16>().ok()?, b.trim().parse::<u16>().ok()?);
    (min > 0 && min <= max).then_some((min, max))
}

/// Local IPv4 addresses a peer could use to reach this server (no loopback,
/// link-local or container bridges).
async fn local_ipv4_addresses() -> Vec<IpAddr> {
    let net = Net::new(None);
    let mut ips: Vec<IpAddr> = net
        .get_interfaces()
        .await
        .iter()
        .filter(|iface| {
            let name = iface.name();
            !["docker", "br-", "veth", "virbr", "lxcbr", "lxdbr", "podman", "cni", "vmnet", "vboxnet"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .flat_map(|iface| iface.addrs().iter().map(|n| n.addr()).collect::<Vec<_>>())
        .filter(|ip| match ip {
            IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
            IpAddr::V6(_) => false,
        })
        .collect();
    ips.sort();
    ips.dedup();
    ips
}

impl TurnService {
    /// Starts the relay as configured by the environment; None when disabled
    /// or when no listener could be opened.
    pub async fn start_from_env() -> Option<Arc<Self>> {
        let settings = settings_from_env()?;
        let secret = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let net = Arc::new(Net::new(None));

        // (bind address, relay address advertised to peers)
        let binds: Vec<(IpAddr, IpAddr)> = match settings.public_ip {
            Some(public) => vec![(IpAddr::from([0, 0, 0, 0]), public)],
            None => local_ipv4_addresses().await.into_iter().map(|ip| (ip, ip)).collect(),
        };

        let mut servers = Vec::new();
        let mut listener_ips = Vec::new();
        for (bind_ip, relay_ip) in binds {
            let socket = match tokio::net::UdpSocket::bind(SocketAddr::new(bind_ip, settings.port)).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("TURN relay: cannot listen on {}:{}: {}", bind_ip, settings.port, e);
                    continue;
                }
            };
            let config = ServerConfig {
                conn_configs: vec![ConnConfig {
                    conn: Arc::new(socket),
                    relay_addr_generator: Box::new(RelayAddressGeneratorRanges {
                        relay_address: relay_ip,
                        min_port: settings.relay_ports.0,
                        max_port: settings.relay_ports.1,
                        max_retries: 20,
                        address: bind_ip.to_string(),
                        net: net.clone(),
                    }),
                }],
                realm: REALM.to_string(),
                auth_handler: Arc::new(LongTermAuthHandler::new(secret.clone())),
                channel_bind_timeout: Duration::from_secs(0),
                alloc_close_notify: None,
            };
            match Server::new(config).await {
                Ok(server) => {
                    servers.push(server);
                    listener_ips.push(relay_ip);
                }
                Err(e) => warn!("TURN relay on {}:{} failed to start: {}", bind_ip, settings.port, e),
            }
        }

        if servers.is_empty() {
            warn!("TURN relay disabled: no listener could be opened");
            return None;
        }
        info!(
            "TURN relay listening on UDP {} for {:?} (relay ports {}-{})",
            settings.port, listener_ips, settings.relay_ports.0, settings.relay_ports.1
        );
        Some(Arc::new(Self {
            secret,
            port: settings.port,
            public_ip: settings.public_ip,
            listener_ips,
            _servers: servers,
        }))
    }

    /// The relay entry for a peer whose signaling connection arrived on
    /// `local_ip` (the server address that peer uses). `fallback_ip` is used
    /// when that address has no relay (e.g. loopback), typically the address
    /// the other peer uses.
    pub fn ice_server_for(&self, local_ip: Option<IpAddr>, fallback_ip: Option<IpAddr>) -> Option<common::RtcIceServer> {
        let host = self.public_ip.or_else(|| {
            [local_ip, fallback_ip]
                .into_iter()
                .flatten()
                .map(normalize_ip)
                .find(|ip| self.listener_ips.contains(ip))
                .or_else(|| self.listener_ips.first().copied())
        })?;
        let (username, credential) = generate_long_term_credentials(&self.secret, CREDENTIAL_LIFETIME).ok()?;
        Some(common::RtcIceServer {
            urls: vec![format!("turn:{}:{}?transport=udp", host, self.port)],
            username: Some(username),
            credential: Some(credential),
        })
    }
}

/// IPv4-mapped IPv6 (from a dual-stack socket) as plain IPv4.
fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_ranges() {
        assert_eq!(parse_port_range("49160-49260"), Some((49160, 49260)));
        assert_eq!(parse_port_range(" 50000 - 50010 "), Some((50000, 50010)));
        assert_eq!(parse_port_range("50010-50000"), None);
        assert_eq!(parse_port_range("0-10"), None);
        assert_eq!(parse_port_range("50000"), None);
    }

    fn service(listener_ips: &[&str], public_ip: Option<&str>) -> TurnService {
        TurnService {
            secret: "s".into(),
            port: 3478,
            public_ip: public_ip.map(|p| p.parse().unwrap()),
            listener_ips: listener_ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            _servers: vec![],
        }
    }

    #[test]
    fn hands_each_peer_the_address_it_uses() {
        let turn = service(&["172.22.0.4", "192.168.45.93"], None);
        let url = |s: Option<common::RtcIceServer>| s.unwrap().urls[0].clone();
        assert_eq!(url(turn.ice_server_for(Some("172.22.0.4".parse().unwrap()), None)), "turn:172.22.0.4:3478?transport=udp");
        assert_eq!(url(turn.ice_server_for(Some("192.168.45.93".parse().unwrap()), None)), "turn:192.168.45.93:3478?transport=udp");
        // Loopback has no relay: use the other peer's address.
        assert_eq!(
            url(turn.ice_server_for(Some("127.0.0.1".parse().unwrap()), Some("172.22.0.4".parse().unwrap()))),
            "turn:172.22.0.4:3478?transport=udp"
        );
        // A configured public address wins.
        let nat = service(&["10.0.0.5"], Some("203.0.113.7"));
        assert_eq!(url(nat.ice_server_for(Some("10.0.0.5".parse().unwrap()), None)), "turn:203.0.113.7:3478?transport=udp");
        let creds = turn.ice_server_for(Some("172.22.0.4".parse().unwrap()), None).unwrap();
        assert!(creds.username.is_some() && creds.credential.is_some());
    }
}
