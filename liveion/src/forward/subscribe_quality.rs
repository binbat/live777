//! Per-subscriber RTCP quality statistics — the feedback half of the
//! adaptive-bitrate control loop (issue #409).
//!
//! The subscribe peer's interceptor chain (NACK responder, RTCP reports,
//! TWCC sender) handles *outbound* concerns.  Inbound feedback from the
//! subscriber — NACKs, Receiver Reports, TWCC feedback, PLI/FIR — reaches
//! the chain's `handle_read`, but anything not claimed by an interceptor is
//! dropped by the `NoopInterceptor` terminal (the same rtc-crate behaviour
//! that motivated livetwo's `RtcpForwarder`).  `SubscribeQualityInterceptor`
//! observes those packets without consuming them and accumulates per
//! subscriber counters into a shared [`SubscribeRtcpStats`]; the
//! adaptive-bitrate controller
//! (`crate::stream::source::adaptive_bitrate`) samples them once per tick.
//!
//! Implemented manually (no derive macros) for the same reason as
//! `RtcpEgressProbe`: the proc-macro output references crate-internal paths
//! that would need extra direct dependencies.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rtc::interceptor::{Interceptor, Packet, StreamInfo, TaggedPacket};
use rtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtcp::receiver_report::ReceiverReport;
use rtc::rtcp::transport_feedbacks::transport_layer_cc::TransportLayerCc;
use rtc::rtcp::transport_feedbacks::transport_layer_nack::TransportLayerNack;
use rtc::sansio::Protocol;
use rtc::shared::error::Error;

/// One windowed sample of a subscriber's reception quality, produced by
/// [`SubscribeRtcpStats::sample`] once per controller tick.
#[derive(Debug, Clone, Copy)]
pub struct QualitySample {
    /// Time since the subscribe peer was created.
    pub age: Duration,
    /// Packet loss fraction over the sampling window, `[0, 1]`.  From TWCC
    /// feedback when the subscriber sends it (per-packet granularity), else
    /// the latest Receiver Report's `fraction_lost`.
    pub loss: f32,
    /// NACKed packets per second over the window.  Informational: NACKs are
    /// served by the responder's retransmissions, so this overstates the
    /// effective loss.
    pub nack_per_sec: f64,
    /// PLI/FIR requests over the window (decoder distress signal).
    pub pli_fir: u64,
    /// Whether any TWCC feedback has been seen from this subscriber.
    pub twcc: bool,
}

/// Cumulative per-subscriber quality counters.
///
/// Written by the interceptor on every inbound RTCP packet; sampled by the
/// controller once per tick.  `sample()` keeps the previous-window counters
/// internally so windowed deltas need exactly one sampler (true here: one
/// adaptive-bitrate controller per stream).
pub struct SubscribeRtcpStats {
    created_at: Instant,
    /// Video SSRCs this subscriber receives (learned from
    /// `bind_local_stream`), used to attribute feedback to the video flow —
    /// audio loss must not drive the video encoder's bitrate.
    video_ssrcs: Mutex<HashSet<u32>>,
    nack_packets: AtomicU64,
    pli_fir: AtomicU64,
    twcc_received: AtomicU64,
    twcc_lost: AtomicU64,
    /// Latest video Receiver Report's `fraction_lost` (1/256 units).
    rr_fraction_lost: AtomicU64,
    sample_state: Mutex<SampleState>,
}

#[derive(Default)]
struct SampleState {
    prev: Option<PrevCounters>,
}

struct PrevCounters {
    at: Instant,
    nack_packets: u64,
    pli_fir: u64,
    twcc_received: u64,
    twcc_lost: u64,
}

