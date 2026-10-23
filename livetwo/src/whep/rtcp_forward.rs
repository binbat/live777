//! RTCP marker interceptor for the WHEP client peer.
//!
//! The interceptor chain terminates inbound RTCP at its `NoopInterceptor`
//! unless an interceptor attaches `Attribute::DeliverToApplication` to the
//! packets the application can act on. This interceptor marks every inbound
//! RTCP packet, so `TrackRemoteEvent::OnRtcpPacket` fires and Sender Reports
//! reach whepfrom's track readers for one-way-delay measurement.
//!
//! Implemented manually (no derive macros): the proc-macro output references
//! crate-internal paths that need extra direct dependencies.

use std::collections::VecDeque;

use rtc::interceptor::{Attribute, Interceptor, Packet, StreamInfo, TaggedPacket};
use rtc::sansio::Protocol;
use rtc::shared::error::Error;

pub struct RtcpForwarder {
    read_queue: VecDeque<TaggedPacket>,
    write_queue: VecDeque<TaggedPacket>,
}

impl RtcpForwarder {
    pub fn new() -> Self {
        Self {
            read_queue: VecDeque::new(),
            write_queue: VecDeque::new(),
        }
    }
}

impl Default for RtcpForwarder {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol<TaggedPacket, TaggedPacket, ()> for RtcpForwarder {
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Time = std::time::Instant;
    type Error = Error;

    fn handle_read(&mut self, mut msg: TaggedPacket) -> Result<(), Error> {
        // Mark inbound RTCP for the application; the packet itself travels on
        // down the chain in place — the belt model has no side channel for
        // copies.
        if matches!(msg.message.packet, Packet::Rtcp(_)) {
            msg.message.add(Attribute::DeliverToApplication);
        }
        self.read_queue.push_back(msg);
        Ok(())
    }

    fn poll_read(&mut self) -> Option<TaggedPacket> {
        self.read_queue.pop_front()
    }

    fn handle_write(&mut self, msg: TaggedPacket) -> Result<(), Error> {
        self.write_queue.push_back(msg);
        Ok(())
    }
    fn poll_write(&mut self) -> Option<TaggedPacket> {
        self.write_queue.pop_front()
    }
    fn handle_event(&mut self, _evt: ()) -> Result<(), Error> {
        Ok(())
    }
    fn poll_event(&mut self) -> Option<()> {
        None
    }
    fn handle_timeout(&mut self, _now: std::time::Instant) -> Result<(), Error> {
        Ok(())
    }
    fn poll_timeout(&mut self) -> Option<std::time::Instant> {
        None
    }
    fn close(&mut self) -> Result<(), Error> {
        self.read_queue.clear();
        self.write_queue.clear();
        Ok(())
    }
}

impl Interceptor for RtcpForwarder {
    fn bind_local_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_local_stream(&mut self, _info: &StreamInfo) {}
    fn bind_remote_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_remote_stream(&mut self, _info: &StreamInfo) {}
}
