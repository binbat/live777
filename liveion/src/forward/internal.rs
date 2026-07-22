use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use chrono::Utc;
#[cfg(feature = "cascade")]
use libwish::Client;
use tokio::sync::{Mutex, Notify, RwLock, broadcast, watch};
use tracing::trace;
use tracing::{debug, info, warn};

use crate::AppError;
#[cfg(feature = "source")]
use crate::config::ChannelConfig;
use crate::event::{Event, SessionStopReason};
use crate::forward::codec_compat::{is_av1_codec, is_h264_codec, is_h265_codec};
use crate::forward::message::{ForwardInfo, SessionInfo};
use crate::forward::rtcp::RtcpMessage;
use crate::result::Result;
use crate::{metrics, new_broadcast_channel};
use rtc::ice::mdns::MulticastDnsMode;
use rtc::peer_connection::configuration::interceptor_registry::{
    configure_nack, configure_rtcp_reports, configure_simulcast_extension_headers,
    configure_twcc_receiver_only, configure_twcc_sender_only,
};
use rtc::peer_connection::configuration::media_engine::{MIME_TYPE_OPUS, MIME_TYPE_VP8};
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use webrtc::data_channel::DataChannel;
use webrtc::media_stream::track_remote::TrackRemote;
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceGatheringState, RTCIceServer,
    RTCPeerConnectionState, Registry, SettingEngine,
};
use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit, RtpSender};

use super::RemovePeerOutcome;
use super::media::{MediaGenerationDecision, MediaInfo, MediaProfile};
use super::message::CascadeInfo;
use super::publish::PublishRTCPeerConnection;
use super::subscribe::SubscribeRTCPeerConnection;
use super::track::{PublishTrackRemote, SharedManualTwccFeedback};

const CLOSED_SESSION_TTL_MS: i64 = 30_000;

fn video_rtcp_feedback() -> Vec<rtc::rtp_transceiver::rtp_sender::RTCPFeedback> {
    vec![
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "goog-remb".to_string(),
            parameter: "".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "transport-cc".to_string(),
            parameter: "".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "ccm".to_string(),
            parameter: "fir".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "nack".to_string(),
            parameter: "".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "nack".to_string(),
            parameter: "pli".to_string(),
        },
    ]
}
fn ensure_video_rtcp_feedback(codec: &mut RTCRtpCodec) {
    for feedback in video_rtcp_feedback() {
        if !codec.rtcp_feedback.iter().any(|existing| {
            existing.typ == feedback.typ && existing.parameter == feedback.parameter
        }) {
            codec.rtcp_feedback.push(feedback);
        }
    }
}

fn format_codec_for_log(codec: &RTCRtpCodec) -> String {
    format!(
        "{}/{}/channels={}/fmtp={}",
        codec.mime_type, codec.clock_rate, codec.channels, codec.sdp_fmtp_line
    )
}

#[derive(Clone)]
struct DataChannelForward {
    publish: broadcast::Sender<Vec<u8>>,
    subscribe: broadcast::Sender<Vec<u8>>,
}

