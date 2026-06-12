//! AV1 RTP payloader with Annex-B fallback.
//!
//! Wraps the `webrtc` crate's `Av1Payloader` and adds automatic Annex-B to
//! low-overhead OBU conversion when the primary payloader rejects the frame.

use bytes::Bytes;
use webrtc::rtp::codecs::av1::Av1Payloader;
use webrtc::rtp::{self, packetizer::Payloader};

use super::obu::av1_annexb_to_low_overhead;

/// AV1 payloader with Annex-B fallback support.
///
/// If the webrtc `Av1Payloader` fails to packetize a frame (e.g. because the
/// input is in Annex-B format), this wrapper attempts to convert the data to
/// low-overhead OBU format and retries.
#[derive(Debug, Clone, Default)]
pub struct LunarisAv1Payloader {
    inner: Av1Payloader,
}

impl LunarisAv1Payloader {
    pub fn new() -> Self {
        Self {
            inner: Av1Payloader::default(),
        }
    }
}

impl Payloader for LunarisAv1Payloader {
    fn payload(&mut self, mtu: usize, b: &Bytes) -> Result<Vec<Bytes>, rtp::Error> {
        match self.inner.payload(mtu, b) {
            Ok(payloads) => Ok(payloads),
            Err(err) => {
                // Try converting Annex-B to low-overhead OBU format
                if let Some(normalized) = av1_annexb_to_low_overhead(b) {
                    log::debug!(
                        "Retrying AV1 packetization after Annex-B unwrap ({} -> {} bytes)",
                        b.len(),
                        normalized.len()
                    );
                    self.inner.payload(mtu, &normalized).map_err(|_| err)
                } else {
                    Err(err)
                }
            }
        }
    }

    fn clone_to(&self) -> Box<dyn Payloader + Send + Sync> {
        Box::new(self.clone())
    }
}
