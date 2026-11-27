//! Sunshine / GameStream connector: the client's side of the protocol that
//! Moonlight speaks (ported from moonlight-common-c and moonlight-embedded,
//! GPLv3; the host's side read in Sunshine's source).
//!
//! Lunaris is only ever a client of a Sunshine that the person installed and
//! runs: it pairs (the person types the PIN shown here into Sunshine's own
//! page), asks for the application list, starts or joins an application,
//! receives its picture and sound and sends input. Nothing else: no
//! administration interface, no configuration of the host.
//!
//! Parts: `http` (identity, HTTPS requests), `pair`, `rtsp`, `control` (the
//! control stream over ENet), `video` / `audio` / `fec` (receiving), `input`
//! and `keymap`, `session` (one connection from start to end).
//!
//! Limits: stereo sound; 8 bit 4:2:0 SDR picture (no HDR, no 4:4:4); one
//! display, the one the host's configuration shows; game controllers
//! without rumble, motion or their touch pads; no touch or pen; no Wake-on-LAN; bitrate, frame rate and size are fixed
//! when a session starts (they are the viewer's wish then, within the
//! computer's `resolution` and `bitrate`). The pointer is the one in the
//! host's picture; the protocol has neither its shape nor its place. With
//! `pointer` = `own` the host is asked to leave it out and the viewer shows
//! its own (session.rs pointer_shortcut).

pub mod audio;
pub mod control;
pub mod fec;
pub mod http;
pub mod input;
pub mod keymap;
pub mod net;
pub mod pair;
pub mod rtsp;
pub mod session;
pub mod video;

/// A host inside the test process (from Sunshine's nvhttp.cpp, rtsp.cpp,
/// stream.cpp and input.cpp), for this connector's tests.
#[cfg(test)]
pub(crate) mod fake_host;

use std::collections::BTreeMap;
use std::sync::OnceLock;

use tokio::sync::mpsc;

use super::{Capabilities, Codec, Connector, Description, Field, Session, SetupStep, Target, Viewer};

/// What the person reads under the Type choice of "Add computer".
pub const NOTE: &str = "Sunshine must already be installed and running on that computer. After you press Add, a PIN is shown here: enter it in Sunshine's web page (PIN).";
/// What the person reads beside the PIN.
pub const SETUP_MESSAGE: &str = "Open Sunshine on the other computer (its web page, PIN) and enter this PIN.";
/// The name the host shows for this client's pairing request.
pub const DEVICE_NAME: &str = "Lunaris";

/// A test seam, read once per process: with `LUNARIS_CONNECTOR_TEST_NO_INPUT=1`
/// in the environment every input command (pointer, wheel, key, text) is
/// dropped before any packet is formed. It exists because checks against a
/// person's real computer must never move its pointer or type there. Nothing
/// else changes: key frame requests and closing work as always.
const NO_INPUT_VARIABLE: &str = "LUNARIS_CONNECTOR_TEST_NO_INPUT";

