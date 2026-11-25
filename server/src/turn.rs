//! Built-in TURN relay.
//!
//! When the agent and the client can't reach each other directly (NAT on both
//! sides, a VPN interface the browser ignores, ...), WebRTC can still connect
//! through a relay. The server already is the one address both sides reach, so
//! it runs a TURN server (UDP, default port 3478) and hands each peer the relay
//! at the address that peer used to reach the server. Credentials are
//! short-lived HMAC credentials from a per-process secret, handed only to
//! users allowed to use the host (signaling.rs). Each hand-out gets its own
//! username ("<expiry>:<random label>", the TURN REST API form), and one
//! username can open only a few allocations (`MAX_ALLOCATIONS_PER_USERNAME`),
//! so credentials seen in one session are no general-purpose relay.
//!
//! The relay never sends to loopback, link-local, unspecified, multicast,
//! broadcast or reserved addresses (0.0.0.0/8, 100.64.0.0/10, 192.0.0.0/24,
//! 198.18.0.0/15, 240.0.0.0/4), nor to any address of the server itself
//! except its own relay ports (peers that both relay through it), so it
//! can't be used to reach services on the server.
//!
//! Environment:
//!   LUNARIS_TURN=0              disable
//!   LUNARIS_TURN_PORT           listening port (default 3478)
//!   LUNARIS_TURN_RELAY_PORTS    relay port range (default 49160-49260)
//!   LUNARIS_TURN_PUBLIC_IP      address to advertise when the server is
//!                               behind 1:1 NAT (e.g. a cloud VM); otherwise
//!                               every local IPv4 address gets a listener.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use turn::relay::RelayAddressGenerator;
use webrtc_util::Conn;

use tracing::{info, warn};
use turn::auth::{generate_auth_key_algorithm, AuthHandler};
use turn::relay::relay_range::RelayAddressGeneratorRanges;
use turn::server::config::{ConnConfig, ServerConfig};
use turn::server::Server;
use webrtc_util::vnet::net::Net;

const REALM: &str = "lunaris";
/// Needed to open allocations (refreshes of an existing one keep working
/// after expiry): long enough for new allocations after an ICE restart or a
/// network change in a long session.
/// Each hand-out is for one session (a new username every time), so this
/// only bounds how long a leaked one stays usable.
const CREDENTIAL_LIFETIME: Duration = Duration::from_secs(2 * 3600);
/// Allocations (client addresses) one username may open: a WebRTC peer
/// opens one per local interface, again after a network change.
pub const MAX_ALLOCATIONS_PER_USERNAME: usize = 16;
/// Usernames remembered at once (expired ones are dropped first).
const MAX_TRACKED_USERNAMES: usize = 20_000;

/// The password for `username`: base64(HMAC-SHA1(secret, username)), as
/// coturn's and the TURN REST API's time-limited credentials.
fn credential_for(username: &str, secret: &str) -> String {
    use common::base64::Engine;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret.as_bytes());
    common::base64::engine::general_purpose::STANDARD.encode(ring::hmac::sign(&key, username.as_bytes()).as_ref())
}

/// New credentials for one hand-out: "<unix expiry>:<random label>".
fn session_credentials(secret: &str, lifetime: Duration) -> (String, String) {
    let username = format!("{}:{}", unix_now() + lifetime.as_secs(), crate::security::random_hex(8));
    let password = credential_for(&username, secret);
    (username, password)
}

/// Checks "<expiry>:<label>" usernames and counts the client addresses
/// (= allocations: one per 5-tuple) each one opens.
struct SessionAuthHandler {
    secret: String,
    /// username -> (expiry, client addresses that authenticated with it)
    seen: Mutex<HashMap<String, (u64, Vec<SocketAddr>)>>,
}

impl SessionAuthHandler {
    fn new(secret: String) -> Self {
        Self { secret, seen: Mutex::new(HashMap::new()) }
    }

