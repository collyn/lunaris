//! A USB/IP server with one device: the mouse of hid.rs.
//!
//! USB/IP (the Linux kernel's protocol, also spoken by usbip-win2 on
//! Windows) carries a USB device over TCP: the client lists the devices
//! (OP_REQ_DEVLIST), imports one (OP_REQ_IMPORT), then sends it the USB
//! requests of its drivers (USBIP_CMD_SUBMIT) and may cancel them
//! (USBIP_CMD_UNLINK). Every number is big-endian.
//! Reference: https://docs.kernel.org/usb/usbip_protocol.html
//!
//! The host polls the mouse with interrupt IN requests. One is answered as
//! soon as a report exists; until then it waits here, as a real mouse lets
//! it wait (NAK) - nothing is sent while the mouse is still.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use super::hid::{self, Report, ReportQueue};

const VERSION: u16 = 0x0111;
const OP_REQ_DEVLIST: u16 = 0x8005;
const OP_REP_DEVLIST: u16 = 0x0005;
const OP_REQ_IMPORT: u16 = 0x8003;
const OP_REP_IMPORT: u16 = 0x0003;
const CMD_SUBMIT: u32 = 1;
const CMD_UNLINK: u32 = 2;
const RET_SUBMIT: u32 = 3;
const RET_UNLINK: u32 = 4;
const DIR_IN: u32 = 1;
/// -EPIPE: the request is not supported (a stall).
const STALL: i32 = -32;
/// -ECONNRESET: the request was cancelled by an unlink.
const UNLINKED: i32 = -104;
/// USB full speed (12 Mb/s), as a mouse is.
const SPEED_FULL: u32 = 2;

/// The bus ID the device is listed and imported under.
pub const BUS_ID: &str = "1-1";
const PATH: &str = "/lunaris/game-mouse";

/// The mouse as the agent's input sees it, and the requests of the host
/// waiting for its next report.
#[derive(Default)]
pub struct Device {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    queue: ReportQueue,
    /// Interrupt IN requests waiting for a report (sequence numbers).
    waiting: VecDeque<u32>,
    /// Replies to the host, while one has the device imported.
    to_host: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// The host chose the configuration: its mouse driver uses the device.
    configured: bool,
}

impl Device {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Whether a host has the device in use (imported and configured).
    pub fn in_use(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.to_host.is_some() && state.configured
    }

    /// Changes the reports to send (`f` on the queue), then answers the
    /// requests that were waiting for them.
    pub fn update(&self, f: impl FnOnce(&mut ReportQueue)) {
        let mut state = self.state.lock().unwrap();
        f(&mut state.queue);
        state.answer_waiting();
    }
}

impl State {
    fn answer_waiting(&mut self) {
        let Some(to_host) = self.to_host.clone() else { return };
        while !self.waiting.is_empty() {
            let Some(report) = self.queue.pop() else { break };
            let seqnum = self.waiting.pop_front().unwrap();
            let _ = to_host.send(ret_submit(seqnum, hid::ENDPOINT_IN as u32 & 0x0F, DIR_IN, 0, &report.bytes()));
        }
    }
}

/// The device as OP_REP_DEVLIST and OP_REP_IMPORT describe it (312 bytes).
fn device_info() -> Vec<u8> {
    let mut out = Vec::with_capacity(312);
    let mut path = PATH.as_bytes().to_vec();
    path.resize(256, 0);
    out.extend_from_slice(&path);
    let mut bus_id = BUS_ID.as_bytes().to_vec();
    bus_id.resize(32, 0);
    out.extend_from_slice(&bus_id);
    out.extend_from_slice(&1u32.to_be_bytes()); // busnum
    out.extend_from_slice(&1u32.to_be_bytes()); // devnum
    out.extend_from_slice(&SPEED_FULL.to_be_bytes());
    out.extend_from_slice(&hid::VENDOR_ID.to_be_bytes());
    out.extend_from_slice(&hid::PRODUCT_ID.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00]); // bcdDevice 1.00
    out.extend_from_slice(&[0, 0, 0]); // class/subclass/protocol: per interface
    out.extend_from_slice(&[1, 1, 1]); // configuration value, configurations, interfaces
    out
}

