//! RTCP forwarder interceptor for the WHEP client peer.
//!
//! The rtc crate's default interceptor chain (a bare `NoopInterceptor`
//! terminal) consumes inbound RTCP and never forwards it to the application,
//! so `TrackRemoteEvent::OnRtcpPacket` would never fire. This interceptor is
//! registered as the outermost layer: it re-queues every inbound RTCP packet
//! for `poll_read()`, letting the peer connection's endpoint demux route it
//! to the owning track. whepfrom uses this to receive Sender Reports for
//! one-way-delay measurement.
//!
//! Implemented manually (no derive macros) for the same reason as liveion's
//! `RtcpEgressProbe`: the proc-macro output references crate-internal paths
//! that need extra direct dependencies; the manual impl is minimal.

use std::collections::VecDeque;

use rtc::interceptor::{Interceptor, Packet, StreamInfo, TaggedPacket};
use rtc::sansio::Protocol;
use rtc::shared::error::Error;

pub struct RtcpForwarder<P> {
    inner: P,
    read_queue: VecDeque<TaggedPacket>,
}

impl<P> RtcpForwarder<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            read_queue: VecDeque::new(),
        }
    }
}

impl<P> Protocol<TaggedPacket, TaggedPacket, ()> for RtcpForwarder<P>
where
    P: Protocol<
            TaggedPacket,
            TaggedPacket,
            (),
            Rout = TaggedPacket,
            Wout = TaggedPacket,
            Eout = (),
            Time = std::time::Instant,
            Error = Error,
        >,
{
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Time = std::time::Instant;
    type Error = Error;

    fn handle_read(&mut self, msg: TaggedPacket) -> Result<(), Error> {
        // Queue a copy of every inbound RTCP packet for the application;
        // the original still goes down the chain for normal processing.
        if let Packet::Rtcp(packets) = &msg.message {
            self.read_queue.push_back(TaggedPacket {
                now: msg.now,
                transport: msg.transport,
                message: Packet::Rtcp(packets.clone()),
            });
        }
        self.inner.handle_read(msg)
    }

    fn poll_read(&mut self) -> Option<TaggedPacket> {
        if let Some(pkt) = self.read_queue.pop_front() {
            return Some(pkt);
        }
        self.inner.poll_read()
    }

    fn handle_write(&mut self, msg: TaggedPacket) -> Result<(), Error> {
        self.inner.handle_write(msg)
    }
    fn poll_write(&mut self) -> Option<TaggedPacket> {
        self.inner.poll_write()
    }
    fn handle_event(&mut self, evt: ()) -> Result<(), Error> {
        self.inner.handle_event(evt)
    }
    fn poll_event(&mut self) -> Option<()> {
        self.inner.poll_event()
    }
    fn handle_timeout(&mut self, now: std::time::Instant) -> Result<(), Error> {
        self.inner.handle_timeout(now)
    }
    fn poll_timeout(&mut self) -> Option<std::time::Instant> {
        self.inner.poll_timeout()
    }
    fn close(&mut self) -> Result<(), Error> {
        self.read_queue.clear();
        self.inner.close()
    }
}

impl<P: Interceptor> Interceptor for RtcpForwarder<P> {
    fn bind_local_stream(&mut self, info: &StreamInfo) {
        self.inner.bind_local_stream(info);
    }
    fn unbind_local_stream(&mut self, info: &StreamInfo) {
        self.inner.unbind_local_stream(info);
    }
    fn bind_remote_stream(&mut self, info: &StreamInfo) {
        self.inner.bind_remote_stream(info);
    }
    fn unbind_remote_stream(&mut self, info: &StreamInfo) {
        self.inner.unbind_remote_stream(info);
    }
}