fn no_input_value(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

fn no_input_from_environment() -> bool {
    static NO_INPUT: OnceLock<bool> = OnceLock::new();
    *NO_INPUT.get_or_init(|| {
        let on = no_input_value(std::env::var(NO_INPUT_VARIABLE).ok().as_deref());
        if on {
            log::warn!("sunshine: {NO_INPUT_VARIABLE} is set: no input is sent to any computer");
        }
        on
    })
}

/// The built-in connector for computers that run Sunshine (or another
/// GameStream host of generation 7.1.431 or later).
pub struct SunshineConnector {
    no_input: bool,
}

impl SunshineConnector {
    pub fn new() -> SunshineConnector {
        SunshineConnector { no_input: no_input_from_environment() }
    }

    /// The test seam set directly (tests of this crate; the environment is
    /// read only once per process).
    #[cfg(test)]
    pub(crate) fn with_no_input(no_input: bool) -> SunshineConnector {
        SunshineConnector { no_input }
    }
}

impl Default for SunshineConnector {
    fn default() -> Self {
        SunshineConnector::new()
    }
}

#[async_trait::async_trait]
impl Connector for SunshineConnector {
    fn describe(&self) -> Description {
        let field = |key: &str, label: &str, kind: &str, default: &str, required: bool| Field {
            key: key.into(),
            label: label.into(),
            kind: kind.into(),
            default: default.into(),
            required,
            options: Vec::new(),
        };
        let option = |value: &str, label: &str| common::FieldOptionMsg { value: value.into(), label: label.into() };
        Description {
            id: "sunshine".into(),
            name: "Sunshine / GameStream".into(),
            fields: vec![
                field("host", "Address", "text", "", true),
                field("port", "Port (HTTP)", "port", &http::DEFAULT_HTTP_PORT.to_string(), false),
                field("app", "Application", "text", session::DEFAULT_APP, false),
                field("resolution", "Resolution", "text", session::DEFAULT_RESOLUTION, false),
                field("bitrate", "Bitrate (Mbit/s)", "text", session::DEFAULT_BITRATE_MBIT, false),
                // "picture": the pointer is part of what Sunshine sends (its
                // own). "own": the viewer's pointer, the host's taken out of
                // the picture with Sunshine's shortcut (session.rs
                // pointer_shortcut, and what cannot be promised of it).
                Field {
                    options: vec![
                        option(session::POINTER_PICTURE, "In the picture (host's pointer)"),
                        option(session::POINTER_OWN, "Drawn here (no lag; arrow only)"),
                    ],
                    ..field("pointer", "Pointer", "choice", session::POINTER_PICTURE, false)
                },
            ],
            note: NOTE.into(),
            capabilities: Capabilities {
                video: vec![Codec::H264, Codec::H265, Codec::Av1],
                audio: true,
                input: true,
                clipboard: false,
                cursor: false,
                files: false,
                setup: true,
            },
        }
    }

    async fn connect(&self, target: &Target, viewer: &Viewer) -> anyhow::Result<Session> {
        session::connect(target, viewer, self.no_input).await
    }

    /// Pairing: a new identity for this computer, a PIN to show, and the wait
    /// for the person to type it at the host. Dropping the future ends the
    /// attempt (the host is told, pair.rs).
    async fn setup(&self, target: &Target, steps: mpsc::Sender<SetupStep>) -> anyhow::Result<BTreeMap<String, String>> {
        let host = target.get("host").trim().to_string();
        if host.is_empty() {
            anyhow::bail!("The computer's address is missing");
        }
        let port = match target.get("port").trim() {
            "" => http::DEFAULT_HTTP_PORT,
            port => port.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| anyhow::anyhow!("\"{port}\" is not a port"))?,
        };
        // Making the RSA key takes a moment of CPU.
        let identity = tokio::task::spawn_blocking(http::Identity::generate)
            .await
            .map_err(|_| anyhow::anyhow!("{}", http::Error::Internal("key generation")))?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // Is the computer there at all? Asked before a PIN is shown for nothing.
        http::HostClient::new(&host, port, &identity, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .server_info_http()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let pin = pair::generate_pin().map_err(|e| anyhow::anyhow!("{e}"))?;
        if steps.send(SetupStep { message: SETUP_MESSAGE.into(), code: Some(pin.clone()) }).await.is_err() {
            anyhow::bail!("Pairing was cancelled");
        }
        let paired = pair::pair(&host, port, &identity, &pin, DEVICE_NAME, pair::DEFAULT_PIN_TIMEOUT)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut values = BTreeMap::new();
        values.insert("client_cert".to_string(), identity.client_cert_pem.clone());
        values.insert("client_key".to_string(), identity.client_key_pem.clone());
        values.insert("server_cert".to_string(), paired.server_cert_pem.clone());
        values.insert("unique_id".to_string(), identity.unique_id.clone());
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_is_the_contract() {
        let d = SunshineConnector::with_no_input(false).describe();
        assert_eq!(d.id, "sunshine");
        assert_eq!(d.name, "Sunshine / GameStream");
        let fields: Vec<(&str, &str, &str, &str, bool)> =
            d.fields.iter().map(|f| (f.key.as_str(), f.label.as_str(), f.kind.as_str(), f.default.as_str(), f.required)).collect();
        assert_eq!(
            fields,
            vec![
                ("host", "Address", "text", "", true),
                ("port", "Port (HTTP)", "port", "47989", false),
                ("app", "Application", "text", "Desktop", false),
                ("resolution", "Resolution", "text", "1920x1080", false),
                ("bitrate", "Bitrate (Mbit/s)", "text", "20", false),
                ("pointer", "Pointer", "choice", "picture", false),
            ]
        );
        assert_eq!(
            d.note,
            "Sunshine must already be installed and running on that computer. After you press Add, a PIN is shown here: enter it in Sunshine's web page (PIN)."
        );
        assert_eq!(SETUP_MESSAGE, "Open Sunshine on the other computer (its web page, PIN) and enter this PIN.");
        assert_eq!(d.capabilities.video, vec![Codec::H264, Codec::H265, Codec::Av1]);
        assert!(d.capabilities.audio && d.capabilities.input && d.capabilities.setup);
        assert!(!d.capabilities.clipboard && !d.capabilities.cursor && !d.capabilities.files);
    }

    #[test]
    fn the_test_switch_is_on_only_for_1() {
        assert!(no_input_value(Some("1")));
        assert!(no_input_value(Some(" 1 ")));
        assert!(!no_input_value(Some("0")));
        assert!(!no_input_value(Some("")));
        assert!(!no_input_value(Some("true")));
        assert!(!no_input_value(None));
    }
}