    /// The expiry of a well-formed, unexpired username; it may also be used
    /// from `src` (a new address within the per-username limit).
    fn admit(&self, username: &str, src: SocketAddr, now: u64) -> Result<(), turn::Error> {
        let (expiry, label) = username
            .split_once(':')
            .ok_or_else(|| turn::Error::Other("not a session username".into()))?;
        let expiry: u64 = expiry.parse().map_err(|_| turn::Error::Other("not a session username".into()))?;
        if label.is_empty() || label.len() > 64 {
            return Err(turn::Error::Other("not a session username".into()));
        }
        if expiry <= now {
            return Err(turn::Error::Other(format!("expired username {username}")));
        }
        let mut seen = self.seen.lock().unwrap();
        if !seen.contains_key(username) && seen.len() >= MAX_TRACKED_USERNAMES {
            seen.retain(|_, (exp, _)| *exp > now);
            if seen.len() >= MAX_TRACKED_USERNAMES {
                return Err(turn::Error::Other("too many relay users".into()));
            }
        }
        let (_, addrs) = seen.entry(username.to_string()).or_insert_with(|| (expiry, Vec::new()));
        if !addrs.contains(&src) {
            if addrs.len() >= MAX_ALLOCATIONS_PER_USERNAME {
                warn!("TURN relay: username {} already has {} allocations; refused {}", username, addrs.len(), src);
                return Err(turn::Error::Other("too many allocations for this username".into()));
            }
            addrs.push(src);
        }
        Ok(())
    }
}

impl AuthHandler for SessionAuthHandler {
    fn password_algorithms(&self) -> Vec<u16> {
        vec![2, 1]
    }

    fn auth_handle_algorithm(&self, username: &str, realm: &str, src_addr: SocketAddr, algorithm: u16) -> Result<Vec<u8>, turn::Error> {
        if algorithm != 1 && algorithm != 2 {
            return Err(turn::Error::Other("unsupported password algorithm".into()));
        }
        self.admit(username, src_addr, unix_now())?;
        generate_auth_key_algorithm(username, realm, &credential_for(username, &self.secret), algorithm)
    }

    fn auth_handle(&self, username: &str, realm: &str, src_addr: SocketAddr) -> Result<Vec<u8>, turn::Error> {
        self.auth_handle_algorithm(username, realm, src_addr, 1)
    }
}

pub struct TurnService {
    secret: String,
    port: u16,
    relay_ports: (u16, u16),
    public_ip: Option<IpAddr>,
    /// Addresses with a listener (relay address = listening address).
    listener_ips: Vec<IpAddr>,
    started_at: u64,
    traffic: Arc<Traffic>,
    servers: Vec<Server>,
}

/// Why the relay is not running, for the admin page.
static OFF_REASON: OnceLock<String> = OnceLock::new();