fn op_reply(code: u16, status: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&code.to_be_bytes());
    out.extend_from_slice(&status.to_be_bytes());
    out
}

fn devlist_reply() -> Vec<u8> {
    let mut out = op_reply(OP_REP_DEVLIST, 0);
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&device_info());
    out.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]); // interface 0: HID
    out
}

fn ret_submit(seqnum: u32, ep: u32, direction: u32, status: i32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 + data.len());
    for v in [RET_SUBMIT, seqnum, 0, direction, ep] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&status.to_be_bytes());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes()); // actual_length
    out.extend_from_slice(&0u32.to_be_bytes()); // start_frame
    out.extend_from_slice(&0u32.to_be_bytes()); // number_of_packets (not isochronous)
    out.extend_from_slice(&0u32.to_be_bytes()); // error_count
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(data);
    out
}

fn ret_unlink(seqnum: u32, status: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    for v in [RET_UNLINK, seqnum, 0, 0, 0] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&status.to_be_bytes());
    out.extend_from_slice(&[0; 24]);
    out
}

/// The answer to a control request on endpoint 0, `None` for a stall.
fn control(setup: [u8; 8], state: &mut State) -> Option<Vec<u8>> {
    let request_type = setup[0];
    let request = setup[1];
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let [index, kind] = value.to_le_bytes();
    match (request_type, request) {
        // GET_DESCRIPTOR (device)
        (0x80, 0x06) => match kind {
            0x01 => Some(hid::device_descriptor()),
            0x02 => Some(hid::configuration_descriptor()),
            0x03 => hid::string_descriptor(index),
            _ => None, // device qualifier, BOS...: a full-speed USB 2.0 device has none
        },
        // GET_DESCRIPTOR (interface): the HID class descriptors
        (0x81, 0x06) => match kind {
            0x21 => Some(hid::hid_descriptor()),
            0x22 => Some(hid::REPORT_DESCRIPTOR.to_vec()),
            _ => None,
        },
        (0x00, 0x09) => {
            // SET_CONFIGURATION
            state.configured = value != 0;
            Some(Vec::new())
        }
        (0x80, 0x08) => Some(vec![u8::from(state.configured)]), // GET_CONFIGURATION
        (0x80, 0x00) | (0x81, 0x00) | (0x82, 0x00) => Some(vec![0, 0]), // GET_STATUS
        (0x80, 0x0A) | (0x81, 0x0A) => Some(vec![0]), // GET_INTERFACE
        // CLEAR_FEATURE / SET_FEATURE (remote wake-up, endpoint halt), SET_INTERFACE
        (0x00..=0x02, 0x01 | 0x03) | (0x01, 0x0B) => Some(Vec::new()),
        // HID: SET_IDLE, SET_PROTOCOL, SET_REPORT
        (0x21, 0x09..=0x0B) => Some(Vec::new()),
        // HID: GET_REPORT (the buttons held, no move), GET_IDLE, GET_PROTOCOL (report)
        (0xA1, 0x01) => Some(Report { buttons: state.queue.buttons(), ..Default::default() }.bytes().to_vec()),
        (0xA1, 0x02) => Some(vec![0]),
        (0xA1, 0x03) => Some(vec![1]),
        _ => None,
    }
}