impl SubscribeRtcpStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            created_at: Instant::now(),
            video_ssrcs: Mutex::new(HashSet::new()),
            nack_packets: AtomicU64::new(0),
            pli_fir: AtomicU64::new(0),
            twcc_received: AtomicU64::new(0),
            twcc_lost: AtomicU64::new(0),
            rr_fraction_lost: AtomicU64::new(0),
            sample_state: Mutex::new(SampleState::default()),
        })
    }

    /// Compute the windowed quality sample (deltas since the previous call).
    pub fn sample(&self) -> QualitySample {
        let now = Instant::now();
        let nack = self.nack_packets.load(Ordering::Relaxed);
        let pli = self.pli_fir.load(Ordering::Relaxed);
        let twcc_rx = self.twcc_received.load(Ordering::Relaxed);
        let twcc_lost = self.twcc_lost.load(Ordering::Relaxed);

        let mut state = self.sample_state.lock().unwrap();
        let prev = state.prev.replace(PrevCounters {
            at: now,
            nack_packets: nack,
            pli_fir: pli,
            twcc_received: twcc_rx,
            twcc_lost,
        });

        let (window_secs, d_nack, d_pli, d_rx, d_lost) = match prev {
            Some(p) => (
                now.duration_since(p.at).as_secs_f64().max(f64::EPSILON),
                nack.saturating_sub(p.nack_packets),
                pli.saturating_sub(p.pli_fir),
                twcc_rx.saturating_sub(p.twcc_received),
                twcc_lost.saturating_sub(p.twcc_lost),
            ),
            None => (1.0, 0, 0, 0, 0),
        };

        // TWCC counts every media packet the receiver reports on; prefer it
        // when present.  Otherwise fall back to the subscriber's own RR
        // fraction_lost (already windowed by the subscriber between RRs).
        let loss = if d_rx + d_lost > 0 {
            d_lost as f32 / (d_rx + d_lost) as f32
        } else {
            self.rr_fraction_lost.load(Ordering::Relaxed) as f32 / 256.0
        };

        QualitySample {
            age: self.created_at.elapsed(),
            loss,
            nack_per_sec: d_nack as f64 / window_secs,
            pli_fir: d_pli,
            twcc: twcc_rx + twcc_lost > 0,
        }
    }

    fn is_video_ssrc(&self, ssrc: u32) -> bool {
        // Before the first bind the set is empty; attribute feedback to
        // video anyway so early loss is not silently dropped (audio-only
        // subscribers simply never matter for the encoder).
        let ssrcs = self.video_ssrcs.lock().unwrap();
        ssrcs.is_empty() || ssrcs.contains(&ssrc)
    }

    fn observe(&self, packets: &[Box<dyn rtc::rtcp::packet::Packet>]) {
        for packet in packets {
            let any = packet.as_any();
            if let Some(nack) = any.downcast_ref::<TransportLayerNack>() {
                if self.is_video_ssrc(nack.media_ssrc) {
                    let count: u64 = nack
                        .nacks
                        .iter()
                        .map(|pair| pair.packet_list().len() as u64)
                        .sum();
                    self.nack_packets.fetch_add(count, Ordering::Relaxed);
                }
            } else if let Some(rr) = any.downcast_ref::<ReceiverReport>() {
                for report in &rr.reports {
                    if self.is_video_ssrc(report.ssrc) {
                        self.rr_fraction_lost
                            .store(report.fraction_lost as u64, Ordering::Relaxed);
                    }
                }
            } else if let Some(twcc) = any.downcast_ref::<TransportLayerCc>() {
                if self.is_video_ssrc(twcc.media_ssrc) {
                    // recv_deltas has one entry per received packet; the
                    // remainder of packet_status_count was not received.
                    let received = twcc.recv_deltas.len() as u64;
                    let total = u64::from(twcc.packet_status_count);
                    self.twcc_received.fetch_add(received, Ordering::Relaxed);
                    self.twcc_lost
                        .fetch_add(total.saturating_sub(received), Ordering::Relaxed);
                }
            } else if any.downcast_ref::<PictureLossIndication>().is_some()
                || any.downcast_ref::<FullIntraRequest>().is_some()
            {
                self.pli_fir.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Inbound-RTCP observer for subscribe peers.  Pure tap: every packet is
/// passed down the chain untouched, so the NACK responder's retransmissions
/// keep working.
pub struct SubscribeQualityInterceptor<P> {
    inner: P,
    stats: Arc<SubscribeRtcpStats>,
}

impl<P> SubscribeQualityInterceptor<P> {
    pub fn new(inner: P, stats: Arc<SubscribeRtcpStats>) -> Self {
        Self { inner, stats }
    }
}

impl<P> Protocol<TaggedPacket, TaggedPacket, ()> for SubscribeQualityInterceptor<P>
where
    P: Protocol<
            TaggedPacket,
            TaggedPacket,
            (),
            Rout = TaggedPacket,
            Wout = TaggedPacket,
            Eout = (),
            Time = Instant,
            Error = Error,
        >,
{
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Time = Instant;
    type Error = Error;

    fn handle_read(&mut self, msg: TaggedPacket) -> Result<(), Error> {
        if let Packet::Rtcp(ref packets) = msg.message {
            self.stats.observe(packets);
        }
        self.inner.handle_read(msg)
    }

    fn poll_read(&mut self) -> Option<TaggedPacket> {
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
    fn handle_timeout(&mut self, now: Instant) -> Result<(), Error> {
        self.inner.handle_timeout(now)
    }
    fn poll_timeout(&mut self) -> Option<Instant> {
        self.inner.poll_timeout()
    }
    fn close(&mut self) -> Result<(), Error> {
        self.inner.close()
    }
}

impl<P: Interceptor> Interceptor for SubscribeQualityInterceptor<P> {
    fn bind_local_stream(&mut self, info: &StreamInfo) {
        if info.mime_type.starts_with("video/") {
            self.stats.video_ssrcs.lock().unwrap().insert(info.ssrc);
        }
        self.inner.bind_local_stream(info);
    }
    fn unbind_local_stream(&mut self, info: &StreamInfo) {
        self.stats.video_ssrcs.lock().unwrap().remove(&info.ssrc);
        self.inner.unbind_local_stream(info);
    }
    fn bind_remote_stream(&mut self, info: &StreamInfo) {
        self.inner.bind_remote_stream(info);
    }
    fn unbind_remote_stream(&mut self, info: &StreamInfo) {
        self.inner.unbind_remote_stream(info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtc::rtcp::reception_report::ReceptionReport;
    use rtc::rtcp::transport_feedbacks::transport_layer_nack::NackPair;

    fn sample_after(
        stats: &Arc<SubscribeRtcpStats>,
        packets: Vec<Box<dyn rtc::rtcp::packet::Packet>>,
    ) -> QualitySample {
        // First sample() call seeds the window; observe, then sample again.
        stats.sample();
        stats.observe(&packets);
        stats.sample()
    }

    #[test]
    fn twcc_feedback_drives_loss() {
        let stats = SubscribeRtcpStats::new();
        let twcc = TransportLayerCc {
            media_ssrc: 42,
            packet_status_count: 10,
            recv_deltas: vec![
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
            ],
            ..Default::default()
        };
        let q = sample_after(&stats, vec![Box::new(twcc)]);
        assert!(q.twcc);
        // 3 of 10 packets lost.
        assert!((q.loss - 0.3).abs() < 1e-6);
    }

    #[test]
    fn rr_fraction_used_without_twcc() {
        let stats = SubscribeRtcpStats::new();
        let rr = ReceiverReport {
            reports: vec![ReceptionReport {
                ssrc: 7,
                fraction_lost: 128, // 50%
                ..Default::default()
            }],
            ..Default::default()
        };
        let q = sample_after(&stats, vec![Box::new(rr)]);
        assert!(!q.twcc);
        assert!((q.loss - 0.5).abs() < 1e-6);
    }

    #[test]
    fn nack_and_pli_counted() {
        let stats = SubscribeRtcpStats::new();
        let nack = TransportLayerNack {
            media_ssrc: 1,
            nacks: vec![NackPair {
                packet_id: 100,
                lost_packets: 0b101, // 3 packets total
            }],
            ..Default::default()
        };
        let pli = PictureLossIndication {
            sender_ssrc: 0,
            media_ssrc: 1,
        };
        let q = sample_after(&stats, vec![Box::new(nack), Box::new(pli)]);
        assert_eq!(q.pli_fir, 1);
        // Window duration ~0s in tests, so only check the count moved via a
        // fresh cumulative read instead of the rate.
        assert!(q.nack_per_sec > 0.0);
    }

    #[test]
    fn non_video_ssrc_ignored_once_video_bound() {
        let stats = SubscribeRtcpStats::new();
        stats.video_ssrcs.lock().unwrap().insert(100);
        // Feedback for an unknown (audio) SSRC must not move the counters.
        let twcc = TransportLayerCc {
            media_ssrc: 200,
            packet_status_count: 10,
            recv_deltas: vec![],
            ..Default::default()
        };
        let q = sample_after(&stats, vec![Box::new(twcc)]);
        assert!(!q.twcc);
        assert_eq!(q.loss, 0.0);
    }
}