fn turned_off(reason: &str) {
    warn!("TURN relay off: {}", reason);
    let _ = OFF_REASON.set(reason.to_string());
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What the relay has carried, counted at the relay sockets.
#[derive(Default)]
struct Traffic {
    allocations: AtomicU64,
    bytes: AtomicU64,
    /// Open relay sockets by relay address.
    open: Mutex<HashMap<SocketAddr, Arc<AllocationTraffic>>>,
}

struct AllocationTraffic {
    bytes: AtomicU64,
    since: u64,
}

impl Traffic {
    fn add(&self, allocation: &AllocationTraffic, n: usize) {
        allocation.bytes.fetch_add(n as u64, Ordering::Relaxed);
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
}

/// The built-in relay as the admin page shows it.
#[derive(serde::Serialize)]
pub struct BuiltinTurnStatus {
    pub enabled: bool,
    pub off_reason: Option<String>,
    pub port: Option<u16>,
    pub relay_ports: Option<(u16, u16)>,
    pub public_ip: Option<String>,
    /// The URLs peers are handed, one per listening address.
    pub urls: Vec<String>,
    pub started_at: Option<u64>,
    pub total_allocations: u64,
    pub total_bytes: u64,
    pub active: Vec<ActiveRelay>,
}

/// One open allocation: a peer relaying through the server.
#[derive(serde::Serialize)]
pub struct ActiveRelay {
    /// The peer's address as the relay sees it.
    pub client: String,
    pub relay: String,
    pub bytes: u64,
    pub since: u64,
}

/// Status of the built-in relay (`None` when it is not running).
pub async fn status(turn: Option<&TurnService>) -> BuiltinTurnStatus {
    let Some(turn) = turn else {
        return BuiltinTurnStatus {
            enabled: false,
            off_reason: Some(OFF_REASON.get().cloned().unwrap_or_else(|| "not started".into())),
            port: None,
            relay_ports: None,
            public_ip: None,
            urls: Vec::new(),
            started_at: None,
            total_allocations: 0,
            total_bytes: 0,
            active: Vec::new(),
        };
    };
    let open: HashMap<SocketAddr, Arc<AllocationTraffic>> = turn.traffic.open.lock().unwrap().clone();
    let mut active = Vec::new();
    for server in &turn.servers {
        let Ok(infos) = server.get_allocations_info(None).await else { continue };
        for (five_tuple, info) in infos {
            let traffic = open.get(&info.relay_addr);
            active.push(ActiveRelay {
                client: SocketAddr::new(normalize_ip(five_tuple.src_addr.ip()), five_tuple.src_addr.port()).to_string(),
                relay: info.relay_addr.to_string(),
                bytes: traffic.map(|t| t.bytes.load(Ordering::Relaxed)).unwrap_or(0),
                since: traffic.map(|t| t.since).unwrap_or(0),
            });
        }
    }
    active.sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.client.cmp(&b.client)));
    let hosts: Vec<IpAddr> = match turn.public_ip {
        Some(ip) => vec![ip],
        None => turn.listener_ips.clone(),
    };
    BuiltinTurnStatus {
        enabled: true,
        off_reason: None,
        port: Some(turn.port),
        relay_ports: Some(turn.relay_ports),
        public_ip: turn.public_ip.map(|ip| ip.to_string()),
        urls: hosts.iter().map(|ip| format!("turn:{}:{}?transport=udp", ip, turn.port)).collect(),
        started_at: Some(turn.started_at),
        total_allocations: turn.traffic.allocations.load(Ordering::Relaxed),
        total_bytes: turn.traffic.bytes.load(Ordering::Relaxed),
        active,
    }
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
        turned_off("turned off by LUNARIS_TURN");
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
        turned_off("running in a container without LUNARIS_TURN_PUBLIC_IP (set it to this host's address and publish the UDP ports)");
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

/// Every address of this machine (all interfaces, IPv4 and IPv6): the
/// relay must not send to them.
async fn all_local_addresses() -> Vec<IpAddr> {
    let net = Net::new(None);
    net.get_interfaces()
        .await
        .iter()
        .flat_map(|iface| iface.addrs().iter().map(|n| normalize_ip(n.addr())).collect::<Vec<_>>())
        .collect()
}

impl TurnService {
    /// Starts the relay as configured by the environment; None when disabled
    /// or when no listener could be opened.
    pub async fn start_from_env() -> Option<Arc<Self>> {
        let settings = settings_from_env()?;
        let secret = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let net = Arc::new(Net::new(None));
        let traffic = Arc::new(Traffic::default());

        // (bind address, relay address advertised to peers)
        let binds: Vec<(IpAddr, IpAddr)> = match settings.public_ip {
            Some(public) => vec![(IpAddr::from([0, 0, 0, 0]), public)],
            None => local_ipv4_addresses().await.into_iter().map(|ip| (ip, ip)).collect(),
        };

        // The server's own addresses, which peers may not be relayed to.
        let mut own: std::collections::HashSet<IpAddr> = all_local_addresses().await.into_iter().collect();
        own.extend(binds.iter().map(|(_, relay)| normalize_ip(*relay)));
        own.extend(settings.public_ip.map(normalize_ip));
        let policy = Arc::new(PeerPolicy { own, relay_ports: settings.relay_ports });
        let auth = Arc::new(SessionAuthHandler::new(secret.clone()));

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
                    relay_addr_generator: Box::new(GuardedRelay {
                        inner: RelayAddressGeneratorRanges {
                            relay_address: relay_ip,
                            min_port: settings.relay_ports.0,
                            max_port: settings.relay_ports.1,
                            max_retries: 20,
                            address: bind_ip.to_string(),
                            net: net.clone(),
                        },
                        traffic: traffic.clone(),
                        policy: policy.clone(),
                    }),
                }],
                realm: REALM.to_string(),
                auth_handler: auth.clone(),
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
            turned_off(&format!("no listener could be opened on UDP port {}", settings.port));
            return None;
        }
        info!(
            "TURN relay listening on UDP {} for {:?} (relay ports {}-{})",
            settings.port, listener_ips, settings.relay_ports.0, settings.relay_ports.1
        );
        Some(Arc::new(Self {
            secret,
            port: settings.port,
            relay_ports: settings.relay_ports,
            public_ip: settings.public_ip,
            listener_ips,
            started_at: unix_now(),
            traffic,
            servers,
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
        // A username of its own for each hand-out (one session's peer).
        let (username, credential) = session_credentials(&self.secret, CREDENTIAL_LIFETIME);
        Some(common::RtcIceServer {
            urls: vec![format!("turn:{}:{}?transport=udp", host, self.port)],
            username: Some(username),
            credential: Some(credential),
        })
    }
}

/// Peers the relay never sends to, whatever the server's addresses:
/// loopback, link-local, multicast, broadcast and reserved networks.
fn forbidden_peer(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_link_local()
                || o[0] == 0 // 0.0.0.0/8 ("this network")
                || v4.is_broadcast()
                || v4.is_multicast()
                || o[0] >= 240 // 240.0.0.0/4 (reserved)
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 (carrier-grade NAT)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24 (protocol assignments)
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 (benchmarking)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Where the relay may send: not `forbidden_peer`, and not to the server
/// itself (any of its addresses, unique-local IPv6 ones too) except to its
/// relay ports, where the other peer's relayed address is when both relay
/// through this server.
struct PeerPolicy {
    own: std::collections::HashSet<IpAddr>,
    relay_ports: (u16, u16),
}

impl PeerPolicy {
    fn forbidden(&self, addr: SocketAddr) -> bool {
        let ip = normalize_ip(addr.ip());
        if forbidden_peer(ip) {
            return true;
        }
        self.own.contains(&ip) && !(self.relay_ports.0..=self.relay_ports.1).contains(&addr.port())
    }
}

/// The turn crate has no permission hook, so the relay socket itself refuses
/// forbidden peers. It also counts what each allocation relays.
struct GuardedRelay {
    inner: RelayAddressGeneratorRanges,
    traffic: Arc<Traffic>,
    policy: Arc<PeerPolicy>,
}

#[async_trait]
impl RelayAddressGenerator for GuardedRelay {
    fn validate(&self) -> Result<(), turn::Error> {
        self.inner.validate()
    }

    fn supports_family(&self, use_ipv4: bool) -> bool {
        self.inner.supports_family(use_ipv4)
    }

    async fn allocate_conn(
        &self,
        use_ipv4: bool,
        requested_port: u16,
    ) -> Result<(Arc<dyn Conn + Send + Sync>, SocketAddr), turn::Error> {
        let (conn, addr) = self.inner.allocate_conn(use_ipv4, requested_port).await?;
        let allocation = Arc::new(AllocationTraffic { bytes: AtomicU64::new(0), since: unix_now() });
        self.traffic.allocations.fetch_add(1, Ordering::Relaxed);
        self.traffic.open.lock().unwrap().insert(addr, allocation.clone());
        Ok((
            Arc::new(GuardedConn { inner: conn, addr, allocation, traffic: self.traffic.clone(), policy: self.policy.clone() }),
            addr,
        ))
    }
}

struct GuardedConn {
    inner: Arc<dyn Conn + Send + Sync>,
    /// Relay address, the key in `Traffic::open`.
    addr: SocketAddr,
    allocation: Arc<AllocationTraffic>,
    traffic: Arc<Traffic>,
    policy: Arc<PeerPolicy>,
}

impl Drop for GuardedConn {
    fn drop(&mut self) {
        let mut open = self.traffic.open.lock().unwrap();
        // Only this socket's entry: the port may already belong to a new one.
        if open.get(&self.addr).is_some_and(|a| Arc::ptr_eq(a, &self.allocation)) {
            open.remove(&self.addr);
        }
    }
}

#[async_trait]
impl Conn for GuardedConn {
    async fn connect(&self, addr: SocketAddr) -> webrtc_util::Result<()> {
        if self.policy.forbidden(addr) {
            return Err(webrtc_util::Error::Other(format!("relay to {} refused", addr)));
        }
        self.inner.connect(addr).await
    }
    async fn recv(&self, buf: &mut [u8]) -> webrtc_util::Result<usize> {
        let n = self.inner.recv(buf).await?;
        self.traffic.add(&self.allocation, n);
        Ok(n)
    }
    async fn recv_from(&self, buf: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
        let (n, from) = self.inner.recv_from(buf).await?;
        self.traffic.add(&self.allocation, n);
        Ok((n, from))
    }
    async fn send(&self, buf: &[u8]) -> webrtc_util::Result<usize> {
        let n = self.inner.send(buf).await?;
        self.traffic.add(&self.allocation, n);
        Ok(n)
    }
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> webrtc_util::Result<usize> {
        if self.policy.forbidden(target) {
            // Dropped quietly: an error would be logged for every packet.
            tracing::debug!("TURN relay: dropped data for {}", target);
            return Ok(buf.len());
        }
        let n = self.inner.send_to(buf, target).await?;
        self.traffic.add(&self.allocation, n);
        Ok(n)
    }
    fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        self.inner.remote_addr()
    }
    async fn close(&self) -> webrtc_util::Result<()> {
        self.inner.close().await
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
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
    fn relay_refuses_local_peers() {
        for ip in [
            "127.0.0.1", "169.254.1.2", "0.0.0.0", "0.1.2.3", "::1", "fe80::1", "::ffff:127.0.0.1", "224.0.0.1",
            "255.255.255.255", "240.0.0.1", "100.64.0.1", "100.127.255.254", "192.0.0.8", "198.18.0.1", "198.19.255.1",
        ] {
            assert!(forbidden_peer(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["192.168.1.20", "10.0.0.5", "172.22.0.4", "203.0.113.7", "2001:db8::1", "100.128.0.1", "198.20.0.1", "192.0.2.1"] {
            assert!(!forbidden_peer(ip.parse().unwrap()), "{ip}");
        }
    }

    /// The server's own addresses are off limits, except its relay ports
    /// (both peers relaying through this server).
    #[test]
    fn relay_refuses_the_servers_own_addresses() {
        let policy = PeerPolicy {
            own: ["192.168.45.93", "203.0.113.7", "fd12::5"].iter().map(|ip| ip.parse().unwrap()).collect(),
            relay_ports: (49160, 49260),
        };
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(policy.forbidden(addr("192.168.45.93:22")));
        assert!(policy.forbidden(addr("192.168.45.93:8090")));
        assert!(policy.forbidden(addr("[::ffff:192.168.45.93]:5432")));
        assert!(policy.forbidden(addr("203.0.113.7:3478")), "not even the TURN port itself");
        assert!(policy.forbidden(addr("[fd12::5]:80")), "an own unique-local address");
        assert!(policy.forbidden(addr("127.0.0.1:49200")), "loopback whatever the port");
        assert!(!policy.forbidden(addr("192.168.45.93:49200")), "the other peer's relayed address");
        assert!(!policy.forbidden(addr("192.168.45.20:22")), "another computer of the LAN");
        assert!(!policy.forbidden(addr("[fd12::9]:80")), "someone else's unique-local address");
    }

    #[test]
    fn session_usernames_expire_and_open_few_allocations() {
        let auth = SessionAuthHandler::new("s".into());
        let (user, pass) = session_credentials("s", CREDENTIAL_LIFETIME);
        assert_eq!(pass, credential_for(&user, "s"));
        assert_ne!(session_credentials("s", CREDENTIAL_LIFETIME).0, user, "one username per hand-out");
        let src = |port: u16| SocketAddr::from(([198, 51, 100, 1], port));
        for port in 0..MAX_ALLOCATIONS_PER_USERNAME as u16 {
            assert!(auth.auth_handle(&user, REALM, src(port)).is_ok());
        }
        // The same addresses again (refreshes, retries): fine; a new one: no.
        assert!(auth.auth_handle(&user, REALM, src(0)).is_ok());
        assert!(auth.auth_handle(&user, REALM, src(999)).is_err());
        // Another hand-out has its own allowance.
        let (other, _) = session_credentials("s", CREDENTIAL_LIFETIME);
        assert!(auth.auth_handle(&other, REALM, src(999)).is_ok());
        // Expired, or not of this form (plain timestamps of the old scheme).
        assert!(auth.admit(&format!("{}:abc", unix_now() - 1), src(1), unix_now()).is_err());
        assert!(auth.auth_handle(&format!("{}", unix_now() + 100), REALM, src(1)).is_err());
        assert!(auth.auth_handle("garbage", REALM, src(1)).is_err());
        assert!(auth.auth_handle(&format!("{}:", unix_now() + 100), REALM, src(1)).is_err());
        // The key matches what a client derives from the handed-out password.
        let key = auth.auth_handle_algorithm(&other, REALM, src(999), 2).unwrap();
        assert_eq!(key, generate_auth_key_algorithm(&other, REALM, &credential_for(&other, "s"), 2).unwrap());
    }

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
            relay_ports: (49160, 49260),
            public_ip: public_ip.map(|p| p.parse().unwrap()),
            listener_ips: listener_ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            started_at: 0,
            traffic: Arc::new(Traffic::default()),
            servers: vec![],
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
        assert!(creds.username.as_deref().unwrap().contains(':'), "a per-session username");
        let again = turn.ice_server_for(Some("172.22.0.4".parse().unwrap()), None).unwrap();
        assert_ne!(creds.username, again.username);
    }

    /// A real allocation shows up in the status with what it relayed, and
    /// leaves it when the peer closes it.
    #[tokio::test]
    async fn status_lists_open_allocations_and_counts_bytes() {
        use turn::client::{Client, ClientConfig};

        let secret = "test-secret".to_string();
        let traffic = Arc::new(Traffic::default());
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = socket.local_addr().unwrap();
        let server = Server::new(ServerConfig {
            conn_configs: vec![ConnConfig {
                conn: Arc::new(socket),
                relay_addr_generator: Box::new(GuardedRelay {
                    inner: RelayAddressGeneratorRanges {
                        relay_address: "127.0.0.1".parse().unwrap(),
                        min_port: 49500,
                        max_port: 49599,
                        max_retries: 20,
                        address: "127.0.0.1".into(),
                        net: Arc::new(Net::new(None)),
                    },
                    traffic: traffic.clone(),
                    policy: Arc::new(PeerPolicy { own: Default::default(), relay_ports: (49500, 49599) }),
                }),
            }],
            realm: REALM.to_string(),
            auth_handler: Arc::new(SessionAuthHandler::new(secret.clone())),
            channel_bind_timeout: Duration::from_secs(0),
            alloc_close_notify: None,
        })
        .await
        .unwrap();
        let mut turn = service(&["127.0.0.1"], None);
        turn.port = server_addr.port();
        turn.traffic = traffic;
        turn.servers = vec![server];

        let (username, password) = session_credentials(&secret, CREDENTIAL_LIFETIME);
        let client = Client::new(ClientConfig {
            stun_serv_addr: server_addr.to_string(),
            turn_serv_addr: server_addr.to_string(),
            username,
            password,
            realm: REALM.to_string(),
            software: String::new(),
            rto_in_ms: 0,
            conn: Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap()),
            vnet: None,
        })
        .await
        .unwrap();
        client.listen().await.unwrap();
        let relay_conn = client.allocate().await.unwrap();
        let relay_addr = relay_conn.local_addr().unwrap();

        // A peer sends to the relay address: counted where it arrives.
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        peer.send_to(&[0u8; 1000], relay_addr).await.unwrap();

        let mut found = None;
        for _ in 0..50 {
            let st = status(Some(&turn)).await;
            if st.active.first().is_some_and(|a| a.bytes >= 1000) {
                found = Some(st);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let st = found.expect("allocation with its bytes in the status");
        assert!(st.enabled);
        assert_eq!(st.total_allocations, 1);
        assert_eq!(st.active.len(), 1);
        assert_eq!(st.active[0].relay, relay_addr.to_string());
        assert!(st.active[0].client.starts_with("127.0.0.1:"));
        assert!(st.total_bytes >= 1000);

        relay_conn.close().await.unwrap();
        client.close().await.unwrap();
        let mut gone = false;
        for _ in 0..100 {
            let st = status(Some(&turn)).await;
            if st.active.is_empty() && turn.traffic.open.lock().unwrap().is_empty() {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone, "closed allocation still listed");
        assert_eq!(status(Some(&turn)).await.total_allocations, 1);
    }

    #[tokio::test]
    async fn status_when_off() {
        let st = status(None).await;
        assert!(!st.enabled);
        assert!(st.off_reason.is_some());
        assert!(st.urls.is_empty() && st.active.is_empty());
    }
}