/// Serves one connection: a device list, or an import and then the
/// device's requests until the host lets it go.
pub async fn serve_connection<S>(stream: S, device: Arc<Device>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut head = [0u8; 8];
    reader.read_exact(&mut head).await?;
    let code = u16::from_be_bytes([head[2], head[3]]);
    match code {
        OP_REQ_DEVLIST => {
            writer.write_all(&devlist_reply()).await?;
            writer.flush().await
        }
        OP_REQ_IMPORT => {
            let mut bus_id = [0u8; 32];
            reader.read_exact(&mut bus_id).await?;
            let asked = String::from_utf8_lossy(&bus_id);
            let asked = asked.trim_end_matches('\0');
            let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
            let taken = {
                let mut state = device.state.lock().unwrap();
                let free = asked == BUS_ID && state.to_host.is_none();
                if free {
                    state.to_host = Some(tx.clone());
                    state.waiting.clear();
                    state.configured = false;
                }
                free
            };
            if !taken {
                writer.write_all(&op_reply(OP_REP_IMPORT, 1)).await?;
                return writer.flush().await;
            }
            let mut reply = op_reply(OP_REP_IMPORT, 0);
            reply.extend_from_slice(&device_info());
            writer.write_all(&reply).await?;
            writer.flush().await?;
            log::info!("Game mouse: plugged in by the USB/IP client");

            let write = tokio::spawn(async move {
                while let Some(frame) = rx.recv().await {
                    if writer.write_all(&frame).await.is_err() {
                        break;
                    }
                }
            });
            let result = requests(&mut reader, &device, &tx).await;
            {
                let mut state = device.state.lock().unwrap();
                state.to_host = None;
                state.waiting.clear();
                state.configured = false;
                state.queue.clear();
            }
            drop(tx);
            let _ = write.await;
            log::info!("Game mouse: unplugged ({})", match &result {
                Ok(()) => "the client let it go".to_string(),
                Err(e) => e.to_string(),
            });
            result
        }
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("USB/IP: unknown operation {other:#06x}"),
        )),
    }
}

/// The URB phase: answers submits and unlinks until the host disconnects.
async fn requests<R: AsyncRead + Unpin>(
    reader: &mut R,
    device: &Device,
    to_host: &mpsc::UnboundedSender<Vec<u8>>,
) -> std::io::Result<()> {
    let mut head = [0u8; 48];
    loop {
        match reader.read_exact(&mut head).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let word = |i: usize| u32::from_be_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        let (command, seqnum, direction, ep) = (word(0), word(4), word(12), word(16));
        match command {
            CMD_SUBMIT => {
                let length = word(24) as usize;
                let packets = word(32);
                let setup: [u8; 8] = head[40..48].try_into().unwrap();
                if direction != DIR_IN && length > 0 {
                    // OUT data (SET_REPORT, ...): read and not needed.
                    let mut data = vec![0u8; length.min(1 << 16)];
                    reader.read_exact(&mut data).await?;
                }
                if packets != 0 && packets != u32::MAX {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "USB/IP: isochronous request"));
                }
                let mut state = device.state.lock().unwrap();
                if ep == 0 {
                    let frame = match control(setup, &mut state) {
                        Some(mut data) => {
                            if direction == DIR_IN {
                                data.truncate(length);
                            } else {
                                data.clear();
                            }
                            ret_submit(seqnum, ep, direction, 0, &data)
                        }
                        None => ret_submit(seqnum, ep, direction, STALL, &[]),
                    };
                    let _ = to_host.send(frame);
                } else if ep == (hid::ENDPOINT_IN & 0x0F) as u32 && direction == DIR_IN {
                    state.waiting.push_back(seqnum);
                    state.answer_waiting();
                } else {
                    let _ = to_host.send(ret_submit(seqnum, ep, direction, STALL, &[]));
                }
            }
            CMD_UNLINK => {
                let target = word(20);
                let mut state = device.state.lock().unwrap();
                let status = match state.waiting.iter().position(|&s| s == target) {
                    Some(i) => {
                        state.waiting.remove(i);
                        UNLINKED
                    }
                    None => 0, // already answered
                };
                let _ = to_host.send(ret_unlink(seqnum, status));
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("USB/IP: unknown command {other}"),
                ))
            }
        }
    }
}

