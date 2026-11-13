//! Hands the client's RTCP feedback about the video stream to the application.
//!
//! In webrtc 0.17 `RTCRtpSender::read` returned every RTCP packet that came
//! in for a sender. In 0.21 inbound RTCP stops at the end of the interceptor
//! chain unless an interceptor marks a packet `Attribute::DeliverToApplication`;
//! the marked packets then arrive as `TrackLocalEvent::OnRtcpPacket`.
//!
//! This is the `RtcpForwarderInterceptor` of the reference project
//! (moonlight-web-stream, `src/api/stream/webrtc/forward_interceptor.rs`, and
//! the webrtc-rs `rtcp-processing` example), widened from "PLI and FIR" to
//! everything the host acts on: PLI and FIR (key frame requests), NACK (only
//! logged: the NACK responder has already answered it) and REMB (bitrate).
//! Everything else (receiver reports, TWCC) is not the application's business
//! and ends here, after the interceptors in front of this one have used it.
//! Register it application-most (`Slot::from(14_000)`), so every other
//! interceptor has seen the whole of the inbound RTCP first.

use std::collections::VecDeque;
use std::time::Instant;

use rtc::interceptor::{Attribute, Interceptor, Packet, StreamInfo, TaggedPacket};
use rtc::rtcp::{
    self,
    payload_feedbacks::{
        full_intra_request::FullIntraRequest, picture_loss_indication::PictureLossIndication,
        receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate,
    },
    transport_feedbacks::transport_layer_nack::TransportLayerNack,
};
use rtc::sansio::Protocol;
use rtc::shared::error::Error;

#[derive(Default)]
pub struct RtcpForwarderInterceptor {
    read_queue: VecDeque<TaggedPacket>,
    write_queue: VecDeque<TaggedPacket>,
}

/// Whether the application wants this RTCP packet.
#[allow(clippy::borrowed_box)]
fn wanted(packet: &Box<dyn rtcp::Packet>) -> bool {
    let p = packet.as_any();
    p.is::<PictureLossIndication>()
        || p.is::<FullIntraRequest>()
        || p.is::<TransportLayerNack>()
        || p.is::<ReceiverEstimatedMaximumBitrate>()
}

impl Protocol<TaggedPacket, TaggedPacket, ()> for RtcpForwarderInterceptor {
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Error = Error;
    type Time = Instant;

    fn handle_read(&mut self, mut msg: TaggedPacket) -> Result<(), Self::Error> {
        if let Packet::Rtcp(packets) = &msg.message.packet {
            let kept: Vec<Box<dyn rtcp::Packet>> =
                packets.iter().filter(|packet| wanted(packet)).cloned().collect();
            if kept.is_empty() {
                return Ok(());
            }
            msg.message.packet = Packet::Rtcp(kept);
            msg.message.add(Attribute::DeliverToApplication);
        }
        self.read_queue.push_back(msg);
        Ok(())
    }

    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.read_queue.pop_front()
    }

    fn handle_write(&mut self, msg: TaggedPacket) -> Result<(), Self::Error> {
        self.write_queue.push_back(msg);
        Ok(())
    }

    fn poll_write(&mut self) -> Option<Self::Wout> {
        self.write_queue.pop_front()
    }
}

impl Interceptor for RtcpForwarderInterceptor {
    fn bind_local_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_local_stream(&mut self, _info: &StreamInfo) {}
    fn bind_remote_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_remote_stream(&mut self, _info: &StreamInfo) {}
}