#[derive(Clone)]
struct PublishPeerHandler {
    internal: std::sync::Weak<PeerForwardInternal>,
    gather_complete: Arc<Notify>,
    connection_state_tx: watch::Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PublishPeerHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if let Some(internal) = self.internal.upgrade() {
            let pc = internal
                .publish_peer_ref
                .lock()
                .await
                .clone()
                .and_then(|w| w.upgrade());
            let _ = self.connection_state_tx.send(state);
            internal.send_event();

            if let Some(pc) = pc {
                info!(
                    "[{}] [publish] connection state changed: {}",
                    internal.stream, state
                );
                match state {
                    RTCPeerConnectionState::Failed => {
                        let _ = pc.close().await;
                    }
                    RTCPeerConnectionState::Disconnected => {
                        // ICE may recover on its own; surface the blind spot
                        // and bound the wait — the watchdog closes the peer
                        // if it never recovers.
                        warn!(
                            "[{}] [publish] connection disconnected; waiting for recovery or failure",
                            internal.stream
                        );
                        spawn_disconnected_watchdog(
                            internal.stream.clone(),
                            "publish",
                            &self.connection_state_tx,
                            &pc,
                        );
                    }
                    RTCPeerConnectionState::Closed => {
                        let _ = internal.remove_publish(pc).await;
                    }
                    _ => {}
                }
            }
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if let Some(internal) = self.internal.upgrade() {
            // Only accept tracks while a publish peer is still alive; a late
            // `on_track` from a torn-down publisher is dropped here. (A track
            // arriving mid-replace is attributed to the new session — see
            // `publish_session_id`.)
            let publish_peer_alive = internal
                .publish_peer_ref
                .lock()
                .await
                .as_ref()
                .and_then(|w| w.upgrade())
                .is_some();
            if publish_peer_alive {
                let _ = internal.publish_track_up(track).await;
            }
        }
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        if let Some(internal) = self.internal.upgrade() {
            let pc = internal
                .publish_peer_ref
                .lock()
                .await
                .clone()
                .and_then(|w| w.upgrade());
            if let Some(pc) = pc {
                let _ = internal.publish_data_channel(pc, dc).await;
            }
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("publish ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

pub(crate) const PUBLISH_CONNECTED_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(15);

pub(crate) async fn wait_for_peer_connected(
    mut peer_state_rx: watch::Receiver<RTCPeerConnectionState>,
    timeout: std::time::Duration,
    context: &str,
) -> Result<()> {
    let wait = async {
        loop {
            let state = *peer_state_rx.borrow_and_update();
            match state {
                RTCPeerConnectionState::Connected => return Ok(()),
                RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected => {
                    return Err(AppError::throw(format!(
                        "{context}: peer closed before Connected: state={state}"
                    )));
                }
                _ => {}
            }

            peer_state_rx.changed().await.map_err(|_| {
                AppError::throw(format!(
                    "{context}: peer connection state channel closed before Connected"
                ))
            })?;
        }
    };

    tokio::time::timeout(timeout, wait).await.map_err(|_| {
        AppError::throw(format!(
            "{context}: timed out waiting for PeerConnection Connected after {:?}",
            timeout
        ))
    })?
}

/// How long a peer may stay `Disconnected` before we stop waiting for ICE
/// recovery and close it (teardown then follows the normal `Closed` path).
const DISCONNECTED_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Give a `Disconnected` peer time to recover (transient network blip, ICE
/// restart); if it is still disconnected when the grace expires, close it so
/// teardown follows the normal `Closed` path instead of lingering as a
/// zombie session with dead forwarding loops.
fn spawn_disconnected_watchdog(
    stream: String,
    role: &'static str,
    connection_state_tx: &watch::Sender<RTCPeerConnectionState>,
    peer: &Arc<dyn PeerConnection>,
) {
    let mut state_rx = connection_state_tx.subscribe();
    let peer = Arc::downgrade(peer);
    tokio::spawn(async move {
        let wait = async {
            while matches!(*state_rx.borrow(), RTCPeerConnectionState::Disconnected) {
                if state_rx.changed().await.is_err() {
                    break;
                }
            }
        };
        // A timeout means the state never left Disconnected during the grace.
        if tokio::time::timeout(DISCONNECTED_CLOSE_TIMEOUT, wait)
            .await
            .is_err()
        {
            warn!(
                "[{}] [{}] connection still disconnected after {}s, closing",
                stream,
                role,
                DISCONNECTED_CLOSE_TIMEOUT.as_secs()
            );
            if let Some(peer) = peer.upgrade() {
                let _ = peer.close().await;
            }
        }
    });
}

#[derive(Clone)]
struct SubscribePeerHandler {
    internal: std::sync::Weak<PeerForwardInternal>,
    peer: Arc<Mutex<Option<std::sync::Weak<dyn PeerConnection>>>>,
    gather_complete: Arc<Notify>,
    connection_state_tx: watch::Sender<RTCPeerConnectionState>,
}

impl SubscribePeerHandler {
    fn new(
        internal: std::sync::Weak<PeerForwardInternal>,
        gather_complete: Arc<Notify>,
        connection_state_tx: watch::Sender<RTCPeerConnectionState>,
    ) -> Self {
        Self {
            internal,
            peer: Arc::new(Mutex::new(None)),
            gather_complete,
            connection_state_tx,
        }
    }

    async fn set_peer(&self, peer: std::sync::Weak<dyn PeerConnection>) {
        *self.peer.lock().await = Some(peer);
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SubscribePeerHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.connection_state_tx.send(state);
        let pc = self.peer.lock().await.clone().and_then(|w| w.upgrade());
        if let Some(internal) = self.internal.upgrade() {
            internal.send_event();
            if let Some(pc) = pc {
                info!(
                    "[{}] [subscribe] connection state changed: {}",
                    internal.stream, state
                );
                match state {
                    RTCPeerConnectionState::Failed => {
                        let _ = pc.close().await;
                    }
                    RTCPeerConnectionState::Disconnected => {
                        // ICE may recover on its own; surface the blind spot
                        // and bound the wait — the watchdog closes the peer
                        // if it never recovers.
                        warn!(
                            "[{}] [subscribe] connection disconnected; waiting for recovery or failure",
                            internal.stream
                        );
                        spawn_disconnected_watchdog(
                            internal.stream.clone(),
                            "subscribe",
                            &self.connection_state_tx,
                            &pc,
                        );
                    }
                    RTCPeerConnectionState::Closed => {
                        let _ = internal.remove_subscribe(pc).await;
                    }
                    _ => {}
                }
            }
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let pc = self.peer.lock().await.clone().and_then(|w| w.upgrade());
        if let (Some(internal), Some(_pc)) = (self.internal.upgrade(), pc) {
            let kind = track.kind().await;
            let ssrcs = track.ssrcs().await;
            info!(
                "[{}] [subscribe] on_track: kind={}, ssrcs={:?}, id={}",
                internal.stream,
                kind,
                ssrcs,
                track.track_id().await
            );
            // Subscribe peer is sendonly — incoming tracks from the remote
            // are unexpected but logged for debugging.
        }
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let pc = self.peer.lock().await.clone().and_then(|w| w.upgrade());
        if let (Some(internal), Some(pc)) = (self.internal.upgrade(), pc) {
            let _ = internal.subscribe_data_channel(pc, dc).await;
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("subscribe ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

// RtcpEgressProbeInterceptor: counts outgoing RTCP packet types from the
// interceptor chain (TwccReceiver, RtcpReports, NackResponder).  This probe
// answers the question: "is liveion really sending TransportLayerCC feedback
// back to the WHIP publisher?"  It is registered as the outermost interceptor
// layer in the publish peer chain so it sees every RTCP packet before it hits
// the wire (ICE/DTLS/SRTP).
mod rtcp_egress_probe {
    use rtc::interceptor::StreamInfo;
    use rtc::interceptor::{Packet, TaggedPacket};
    use rtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
    use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
    use rtc::rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate;
    use rtc::rtcp::receiver_report::ReceiverReport;
    use rtc::rtcp::sender_report::SenderReport;
    use rtc::rtcp::transport_feedbacks::transport_layer_cc::TransportLayerCc;
    use rtc::rtcp::transport_feedbacks::transport_layer_nack::TransportLayerNack;
    use rtc_shared::error::Error;
    use sansio::Protocol;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicU64;

    pub(crate) struct Counters {
        pub transport_layer_cc: AtomicU64,
        pub receiver_report: AtomicU64,
        pub sender_report: AtomicU64,
        pub pli: AtomicU64,
        pub fir: AtomicU64,
        pub nack: AtomicU64,
        pub remb: AtomicU64,
        pub poll_write_calls: AtomicU64,
        pub poll_write_non_empty: AtomicU64,
        pub poll_write_rtp: AtomicU64,
        pub other: AtomicU64,
        pub last_twcc_time: std::sync::Mutex<Option<std::time::Instant>>,
    }

    impl Counters {
        pub fn new() -> Self {
            Self {
                transport_layer_cc: AtomicU64::new(0),
                receiver_report: AtomicU64::new(0),
                sender_report: AtomicU64::new(0),
                pli: AtomicU64::new(0),
                fir: AtomicU64::new(0),
                nack: AtomicU64::new(0),
                remb: AtomicU64::new(0),
                poll_write_calls: AtomicU64::new(0),
                poll_write_non_empty: AtomicU64::new(0),
                poll_write_rtp: AtomicU64::new(0),
                other: AtomicU64::new(0),
                last_twcc_time: std::sync::Mutex::new(None),
            }
        }

        fn classify(pkt: &dyn rtc::rtcp::packet::Packet) -> Option<RtcpType> {
            let any = pkt.as_any();
            if let Some(twcc) = any.downcast_ref::<TransportLayerCc>() {
                Some(RtcpType::Twcc(TwccInfo {
                    sender_ssrc: twcc.sender_ssrc,
                    media_ssrc: twcc.media_ssrc,
                    base_seq: twcc.base_sequence_number,
                    status_count: twcc.packet_status_count,
                }))
            } else if any.downcast_ref::<ReceiverReport>().is_some() {
                Some(RtcpType::ReceiverReport)
            } else if any.downcast_ref::<SenderReport>().is_some() {
                Some(RtcpType::SenderReport)
            } else if any.downcast_ref::<PictureLossIndication>().is_some() {
                Some(RtcpType::Pli)
            } else if any.downcast_ref::<FullIntraRequest>().is_some() {
                Some(RtcpType::Fir)
            } else if any.downcast_ref::<TransportLayerNack>().is_some() {
                Some(RtcpType::Nack)
            } else if any
                .downcast_ref::<ReceiverEstimatedMaximumBitrate>()
                .is_some()
            {
                Some(RtcpType::Remb)
            } else {
                None
            }
        }

        pub fn tally(&self, pkts: &[Box<dyn rtc::rtcp::packet::Packet>], stream: &str) {
            for pkt in pkts {
                match Self::classify(pkt.as_ref()) {
                    Some(RtcpType::Twcc(info)) => {
                        let cnt = self
                            .transport_layer_cc
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            + 1;
                        *self.last_twcc_time.lock().unwrap() = Some(std::time::Instant::now());
                        tracing::trace!(
                            "[{}] [rtcp-egress-probe] outgoing TWCC feedback count={} sender_ssrc={} media_ssrc={} base_seq={} status_count={}",
                            stream,
                            cnt,
                            info.sender_ssrc,
                            info.media_ssrc,
                            info.base_seq,
                            info.status_count,
                        );
                    }
                    Some(RtcpType::ReceiverReport) => {
                        self.receiver_report
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Some(RtcpType::SenderReport) => {
                        self.sender_report
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Some(RtcpType::Pli) => {
                        self.pli.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Some(RtcpType::Fir) => {
                        self.fir.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Some(RtcpType::Nack) => {
                        self.nack.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Some(RtcpType::Remb) => {
                        self.remb.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    None => {
                        self.other
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        }

        pub fn snapshot(&self) -> EgressSnapshot {
            EgressSnapshot {
                twcc: self
                    .transport_layer_cc
                    .load(std::sync::atomic::Ordering::Relaxed),
                rr: self
                    .receiver_report
                    .load(std::sync::atomic::Ordering::Relaxed),
                sr: self
                    .sender_report
                    .load(std::sync::atomic::Ordering::Relaxed),
                pli: self.pli.load(std::sync::atomic::Ordering::Relaxed),
                fir: self.fir.load(std::sync::atomic::Ordering::Relaxed),
                nack: self.nack.load(std::sync::atomic::Ordering::Relaxed),
                remb: self.remb.load(std::sync::atomic::Ordering::Relaxed),
                poll_write_calls: self
                    .poll_write_calls
                    .load(std::sync::atomic::Ordering::Relaxed),
                poll_write_non_empty: self
                    .poll_write_non_empty
                    .load(std::sync::atomic::Ordering::Relaxed),
                poll_write_rtp: self
                    .poll_write_rtp
                    .load(std::sync::atomic::Ordering::Relaxed),
                other: self.other.load(std::sync::atomic::Ordering::Relaxed),
                last_twcc: self.last_twcc_time.lock().unwrap().map(|t| t.elapsed()),
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) struct EgressSnapshot {
        pub twcc: u64,
        pub rr: u64,
        pub sr: u64,
        pub pli: u64,
        pub fir: u64,
        pub nack: u64,
        pub remb: u64,
        pub poll_write_calls: u64,
        pub poll_write_non_empty: u64,
        pub poll_write_rtp: u64,
        pub other: u64,
        pub last_twcc: Option<std::time::Duration>,
    }

    enum RtcpType {
        Twcc(TwccInfo),
        ReceiverReport,
        SenderReport,
        Pli,
        Fir,
        Nack,
        Remb,
    }

    struct TwccInfo {
        sender_ssrc: u32,
        media_ssrc: u32,
        base_seq: u16,
        status_count: u16,
    }

    /// RtcpEgressProbe sits as the outermost interceptor in the publish
    /// peer chain.  Its poll_write sees every RTCP packet that inner
    /// interceptors (notably TwccReceiver) produce before the packets
    /// go to the network.
    ///
    /// This interceptor is implemented manually (without derive macros)
    /// because rtc-interceptor-derive's proc macros generate references to
    /// crate-internal paths (shared::error::Error, sansio::Protocol) that
    /// require sansio + rtc-interceptor as direct dependencies.  We already
    /// add those deps, but the generated code can still have path resolution
    /// issues in external crates.  The manual impl is minimal and avoids
    /// the derive dependency entirely.
    pub(crate) struct RtcpEgressProbe<P> {
        inner: P,
        stream: String,
        counters: std::sync::Arc<Counters>,
        native_twcc_bound: Arc<AtomicBool>,
    }

    impl<P> RtcpEgressProbe<P> {
        pub fn new(
            inner: P,
            stream: String,
            counters: std::sync::Arc<Counters>,
            native_twcc_bound: Arc<AtomicBool>,
        ) -> Self {
            Self {
                inner,
                stream,
                counters,
                native_twcc_bound,
            }
        }
    }

    // --- sansio::Protocol impl (manual delegation) ---
    impl<
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
    > Protocol<TaggedPacket, TaggedPacket, ()> for RtcpEgressProbe<P>
    {
        type Rout = TaggedPacket;
        type Wout = TaggedPacket;
        type Eout = ();
        type Time = std::time::Instant;
        type Error = Error;

        fn handle_read(&mut self, msg: TaggedPacket) -> Result<(), Error> {
            self.inner.handle_read(msg)
        }
        fn poll_read(&mut self) -> Option<TaggedPacket> {
            self.inner.poll_read()
        }
        fn handle_write(&mut self, msg: TaggedPacket) -> Result<(), Error> {
            self.inner.handle_write(msg)
        }
        fn poll_write(&mut self) -> Option<TaggedPacket> {
            let cnt = self
                .counters
                .poll_write_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            let out = self.inner.poll_write();
            if let Some(tagged) = &out {
                self.counters
                    .poll_write_non_empty
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match &tagged.message {
                    Packet::Rtcp(pkts) => {
                        self.counters.tally(pkts, &self.stream);
                    }
                    Packet::Rtp(_) => {
                        self.counters
                            .poll_write_rtp
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            // This probe is useful when debugging RTCP egress, but it is too noisy for normal
            // debug logs while streams are active.
            if cnt % 3000 == 1 {
                let snap = self.counters.snapshot();
                tracing::trace!(
                    "[{}] [rtcp-egress-probe] poll_write_calls={} non_empty={} rtp={} twcc={} rr={} sr={} pli={} fir={} nack={} remb={} other={}",
                    self.stream,
                    snap.poll_write_calls,
                    snap.poll_write_non_empty,
                    snap.poll_write_rtp,
                    snap.twcc,
                    snap.rr,
                    snap.sr,
                    snap.pli,
                    snap.fir,
                    snap.nack,
                    snap.remb,
                    snap.other,
                );
            }
            out
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
            self.inner.close()
        }
    }

    // --- Interceptor impl (manual delegation) ---
    impl<P: rtc::interceptor::Interceptor> rtc::interceptor::Interceptor for RtcpEgressProbe<P> {
        fn bind_local_stream(&mut self, info: &StreamInfo) {
            self.inner.bind_local_stream(info);
        }
        fn unbind_local_stream(&mut self, info: &StreamInfo) {
            self.inner.unbind_local_stream(info);
        }
        fn bind_remote_stream(&mut self, info: &StreamInfo) {
            let has_twcc = info
                .rtp_header_extensions
                .iter()
                .any(|e| e.uri.contains("transport-wide-cc"));
            if has_twcc {
                self.native_twcc_bound
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            tracing::trace!(
                "[{}] [rtcp-egress-probe] bind_remote_stream ssrc={} pt={} mime={} twcc_ext={} ext_count={}",
                self.stream,
                info.ssrc,
                info.payload_type,
                info.mime_type,
                has_twcc,
                info.rtp_header_extensions.len(),
            );
            self.inner.bind_remote_stream(info);
        }
        fn unbind_remote_stream(&mut self, info: &StreamInfo) {
            tracing::trace!(
                "[{}] [rtcp-egress-probe] unbind_remote_stream ssrc={}",
                self.stream,
                info.ssrc,
            );
            self.inner.unbind_remote_stream(info);
        }
    }
}

pub(crate) struct PeerForwardInternal {
    pub(crate) stream: String,
    create_at: i64,
    publish_leave_at: RwLock<i64>,
    subscribe_leave_at: RwLock<i64>,
    publish: RwLock<Option<PublishRTCPeerConnection>>,
    pub(crate) publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
    pub(crate) publish_tracks_change: broadcast::Sender<()>,
    pub(crate) publish_rtcp_channel: broadcast::Sender<(RtcpMessage, u32)>,
    pub(crate) subscribe_group: RwLock<Vec<SubscribeRTCPeerConnection>>,
    closed_publish_sessions: RwLock<Vec<SessionInfo>>,
    closed_subscribe_sessions: RwLock<Vec<SessionInfo>>,
    data_channel_forward: DataChannelForward,
    ice_server: Vec<RTCIceServer>,
    ice_udp_addrs: Vec<SocketAddr>,
    /// Manager-wide lifecycle event bus; owned by `stream::manager::Manager`
    /// and shared with every forward.
    lifecycle_sender: broadcast::Sender<Event>,
    /// Weak reference to the publish peer, set before signaling via `set_publish_peer_ref`
    publish_peer_ref: Mutex<Option<std::sync::Weak<dyn PeerConnection>>>,
    publish_peer_state_rx: Mutex<Option<watch::Receiver<RTCPeerConnectionState>>>,
    /// Negotiated transport-wide-cc RTP header extension ID (0 = unknown/missing)
    negotiated_twcc_ext_id: std::sync::atomic::AtomicU8,
    /// RTCP egress counters from the probe interceptor (None until publish peer created)
    rtcp_egress_counters: std::sync::Mutex<Option<std::sync::Arc<rtcp_egress_probe::Counters>>>,
    /// Set to `true` by the publish interceptor (`RtcpEgressProbe`) once native
    /// TWCC receiver binding is active on any remote stream. `PeerForwardInternal`
    /// never reads this field directly — the `Arc` is cloned into the interceptor
    /// and `SharedManualTwccFeedback` so they can coordinate native vs. manual TWCC
    /// across the publish interceptor chain and all per-track TWCC senders.
    native_twcc_bound: Arc<AtomicBool>,
    /// Shared manual TWCC fallback for all tracks in a publish peer.
    manual_twcc_feedback: std::sync::Mutex<Option<SharedManualTwccFeedback>>,
    media_generation_id: RwLock<u64>,
    last_publish_profile: RwLock<Option<MediaProfile>>,
    #[cfg(feature = "source")]
    channel: Option<ChannelConfig>,
    /// Effective strategy for this stream (global strategy merged with any
    /// per-stream override).
    pub(crate) strategy: api::strategy::Strategy,
}

impl PeerForwardInternal {
    pub(crate) fn new(
        stream: impl ToString,
        ice_server: Vec<RTCIceServer>,
        ice_udp_addrs: Vec<SocketAddr>,
        #[cfg(feature = "source")] channel: Option<ChannelConfig>,
        strategy: api::strategy::Strategy,
        lifecycle_sender: broadcast::Sender<Event>,
    ) -> Self {
        PeerForwardInternal {
            stream: stream.to_string(),
            create_at: Utc::now().timestamp_millis(),
            publish_leave_at: RwLock::new(0),
            subscribe_leave_at: RwLock::new(Utc::now().timestamp_millis()),
            publish: RwLock::new(None),
            publish_tracks: Arc::new(RwLock::new(Vec::new())),
            publish_tracks_change: new_broadcast_channel!(16),
            publish_rtcp_channel: new_broadcast_channel!(48),
            subscribe_group: RwLock::new(Vec::new()),
            closed_publish_sessions: RwLock::new(Vec::new()),
            closed_subscribe_sessions: RwLock::new(Vec::new()),
            data_channel_forward: DataChannelForward {
                publish: new_broadcast_channel!(1024),
                subscribe: new_broadcast_channel!(1024),
            },
            ice_server,
            ice_udp_addrs,
            lifecycle_sender,
            publish_peer_ref: Mutex::new(None),
            publish_peer_state_rx: Mutex::new(None),
            negotiated_twcc_ext_id: AtomicU8::new(0),
            rtcp_egress_counters: std::sync::Mutex::new(None),
            native_twcc_bound: Arc::new(AtomicBool::new(false)),
            manual_twcc_feedback: std::sync::Mutex::new(None),
            media_generation_id: RwLock::new(0),
            last_publish_profile: RwLock::new(None),
            #[cfg(feature = "source")]
            channel,
            strategy,
        }
    }

    pub(crate) async fn info(&self) -> ForwardInfo {
        let mut subscribe_session_infos = vec![];
        let subscribe_group = self.subscribe_group.read().await;

        for subscribe in subscribe_group.iter() {
            subscribe_session_infos.push(subscribe.info().await);
        }

        let publish_tracks = self.publish_tracks.read().await;

        #[cfg(feature = "source")]
        let has_virtual_publisher = publish_tracks
            .iter()
            .any(|track| matches!(track, PublishTrackRemote::Virtual(_)));

        #[cfg(not(feature = "source"))]
        let has_virtual_publisher = false;

        let publish_session_info = match self.publish.read().await.as_ref() {
            Some(publish) => Some(publish.info().await),
            None => None,
        };

        let effective_publish_session_info =
            if publish_session_info.is_none() && has_virtual_publisher {
                Some(SessionInfo {
                    id: "virtual-source".to_string(),
                    create_at: self.create_at,
                    leave_at: 0,
                    state: RTCPeerConnectionState::Connected,
                    cascade: None,
                    has_data_channel: false,
                })
            } else {
                publish_session_info
            };

        let now = Utc::now().timestamp_millis();
        let closed_publish_sessions = self.closed_publish_sessions.read().await;
        let recent_closed_publish: Vec<SessionInfo> = closed_publish_sessions
            .iter()
            .filter(|s| now - s.leave_at < CLOSED_SESSION_TTL_MS)
            .cloned()
            .collect();
        drop(closed_publish_sessions);

        let closed_subscribe_sessions = self.closed_subscribe_sessions.read().await;
        let recent_closed_subscribe: Vec<SessionInfo> = closed_subscribe_sessions
            .iter()
            .filter(|s| now - s.leave_at < CLOSED_SESSION_TTL_MS)
            .cloned()
            .collect();
        drop(closed_subscribe_sessions);

        for closed in recent_closed_subscribe.iter() {
            if !subscribe_session_infos.iter().any(|s| s.id == closed.id) {
                subscribe_session_infos.push(closed.clone());
            }
        }

        let final_publish_session_info = effective_publish_session_info.or_else(|| {
            recent_closed_publish
                .iter()
                .max_by_key(|s| s.leave_at)
                .cloned()
        });

        ForwardInfo {
            id: self.stream.clone(),
            create_at: self.create_at,
            publish_leave_at: *self.publish_leave_at.read().await,
            subscribe_leave_at: *self.subscribe_leave_at.read().await,
            publish_session_info: final_publish_session_info,
            subscribe_session_infos,
            codecs: publish_tracks.iter().map(|track| track.codec()).collect(),
            has_virtual_publisher,
        }
    }

    pub(crate) async fn add_ice_candidate(
        &self,
        id: String,
        ice_candidates: Vec<RTCIceCandidateInit>,
    ) -> Result<()> {
        trace!(
            "Adding {} ICE candidates for session {}",
            ice_candidates.len(),
            id
        );

        let publish = self.publish.read().await;
        if publish.is_some() && publish.as_ref().unwrap().id == id {
            let publish = publish.as_ref().unwrap();
            for ice_candidate in ice_candidates {
                publish.peer.add_ice_candidate(ice_candidate).await?;
            }
            return Ok(());
        }
        drop(publish);

        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == id {
                for ice_candidate in ice_candidates {
                    subscribe.peer.add_ice_candidate(ice_candidate).await?;
                }
                return Ok(());
            }
        }

        Ok(())
    }

    /// Millisecond timestamp of the last time the subscriber group became
    /// empty: the creation time for a fresh forward, 0 while at least one
    /// subscriber is attached.
    pub(crate) async fn subscribe_leave_at(&self) -> i64 {
        *self.subscribe_leave_at.read().await
    }

    pub(crate) async fn has_subscribers(&self) -> bool {
        !self.subscribe_group.read().await.is_empty()
    }

    /// A virtual (source) publisher keeps the stream alive even though
    /// `publish` is `None`.
    #[cfg(feature = "source")]
    pub(crate) async fn has_virtual_publisher(&self) -> bool {
        self.publish_tracks
            .read()
            .await
            .iter()
            .any(|track| matches!(track, PublishTrackRemote::Virtual(_)))
    }

    /// Remove a peer by session id. The returned [`RemovePeerOutcome`] tells
    /// the caller whether the whole stream should be torn down:
    /// - `PublisherRemoved`: the removed session was the publisher.
    /// - `Orphaned`: the removed session was the last remaining session of a
    ///   stream whose publisher is already gone (e.g. after an ungraceful
    ///   publisher disconnect) — without teardown the stream entry would
    ///   linger forever. This is only a hint: a virtual (source) publisher
    ///   never counts as gone, and a new publish may be mid-handshake, so
    ///   the caller must re-check with `PeerForward::confirm_orphan_teardown`
    ///   before deleting the stream.
    /// - `None`: a subscriber left a stream that still has a publisher or
    ///   other subscribers (or the session was not found) — the stream lives
    ///   on.
    pub(crate) async fn remove_peer(&self, id: String) -> Result<RemovePeerOutcome> {
        // ── Publish: atomic check-and-take under write lock ──
        {
            let mut publish = self.publish.write().await;
            if let Some(ref p) = *publish
                && p.id == id
            {
                let mut session_info = p.info().await;
                session_info.state = RTCPeerConnectionState::Closed;
                session_info.leave_at = Utc::now().timestamp_millis();
                let old = publish.take().unwrap();
                drop(publish);
                let _ = old.peer.close().await;
                self.do_remove_publish_cleanup(session_info, SessionStopReason::ApiDeleted)
                    .await;
                return Ok(RemovePeerOutcome::PublisherRemoved);
            }
        }

        // ── Subscribe: atomic check-and-remove under write lock ──
        {
            let mut subscribe_group = self.subscribe_group.write().await;
            let pos = subscribe_group.iter().position(|s| s.id == id);
            if let Some(i) = pos {
                let old = subscribe_group.remove(i);
                let is_empty = subscribe_group.is_empty();
                drop(subscribe_group);
                if is_empty {
                    *self.subscribe_leave_at.write().await = Utc::now().timestamp_millis();
                }
                self.do_remove_subscribe_cleanup(&old, SessionStopReason::ApiDeleted)
                    .await;
                let _ = old.peer.close().await;
                // Orphan hint: with the publisher already gone and no
                // sessions left, the stream entry would linger forever.
                // The caller re-confirms (virtual publisher, in-flight
                // publish setup, new subscribers) before tearing down.
                if is_empty && self.publish.read().await.is_none() {
                    return Ok(RemovePeerOutcome::Orphaned);
                }
                return Ok(RemovePeerOutcome::None);
            }
        }

        Ok(RemovePeerOutcome::None)
    }

    pub(crate) async fn close(&self) -> Result<()> {
        let publish = self.publish.read().await;
        let subscribe_group = self.subscribe_group.read().await;

        if publish.is_some() {
            publish.as_ref().unwrap().peer.close().await?;
        }

        for subscribe in subscribe_group.iter() {
            subscribe.peer.close().await?;
        }

        info!("{} close", self.stream);
        Ok(())
    }

    /// Initialize the UDP <-> DataChannel bridge for this stream, if configured.
    #[cfg(feature = "source")]
    pub(crate) async fn try_init_udp_channel(&self) -> Result<()> {
        if let Some(stream_cfg) = self.channel.clone() {
            let dc_rx = self.data_channel_forward.publish.subscribe();
            let dc_tx = self.data_channel_forward.subscribe.clone();
            super::channel::spawn_channel(self.stream.clone(), dc_rx, dc_tx, stream_cfg).await?;
        }
        Ok(())
    }

    fn data_channel_forward(
        dc: Arc<dyn DataChannel>,
        sender: broadcast::Sender<Vec<u8>>,
        receiver: broadcast::Receiver<Vec<u8>>,
        connected_gate: Option<watch::Receiver<RTCPeerConnectionState>>,
    ) {
        let dc_rx = dc.clone();
        let dc_tx = dc.clone();

        tokio::spawn(async move {
            loop {
                match dc_rx.poll().await {
                    Some(webrtc::data_channel::DataChannelEvent::OnMessage(data)) => {
                        if let Err(err) = sender.send(data.data.to_vec()) {
                            debug!("send data channel err: {}", err);
                            return;
                        }
                    }
                    Some(webrtc::data_channel::DataChannelEvent::OnOpen) => {
                        debug!("Data channel opened");
                    }
                    Some(webrtc::data_channel::DataChannelEvent::OnClose) | None => {
                        info!("Datachannel closed; Exit the read_loop");
                        return;
                    }
                    _ => {}
                }
            }
        });

        tokio::spawn(async move {
            let mut receiver = receiver;
            let mut connected = connected_gate.is_none();
            loop {
                let msg = match receiver.recv().await {
                    Ok(msg) => msg,
                    // Data-channel messages must not silently stop
                    // forwarding on a lag burst.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("data channel receiver lagged, dropped {} messages", n);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if !connected {
                    if let Some(gate) = connected_gate.clone()
                        && let Err(err) = wait_for_peer_connected(
                            gate,
                            PUBLISH_CONNECTED_TIMEOUT,
                            "publish data channel send",
                        )
                        .await
                    {
                        info!(
                            "publish data channel send stopped before Connected: {:?}",
                            err
                        );
                        return;
                    }
                    connected = true;
                }
                if let Err(err) = dc_tx.send(bytes::BytesMut::from(&msg[..])).await {
                    info!("write data channel err: {}", err);
                    return;
                }
            }
        });
    }
}

// publish
impl PeerForwardInternal {
    pub(crate) async fn publish_is_some(&self) -> bool {
        let publish = self.publish.read().await;
        publish.is_some()
    }

    /// Session ID of the current publisher, empty when none is attached. A
    /// late `on_track` from a publisher that was just replaced is attributed
    /// to the *new* session (or to none) — cosmetic only, the ID is used for
    /// logging.
    async fn publish_session_id(&self) -> String {
        self.publish
            .read()
            .await
            .as_ref()
            .map(|p| p.id.clone())
            .unwrap_or_default()
    }

    pub(crate) async fn decide_publish_generation(
        &self,
        next_profile: &MediaProfile,
    ) -> MediaGenerationDecision {
        let current_generation_id = *self.media_generation_id.read().await;
        let last_profile = self.last_publish_profile.read().await;
        MediaGenerationDecision::decide(current_generation_id, last_profile.as_ref(), next_profile)
    }

    pub(crate) async fn apply_publish_generation(
        &self,
        decision: &MediaGenerationDecision,
        next_profile: MediaProfile,
    ) -> Result<()> {
        let old_generation_id = *self.media_generation_id.read().await;

        if decision.changed {
            info!(
                "[{}] publisher restarted with incompatible codec, rebuilding media generation; old profile: {:?}; new profile: {:?}; generation changed: {} -> {}",
                self.stream,
                *self.last_publish_profile.read().await,
                next_profile,
                old_generation_id,
                decision.generation_id,
            );
            self.close_subscribers_for_generation_change().await?;
        } else if self.last_publish_profile.read().await.is_some() {
            info!(
                "[{}] publisher restarted with same codec, reusing media generation {}",
                self.stream, decision.generation_id
            );
        }

        *self.media_generation_id.write().await = decision.generation_id;
        *self.last_publish_profile.write().await = Some(next_profile);
        Ok(())
    }

    async fn close_subscribers_for_generation_change(&self) -> Result<()> {
        let subscribers = {
            self.subscribe_group
                .read()
                .await
                .iter()
                .map(|subscribe| (subscribe.id.clone(), subscribe.peer.clone()))
                .collect::<Vec<_>>()
        };

        for (id, peer) in subscribers {
            info!(
                "[{}] subscriber session marked stale because codec changed: {}",
                self.stream, id
            );
            peer.close().await?;
        }

        Ok(())
    }

    /// Register a publish session. Returns the new session ID on success.
    pub(crate) async fn set_publish(
        &self,
        peer: Arc<dyn PeerConnection>,
        cascade: Option<CascadeInfo>,
        connection_state_rx: watch::Receiver<RTCPeerConnectionState>,
    ) -> Result<String> {
        let session_id = {
            let mut publish = self.publish.write().await;
            if publish.is_some() {
                return Err(AppError::stream_already_exists(
                    "A connection has already been established",
                ));
            }

            let publish_peer = PublishRTCPeerConnection::new(
                self.stream.clone(),
                peer.clone(),
                self.publish_rtcp_channel.subscribe(),
                cascade,
                connection_state_rx,
            )
            .await?;

            info!("[{}] [publish] set {}", self.stream, publish_peer.id);
            let session_id = publish_peer.id.clone();
            *publish = Some(publish_peer);
            session_id
        };

        {
            let mut publish_leave_at = self.publish_leave_at.write().await;
            *publish_leave_at = 0;
        }

        metrics::PUBLISH.inc();
        self.emit(Event::PublishStarted {
            stream: self.stream.clone(),
            session: session_id.clone(),
        });
        self.send_event();

        Ok(session_id)
    }

    pub(crate) async fn remove_publish(&self, peer: Arc<dyn PeerConnection>) -> Result<()> {
        let closed_session = {
            let mut publish = self.publish.write().await;
            if publish.is_none() {
                // Already removed (e.g. the Closed callback fired before this
                // manual call). Treat as success so callers don't see a spurious
                // error from a benign double-remove.
                return Ok(());
            }

            if !Arc::ptr_eq(&publish.as_ref().unwrap().peer, &peer) {
                return Err(AppError::throw("publish not myself"));
            }

            let mut session_info = publish.as_ref().unwrap().info().await;
            session_info.state = RTCPeerConnectionState::Closed;
            session_info.leave_at = Utc::now().timestamp_millis();
            *publish = None;
            session_info
        };

        self.do_remove_publish_cleanup(closed_session, SessionStopReason::PeerClosed)
            .await;
        Ok(())
    }

    /// Shared cleanup after a publish session has been taken out of `self.publish`.
    /// Callers must have already set `self.publish` to `None` and prepared the
    /// `SessionInfo` with the final state and `leave_at` timestamp.
    async fn do_remove_publish_cleanup(
        &self,
        closed_session: SessionInfo,
        reason: SessionStopReason,
    ) {
        *self.publish_peer_state_rx.lock().await = None;
        // Drop the weak peer ref as well: while it is set, a late `on_track`
        // from the torn-down publisher would still pass the liveness check
        // in `PublishPeerHandler::on_track`.
        *self.publish_peer_ref.lock().await = None;

        {
            let mut publish_tracks = self.publish_tracks.write().await;
            publish_tracks.clear();
            let _ = self.publish_tracks_change.send(());
        }

        {
            let mut publish_leave_at = self.publish_leave_at.write().await;
            *publish_leave_at = Utc::now().timestamp_millis();
        }

        let session_id = closed_session.id.clone();
        {
            let mut closed_publish_sessions = self.closed_publish_sessions.write().await;
            closed_publish_sessions.push(closed_session);
        }

        info!("[{}] [publish] set none", self.stream);
        metrics::PUBLISH.dec();
        self.emit(Event::PublishStopped {
            stream: self.stream.clone(),
            session: session_id,
            reason,
        });

        self.send_event();
    }

    pub async fn publish_is_svc(&self) -> bool {
        let publish = self.publish.read().await;
        if publish.is_none() {
            return false;
        }
        publish.as_ref().unwrap().media_info.video_transceiver.2
    }

    pub async fn publish_svc_rids(&self) -> Result<Vec<String>> {
        let publish_tracks = self.publish_tracks.read().await;
        let rids = publish_tracks
            .iter()
            .filter(|t| t.kind() == RtpCodecKind::Video)
            .map(|t| t.rid().to_string())
            .collect::<Vec<_>>();
        Ok(rids)
    }

    pub(crate) async fn publisher_codec(&self, kind: RtpCodecKind) -> Option<RTCRtpCodec> {
        {
            let publish_tracks = self.publish_tracks.read().await;
            for t in publish_tracks.iter() {
                if t.kind() == kind {
                    return Some(match t {
                        PublishTrackRemote::Real { track, .. } => {
                            let ssrcs = track.ssrcs().await;
                            let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                            track.codec(first_ssrc).await.unwrap_or_default()
                        }
                        #[cfg(feature = "source")]
                        PublishTrackRemote::Virtual(v) => v.codec_params.rtp_codec.clone(),
                    });
                }
            }
        }

        let publish = self.publish.read().await;
        publish
            .as_ref()
            .and_then(|publish| publish.media_info.codec_for_kind(kind))
    }

    pub(crate) async fn new_publish_peer(
        &self,
        media_info: MediaInfo,
        internal_weak: std::sync::Weak<PeerForwardInternal>,
    ) -> Result<(
        Arc<dyn PeerConnection>,
        Arc<Notify>,
        watch::Receiver<RTCPeerConnectionState>,
    )> {
        if media_info.video_transceiver.0 > 1 && media_info.audio_transceiver.0 > 1 {
            return Err(AppError::throw("sendonly is more than 1"));
        }

        let mut m = MediaEngine::default();
        m.register_default_codecs()?;

        let registry = Registry::new();
        let registry = configure_nack(registry, &mut m);
        let registry = configure_rtcp_reports(registry);
        configure_simulcast_extension_headers(&mut m)?;
        // WHIP publishers need a clear upstream BWE contract: the browser adds
        // transport-wide sequence numbers and liveion returns TWCC feedback for
        // the publisher->liveion path. Downstream WHEP TWCC/REMB/RR is not
        // bridged back because it describes a different network path.
        let registry = configure_twcc_receiver_only(registry, &mut m)?;

        // Wrap the interceptor chain with an RTCP egress probe.  This probe
        // is the outermost layer, so it sees every RTCP packet produced by
        // inner interceptors (TwccReceiver, RtcpReports, NackChain) before
        // the packets are written to the network.
        let stream = self.stream.clone();
        let egress_counters = std::sync::Arc::new(rtcp_egress_probe::Counters::new());
        let c = egress_counters.clone();
        let native_twcc_bound = self.native_twcc_bound.clone();
        native_twcc_bound.store(false, Ordering::Relaxed);
        *self.manual_twcc_feedback.lock().unwrap() = None;
        let registry = registry.with(move |inner| {
            rtcp_egress_probe::RtcpEgressProbe::new(inner, stream, c, native_twcc_bound)
        });
        *self.rtcp_egress_counters.lock().unwrap() = Some(egress_counters);

        let mut s = SettingEngine::default();
        s.set_multicast_dns_mode(MulticastDnsMode::Disabled);

        let ice_servers = self.ice_server.clone();
        info!(
            "ICE servers for publish: count={}, urls=[{}], has_username={}, has_credential={}",
            ice_servers.len(),
            ice_servers
                .iter()
                .flat_map(|s| s.urls.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            ice_servers.iter().any(|s| !s.username.is_empty()),
            ice_servers.iter().any(|s| !s.credential.is_empty()),
        );

        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(ice_servers)
            .build();

        let gather_complete = Arc::new(Notify::new());
        let (connection_state_tx, connection_state_rx) =
            watch::channel(RTCPeerConnectionState::New);
        *self.publish_peer_state_rx.lock().await = Some(connection_state_rx.clone());
        let handler = PublishPeerHandler {
            internal: internal_weak,
            gather_complete: gather_complete.clone(),
            connection_state_tx,
        };
        let peer: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::<std::net::SocketAddr>::new()
                .with_media_engine(m)
                .with_interceptor_registry(registry)
                .with_setting_engine(s)
                .with_handler(Arc::new(handler))
                .with_udp_addrs(self.ice_udp_addrs.clone())
                .with_configuration(config)
                .build()
                .await?,
        );
        // Store weak ref so the handler can find the peer during events
        *self.publish_peer_ref.lock().await = Some(Arc::downgrade(&peer));

        let mut transceiver_kinds = vec![];
        if media_info.video_transceiver.0 > 0 {
            transceiver_kinds.push(RtpCodecKind::Video);
        }
        if media_info.audio_transceiver.0 > 0 {
            transceiver_kinds.push(RtpCodecKind::Audio);
        }

        for kind in transceiver_kinds {
            let _ = peer
                .add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        streams: vec![],
                        send_encodings: Vec::new(),
                    }),
                )
                .await?;
        }

        Ok((peer, gather_complete, connection_state_rx))
    }

    pub(crate) fn set_twcc_ext_id(&self, ext_id: u8) {
        self.negotiated_twcc_ext_id.store(ext_id, Ordering::Relaxed);
    }

    pub(crate) async fn publish_track_up(&self, track: Arc<dyn TrackRemote>) -> Result<()> {
        let generation_id = *self.media_generation_id.read().await;
        let twcc_ext_id = self.negotiated_twcc_ext_id.load(Ordering::Relaxed);
        let manual_twcc_feedback = if twcc_ext_id != 0 {
            let mut shared = self.manual_twcc_feedback.lock().unwrap();
            Some(
                shared
                    .get_or_insert_with(|| {
                        SharedManualTwccFeedback::new(
                            twcc_ext_id,
                            Some(self.native_twcc_bound.clone()),
                        )
                    })
                    .clone(),
            )
        } else {
            None
        };
        let publish_track_remote = PublishTrackRemote::new(
            self.stream.clone(),
            self.publish_session_id().await,
            track,
            self.publish_peer_state_rx.lock().await.clone(),
            twcc_ext_id,
            self.native_twcc_bound.clone(),
            manual_twcc_feedback,
            generation_id,
        )
        .await;

        let mut publish_tracks = self.publish_tracks.write().await;
        publish_tracks.push(publish_track_remote);
        publish_tracks.sort_by(|a, b| a.rid().cmp(b.rid()));

        let _ = self.publish_tracks_change.send(());

        Ok(())
    }

    pub(crate) async fn publish_data_channel(
        &self,
        _peer: Arc<dyn PeerConnection>,
        dc: Arc<dyn DataChannel>,
    ) -> Result<()> {
        let sender = self.data_channel_forward.subscribe.clone();
        let receiver = self.data_channel_forward.publish.subscribe();
        let connection_state_rx = self.publish_peer_state_rx.lock().await.clone();
        Self::data_channel_forward(dc, sender, receiver, connection_state_rx);
        Ok(())
    }

    #[cfg(feature = "recorder")]
    pub(crate) async fn first_publish_video_codec(&self) -> Option<String> {
        let publish_tracks = self.publish_tracks.read().await;
        for t in publish_tracks.iter() {
            if t.kind() == RtpCodecKind::Video {
                let c = t.codec();
                return Some(format!(
                    "{}/{}",
                    c.kind.to_lowercase(),
                    c.codec.to_lowercase()
                ));
            }
        }
        None
    }

    #[cfg(any(feature = "recorder", feature = "rtsp"))]
    pub(crate) fn subscribe_publish_tracks_change(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.publish_tracks_change.subscribe()
    }

    #[cfg(feature = "recorder")]
    pub(crate) async fn first_video_track(&self) -> Option<Arc<dyn TrackRemote>> {
        let publish_tracks = self.publish_tracks.read().await;
        publish_tracks.iter().find_map(|track| match track {
            PublishTrackRemote::Real { track, kind, .. } if *kind == RtpCodecKind::Video => {
                Some(track.clone())
            }
            _ => None,
        })
    }

    #[cfg(feature = "recorder")]
    pub(crate) async fn send_rtcp_to_publish(
        &self,
        message: crate::forward::rtcp::RtcpMessage,
        ssrc: u32,
    ) -> Result<()> {
        if self.publish_rtcp_channel.send((message, ssrc)).is_err() {
            return Err(crate::error::AppError::throw("Failed to send RTCP message"));
        }
        Ok(())
    }
}

// subscribe
impl PeerForwardInternal {
    pub(crate) async fn new_subscription_peer(
        &self,
        media_info: MediaInfo,
        internal_weak: std::sync::Weak<PeerForwardInternal>,
    ) -> Result<(
        Arc<dyn PeerConnection>,
        Arc<Notify>,
        watch::Receiver<RTCPeerConnectionState>,
        String,
    )> {
        if media_info.video_transceiver.1 > 1 && media_info.audio_transceiver.1 > 1 {
            return Err(AppError::throw("recvonly is more than 1"));
        }

        let mut m = MediaEngine::default();
        m.register_default_codecs()?;

        let registry = Registry::new();
        let registry = configure_nack(registry, &mut m);
        let registry = configure_rtcp_reports(registry);
        configure_simulcast_extension_headers(&mut m)?;
        let registry = configure_twcc_sender_only(registry, &mut m)?;

        let mut s = SettingEngine::default();
        s.set_multicast_dns_mode(MulticastDnsMode::Disabled);

        let ice_servers = self.ice_server.clone();
        info!(
            "ICE servers for subscribe: count={}, urls=[{}], has_username={}, has_credential={}",
            ice_servers.len(),
            ice_servers
                .iter()
                .flat_map(|s| s.urls.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            ice_servers.iter().any(|s| !s.username.is_empty()),
            ice_servers.iter().any(|s| !s.credential.is_empty()),
        );

        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(ice_servers)
            .build();

        let gather_complete = Arc::new(Notify::new());
        let (connection_state_tx, connection_state_rx) =
            watch::channel(RTCPeerConnectionState::New);
        let handler =
            SubscribePeerHandler::new(internal_weak, gather_complete.clone(), connection_state_tx);
        let peer: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::<std::net::SocketAddr>::new()
                .with_media_engine(m)
                .with_interceptor_registry(registry)
                .with_setting_engine(s)
                .with_handler(Arc::new(handler.clone()))
                .with_udp_addrs(self.ice_udp_addrs.clone())
                .with_configuration(config)
                .build()
                .await?,
        );
        handler.set_peer(Arc::downgrade(&peer)).await;

        // The session ID is allocated up front (not in `add_subscribe`) so
        // the sender-setup logs below can already be attributed to it.
        let session_id = uuid::Uuid::new_v4().to_string();

        // Use the publisher's negotiated codec for the subscriber's sender encoding.
        // This ensures the encoding codec matches what the publisher is actually sending,
        // so the rtc-layer write_rtp uses the correct payload type.
        let video_codec = self.publisher_codec(RtpCodecKind::Video).await;
        let audio_codec = self.publisher_codec(RtpCodecKind::Audio).await;

        Self::new_sender(
            &self.stream,
            &session_id,
            &peer,
            RtpCodecKind::Video,
            media_info.video_transceiver.1,
            video_codec,
        )
        .await?;
        Self::new_sender(
            &self.stream,
            &session_id,
            &peer,
            RtpCodecKind::Audio,
            media_info.audio_transceiver.1,
            audio_codec,
        )
        .await?;

        Ok((peer, gather_complete, connection_state_rx, session_id))
    }

    async fn new_sender(
        stream: &str,
        session: &str,
        peer: &Arc<dyn PeerConnection>,
        kind: RtpCodecKind,
        recv_sender: u8,
        publisher_codec: Option<RTCRtpCodec>,
    ) -> Result<Option<Arc<dyn RtpSender>>> {
        Ok(if recv_sender > 0 {
            let default_codec = if kind == RtpCodecKind::Video {
                RTCRtpCodec {
                    mime_type: MIME_TYPE_VP8.to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "".to_owned(),
                    rtcp_feedback: video_rtcp_feedback(),
                }
            } else {
                RTCRtpCodec {
                    mime_type: MIME_TYPE_OPUS.to_owned(),
                    clock_rate: 48000,
                    channels: 2,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                    rtcp_feedback: vec![],
                }
            };
            // Use the publisher's negotiated codec if available, otherwise fall back to default.
            // This ensures the subscriber's sender encoding codec matches the publisher's codec,
            // so the rtc-layer write_rtp uses the correct payload type.
            let mut codec = publisher_codec.unwrap_or(default_codec);

            // Clean up the publisher codec to match WHIP behaviour:
            // 1. Zero the H264 profile-level-id constraint byte so it
            //    matches Chrome's offer (e.g. "42C01F" → "42001f").
            // 2. Strip H264 sprop-parameter-sets because webrtc-rs will
            //    include them in the SDP answer from the sender's track
            //    codec, and when they differ from the MediaEngine defaults
            //    Chrome may reject the codec.  sprop is re-injected into
            //    the answer SDP by inject_publisher_sprop.
            // 3. H265 sprop-vps/sps/pps are kept — webrtc-rs needs them
            //    for sender codec matching.
            //    Chrome receives SPS/PPS/VPS in-band from the repayloader.
            if is_h264_codec(&codec) || is_h265_codec(&codec) || is_av1_codec(&codec) {
                let sprop_keys: &[&str] = if is_h264_codec(&codec) {
                    &["sprop-parameter-sets"]
                } else {
                    // H265: keep sprop-vps/sps/pps — they are required
                    // for webrtc-rs sender codec matching.
                    &[]
                };

                let plid_key: Option<&str> = if is_h264_codec(&codec) {
                    Some("profile-level-id")
                } else {
                    None
                };

                codec.sdp_fmtp_line = codec
                    .sdp_fmtp_line
                    .split(';')
                    .filter_map(|part| {
                        let trimmed = part.trim();
                        if trimmed.is_empty() {
                            return None;
                        }
                        if let Some((k, _)) = trimmed.split_once('=') {
                            let key = k.trim();
                            // Strip sprop keys.
                            if sprop_keys.iter().any(|sk| key.eq_ignore_ascii_case(sk)) {
                                return None;
                            }
                            // Normalize H264 profile-level-id.
                            if let Some(plk) = plid_key
                                && key.eq_ignore_ascii_case(plk)
                                && trimmed.len() >= plk.len() + 7
                            {
                                let val = &trimmed[plk.len() + 1..];
                                if val.len() == 6 {
                                    let normalized = format!("{}00{}", &val[0..2], &val[4..6])
                                        .to_ascii_lowercase();
                                    return Some(format!("{}={}", key, normalized));
                                }
                            }
                        }
                        Some(trimmed.to_owned())
                    })
                    .collect::<Vec<_>>()
                    .join(";");
            }

            if kind == RtpCodecKind::Video {
                ensure_video_rtcp_feedback(&mut codec);
            }

            // Log the codec that will be used to create the transceiver.
            info!(
                "[{}] [{}] creating sender transceiver with codec: {}",
                stream,
                session,
                format_codec_for_log(&codec)
            );

            // Use a single SSRC for both the encoding and the track.
            // The rtc-layer write_rtp validates that packet.ssrc matches sender.track().ssrcs(),
            // so the track's SSRC must match the encoding's SSRC.
            let ssrc = rand::random::<u32>();

            let transceiver = peer
                .add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        streams: vec![],
                        send_encodings: vec![RTCRtpEncodingParameters {
                            rtp_coding_parameters: RTCRtpCodingParameters {
                                ssrc: Some(ssrc),
                                ..Default::default()
                            },
                            codec: codec.clone(),
                            ..Default::default()
                        }],
                    }),
                )
                .await?;

            let sender = transceiver
                .sender()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get sender: {}", e))?
                .ok_or_else(|| anyhow::anyhow!("No sender found"))?;

            let params = sender
                .get_parameters()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get parameters: {}", e))?;
            let sender_track = sender.track();
            let track_codec = sender_track.codec(ssrc).await;
            info!(
                "[{}] [{}] new sender, kind={}, ssrc={}, requested_codec={}, track_codec={}",
                stream,
                session,
                kind,
                params
                    .encodings
                    .first()
                    .map(|e| e.rtp_coding_parameters.ssrc.unwrap_or(0))
                    .unwrap_or(0),
                format_codec_for_log(&codec),
                track_codec
                    .as_ref()
                    .map(format_codec_for_log)
                    .unwrap_or_else(|| "<none>".to_string()),
            );

            Some(sender)
        } else {
            None
        })
    }

    /// Register a subscribe session. `session_id` is allocated by
    /// [`Self::new_subscription_peer`] and returned on success.
    pub async fn add_subscribe(
        &self,
        peer: Arc<dyn PeerConnection>,
        cascade: Option<CascadeInfo>,
        media_info: MediaInfo,
        connection_state_rx: watch::Receiver<RTCPeerConnectionState>,
        session_id: String,
    ) -> Result<String> {
        let transceivers = peer.get_transceivers().await;

        let mut video_sender = None;
        let mut audio_sender = None;

        for transceiver in transceivers {
            let sender = transceiver
                .sender()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get sender: {}", e))?;
            let _kind = transceiver
                .current_direction()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get direction: {}", e))?;
            // Determine kind from the sender's track
            if let Some(ref s) = sender {
                let track_kind = s.track().kind().await;
                match track_kind {
                    RtpCodecKind::Video => video_sender = sender,
                    RtpCodecKind::Audio => audio_sender = sender,
                    RtpCodecKind::Unspecified => {}
                }
            }
        }

        {
            let s = SubscribeRTCPeerConnection::new(
                cascade.clone(),
                (self.stream.clone(), session_id.clone()),
                (peer.clone(), media_info),
                self.publish_rtcp_channel.clone(),
                (
                    self.publish_tracks.clone(),
                    self.publish_tracks_change.clone(),
                ),
                (video_sender, audio_sender),
                super::subscribe::SubscribeRuntime {
                    connection_state_rx,
                    generation_id: *self.media_generation_id.read().await,
                },
            )
            .await;

            self.subscribe_group.write().await.push(s);
            *self.subscribe_leave_at.write().await = 0;
        }

        metrics::SUBSCRIBE.inc();
        self.emit(Event::SubscribeStarted {
            stream: self.stream.clone(),
            session: session_id.clone(),
        });
        self.send_event();

        if cascade.is_some() {
            metrics::REFORWARD.inc();
        }

        Ok(session_id)
    }

    pub async fn remove_subscribe(&self, peer: Arc<dyn PeerConnection>) -> Result<()> {
        let old = {
            let mut subscribe_peers = self.subscribe_group.write().await;
            let Some(i) = subscribe_peers
                .iter()
                .position(|s| Arc::ptr_eq(&s.peer, &peer))
            else {
                // Already removed — idempotent no-op (see remove_publish).
                return Ok(());
            };
            let old = subscribe_peers.remove(i);
            let is_empty = subscribe_peers.is_empty();
            drop(subscribe_peers);
            if is_empty {
                *self.subscribe_leave_at.write().await = Utc::now().timestamp_millis();
            }
            old
        };

        self.do_remove_subscribe_cleanup(&old, SessionStopReason::PeerClosed)
            .await;
        Ok(())
    }

    /// Shared cleanup after a subscribe session has been removed from
    /// `self.subscribe_group`.  The caller must have already removed the entry
    /// from the vec (so the write lock is released before this runs) and
    /// finalized `subscribe_leave_at`, since this emits the `SubscribeStopped`
    /// event and the paired `ForwardChanged` ping itself.
    async fn do_remove_subscribe_cleanup(
        &self,
        subscribe: &SubscribeRTCPeerConnection,
        reason: SessionStopReason,
    ) {
        #[cfg(feature = "cascade")]
        if let Some(cascade) = subscribe.cascade.clone() {
            metrics::REFORWARD.dec();

            let client = Client::build(
                cascade.target_url.clone().unwrap(),
                cascade.session_url.clone(),
                Client::get_authorization_header_map(cascade.token.clone()),
            );

            tokio::spawn(async move {
                let _ = client.remove_resource().await;
            });
        }

        let mut session_info = subscribe.info().await;
        session_info.state = RTCPeerConnectionState::Closed;
        session_info.leave_at = Utc::now().timestamp_millis();
        let session_id = session_info.id.clone();

        {
            let mut closed_subscribe_sessions = self.closed_subscribe_sessions.write().await;
            closed_subscribe_sessions.push(session_info);
        }

        metrics::SUBSCRIBE.dec();
        self.emit(Event::SubscribeStopped {
            stream: self.stream.clone(),
            session: session_id,
            reason,
        });

        self.send_event();
    }

    pub(crate) async fn cleanup_closed_sessions(&self) {
        let now = Utc::now().timestamp_millis();
        let mut removed = false;
        {
            let mut closed_publish_sessions = self.closed_publish_sessions.write().await;
            let before = closed_publish_sessions.len();
            closed_publish_sessions.retain(|s| now - s.leave_at < CLOSED_SESSION_TTL_MS);
            if closed_publish_sessions.len() != before {
                removed = true;
            }
        }
        {
            let mut closed_subscribe_sessions = self.closed_subscribe_sessions.write().await;
            let before = closed_subscribe_sessions.len();
            closed_subscribe_sessions.retain(|s| now - s.leave_at < CLOSED_SESSION_TTL_MS);
            if closed_subscribe_sessions.len() != before {
                removed = true;
            }
        }
        // Purging an expired closed session changes the snapshot, so push an
        // event so SSE clients drop it instead of showing a stale ghost row.
        if removed {
            self.send_event();
        }
    }

    pub async fn select_kind_rid(&self, id: String, kind: RtpCodecKind, rid: String) -> Result<()> {
        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == id {
                subscribe.select_kind_rid(kind, rid)?;
                break;
            }
        }
        Ok(())
    }

    pub(crate) async fn subscribe_data_channel(
        &self,
        _peer: Arc<dyn PeerConnection>,
        dc: Arc<dyn DataChannel>,
    ) -> Result<()> {
        let sender = self.data_channel_forward.publish.clone();
        let receiver = self.data_channel_forward.subscribe.subscribe();
        Self::data_channel_forward(dc, sender, receiver, None);
        Ok(())
    }

    /// Emit the content-free change ping consumed by snapshot-based
    /// subscribers (SSE, net4mqtt).
    fn send_event(&self) {
        trace!("[{}] send forward event", self.stream);
        self.emit(Event::ForwardChanged {
            stream: self.stream.clone(),
        });
    }

    /// Emit a typed lifecycle event onto the manager-wide bus.
    fn emit(&self, event: Event) {
        let _ = self.lifecycle_sender.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::watch;

    #[tokio::test]
    async fn publish_connected_gate_blocks_until_connected() {
        let (tx, rx) = watch::channel(RTCPeerConnectionState::New);

        let task = tokio::spawn(wait_for_peer_connected(
            rx,
            std::time::Duration::from_secs(1),
            "publish rtcp",
        ));

        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        assert!(!task.is_finished());

        tx.send(RTCPeerConnectionState::Connected).unwrap();

        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn publish_connected_gate_fails_on_terminal_state() {
        for state in [
            RTCPeerConnectionState::Failed,
            RTCPeerConnectionState::Closed,
            RTCPeerConnectionState::Disconnected,
        ] {
            let (tx, rx) = watch::channel(RTCPeerConnectionState::New);
            tx.send(state).unwrap();

            let error =
                wait_for_peer_connected(rx, std::time::Duration::from_secs(1), "publish rtcp")
                    .await
                    .unwrap_err();
            let error = format!("{error:?}");

            assert!(error.contains("publish rtcp"));
            assert!(error.contains("before Connected"));
        }
    }

    #[tokio::test]
    async fn publish_connected_gate_times_out_with_context() {
        let (_tx, rx) = watch::channel(RTCPeerConnectionState::New);

        let error =
            wait_for_peer_connected(rx, std::time::Duration::from_millis(10), "manual twcc")
                .await
                .unwrap_err();
        let error = format!("{error:?}");

        assert!(error.contains("manual twcc"));
        assert!(error.contains("timed out"));
    }
}