/// Accepts USB/IP connections on `listener` for as long as the agent runs.
pub async fn serve(listener: tokio::net::TcpListener, device: Arc<Device>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                // Only this computer's own USB/IP client (the listener is on
                // the loopback address; this keeps it so).
                if !peer.ip().is_loopback() {
                    continue;
                }
                let _ = stream.set_nodelay(true);
                let device = device.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_connection(stream, device).await {
                        log::warn!("Game mouse: USB/IP connection: {e}");
                    }
                });
            }
            Err(e) => {
                log::warn!("Game mouse: USB/IP accept: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn submit(seqnum: u32, ep: u32, direction: u32, length: u32, setup: [u8; 8]) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [CMD_SUBMIT, seqnum, 0x0001_0001, direction, ep, 0, length, 0, 0, 0] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&setup);
        out
    }

    fn unlink(seqnum: u32, target: u32) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [CMD_UNLINK, seqnum, 0x0001_0001, 0, 0, target] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&[0; 24]);
        out
    }

    async fn read_ret<R: AsyncRead + Unpin>(r: &mut R) -> (u32, u32, i32, Vec<u8>) {
        let mut head = [0u8; 48];
        r.read_exact(&mut head).await.unwrap();
        let word = |i: usize| u32::from_be_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        let command = word(0);
        let len = if command == RET_SUBMIT { word(24) as usize } else { 0 };
        let mut data = vec![0u8; len];
        r.read_exact(&mut data).await.unwrap();
        (command, word(4), word(20) as i32, data)
    }

    async fn import(device: Arc<Device>) -> tokio::io::DuplexStream {
        let (mut client, server) = duplex(1 << 16);
        tokio::spawn(serve_connection(server, device));
        let mut req = Vec::new();
        req.extend_from_slice(&VERSION.to_be_bytes());
        req.extend_from_slice(&OP_REQ_IMPORT.to_be_bytes());
        req.extend_from_slice(&0u32.to_be_bytes());
        let mut bus = BUS_ID.as_bytes().to_vec();
        bus.resize(32, 0);
        req.extend_from_slice(&bus);
        client.write_all(&req).await.unwrap();
        let mut reply = [0u8; 8 + 312];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(u16::from_be_bytes([reply[2], reply[3]]), OP_REP_IMPORT);
        assert_eq!(u32::from_be_bytes(reply[4..8].try_into().unwrap()), 0, "import accepted");
        client
    }

    #[tokio::test]
    async fn the_device_is_listed() {
        let (mut client, server) = duplex(1 << 16);
        tokio::spawn(serve_connection(server, Device::new()));
        let mut req = Vec::new();
        req.extend_from_slice(&VERSION.to_be_bytes());
        req.extend_from_slice(&OP_REQ_DEVLIST.to_be_bytes());
        req.extend_from_slice(&0u32.to_be_bytes());
        client.write_all(&req).await.unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).await.unwrap();
        assert_eq!(reply.len(), 12 + 312 + 4);
        assert_eq!(u32::from_be_bytes(reply[8..12].try_into().unwrap()), 1);
        assert_eq!(&reply[12 + 256..12 + 256 + 3], BUS_ID.as_bytes());
        assert_eq!(u16::from_be_bytes([reply[12 + 300], reply[12 + 301]]), hid::VENDOR_ID);
        assert_eq!(reply[12 + 312], 0x03, "a HID interface");
    }

    #[tokio::test]
    async fn the_host_reads_the_descriptors_and_configures_the_mouse() {
        let device = Device::new();
        let mut c = import(device.clone()).await;
        // GET_DESCRIPTOR device, asked for 64 bytes: the 18 it has.
        c.write_all(&submit(1, 0, DIR_IN, 64, [0x80, 0x06, 0x00, 0x01, 0, 0, 64, 0])).await.unwrap();
        let (cmd, seq, status, data) = read_ret(&mut c).await;
        assert_eq!((cmd, seq, status), (RET_SUBMIT, 1, 0));
        assert_eq!(data, hid::device_descriptor());
        // The configuration, cut to the 9 bytes asked for first.
        c.write_all(&submit(2, 0, DIR_IN, 9, [0x80, 0x06, 0x00, 0x02, 0, 0, 9, 0])).await.unwrap();
        assert_eq!(read_ret(&mut c).await.3.len(), 9);
        // The report descriptor.
        c.write_all(&submit(3, 0, DIR_IN, 255, [0x81, 0x06, 0x00, 0x22, 0, 0, 255, 0])).await.unwrap();
        assert_eq!(read_ret(&mut c).await.3, hid::REPORT_DESCRIPTOR);
        // A device qualifier: none (a stall).
        c.write_all(&submit(4, 0, DIR_IN, 10, [0x80, 0x06, 0x00, 0x06, 0, 0, 10, 0])).await.unwrap();
        assert_eq!(read_ret(&mut c).await.2, STALL);
        assert!(!device.in_use());
        // SET_CONFIGURATION 1.
        c.write_all(&submit(5, 0, 0, 0, [0x00, 0x09, 0x01, 0x00, 0, 0, 0, 0])).await.unwrap();
        assert_eq!(read_ret(&mut c).await.2, 0);
        assert!(device.in_use());
    }

    #[tokio::test]
    async fn a_poll_waits_for_a_report_and_an_unlink_cancels_it() {
        let device = Device::new();
        let mut c = import(device.clone()).await;
        c.write_all(&submit(10, 1, DIR_IN, 8, [0; 8])).await.unwrap();
        c.write_all(&submit(11, 1, DIR_IN, 8, [0; 8])).await.unwrap();
        // Nothing is answered while the mouse is still.
        let quiet = tokio::time::timeout(std::time::Duration::from_millis(50), read_ret(&mut c)).await;
        assert!(quiet.is_err(), "a poll is answered only with a report");
        // A move answers the oldest poll.
        device.update(|q| q.move_by(5, -3));
        let (cmd, seq, status, data) = read_ret(&mut c).await;
        assert_eq!((cmd, seq, status), (RET_SUBMIT, 10, 0));
        assert_eq!(data, Report { dx: 5, dy: -3, ..Default::default() }.bytes());
        // The other one is cancelled: it is not answered afterwards.
        c.write_all(&unlink(12, 11)).await.unwrap();
        assert_eq!(read_ret(&mut c).await, (RET_UNLINK, 12, UNLINKED, vec![]));
        // An unlink of what was answered already: status 0.
        c.write_all(&unlink(13, 10)).await.unwrap();
        assert_eq!(read_ret(&mut c).await.2, 0);
        // A report made before the next poll is kept for it.
        device.update(|q| q.button(0, true));
        c.write_all(&submit(14, 1, DIR_IN, 8, [0; 8])).await.unwrap();
        let (_, seq, _, data) = read_ret(&mut c).await;
        assert_eq!((seq, data[0]), (14, 1));
    }

    #[tokio::test]
    async fn one_host_at_a_time_and_the_device_is_free_again_after_it() {
        let device = Device::new();
        let first = import(device.clone()).await;
        // A second import is refused while the first holds the device.
        let (mut second, server) = duplex(1 << 16);
        tokio::spawn(serve_connection(server, device.clone()));
        let mut req = Vec::new();
        req.extend_from_slice(&VERSION.to_be_bytes());
        req.extend_from_slice(&OP_REQ_IMPORT.to_be_bytes());
        req.extend_from_slice(&0u32.to_be_bytes());
        let mut bus = BUS_ID.as_bytes().to_vec();
        bus.resize(32, 0);
        req.extend_from_slice(&bus);
        second.write_all(&req).await.unwrap();
        let mut reply = [0u8; 8];
        second.read_exact(&mut reply).await.unwrap();
        assert_eq!(u32::from_be_bytes(reply[4..8].try_into().unwrap()), 1);
        // The first lets go: the device can be imported again.
        drop(first);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let _again = import(device.clone()).await;
    }
}
