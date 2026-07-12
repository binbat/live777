use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rtc::rtcp::transport_feedbacks::transport_layer_cc::{
    PacketStatusChunk, RecvDelta, RunLengthChunk, StatusChunkTypeTcc, StatusVectorChunk,
    SymbolSizeTypeTcc, SymbolTypeTcc, TransportLayerCc,
};
use rtc::rtp::packet::Packet;
use rtc::shared::marshal::Unmarshal;
use std::collections::BTreeMap;
use tokio::sync::{broadcast, watch};
use tokio::time::Duration;
use tracing::{debug, info, trace, warn};

#[cfg(feature = "source")]
use rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
#[cfg(feature = "source")]
use tracing::error;
use webrtc::media_stream::track_remote::TrackRemote;

#[cfg(feature = "source")]
use std::sync::atomic::AtomicU32;
#[cfg(feature = "source")]
use std::time::{SystemTime, UNIX_EPOCH};

use super::internal::{PUBLISH_CONNECTED_TIMEOUT, wait_for_peer_connected};
use super::message::Codec;
use crate::new_broadcast_channel;

#[cfg(feature = "source")]
fn codec_string(params: &rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters) -> String {
    format!(
        "{}[{}],{}",
        params.rtp_codec.mime_type, params.payload_type, params.rtp_codec.sdp_fmtp_line,
    )
}

pub(crate) type ForwardData = Arc<Packet>;

const MANUAL_TWCC_INTERVAL: Duration = Duration::from_millis(100);
const MANUAL_TWCC_MAX_STATUS_COUNT: u16 = 512;
const TYPE_TCC_DELTA_SCALE_FACTOR_US: i64 = 250;

struct ManualTwccFeedback {
    sender_ssrc: u32,
    media_ssrc: u32,
    native_twcc_bound: Option<Arc<AtomicBool>>,
    start: Option<Instant>,
    next_flush: Option<Instant>,
    fb_pkt_count: u8,
    base_sequence_number: Option<u16>,
    arrivals_us: BTreeMap<u16, i64>,
}

impl ManualTwccFeedback {
    fn new(_twcc_ext_id: u8, native_twcc_bound: Option<Arc<AtomicBool>>) -> Self {
        Self {
            sender_ssrc: rand::random(),
            media_ssrc: 0,
            native_twcc_bound,
            start: None,
            next_flush: None,
            fb_pkt_count: 0,
            base_sequence_number: None,
            arrivals_us: BTreeMap::new(),
        }
    }

    fn new_shared(
        twcc_ext_id: u8,
        native_twcc_bound: Option<Arc<AtomicBool>>,
    ) -> SharedManualTwccFeedback {
        SharedManualTwccFeedback::new(twcc_ext_id, native_twcc_bound)
    }

    fn record(
        &mut self,
        media_ssrc: u32,
        transport_sequence: u16,
        now: Instant,
    ) -> Vec<Box<dyn rtc::rtcp::Packet>> {
        if self
            .native_twcc_bound
            .as_ref()
            .is_some_and(|native| native.load(Ordering::Relaxed))
        {
            self.arrivals_us.clear();
            self.base_sequence_number = None;
            return Vec::new();
        }

        self.media_ssrc = media_ssrc;
        let start = *self.start.get_or_insert(now);
        self.next_flush.get_or_insert(now + MANUAL_TWCC_INTERVAL);

        if self.base_sequence_number.is_none() {
            self.base_sequence_number = Some(transport_sequence);
        } else if let Some(base) = self.base_sequence_number {
            let distance = transport_sequence.wrapping_sub(base);
            if distance > MANUAL_TWCC_MAX_STATUS_COUNT {
                let packets = self.flush(now);
                self.base_sequence_number = Some(transport_sequence);
                self.arrivals_us.insert(
                    transport_sequence,
                    now.duration_since(start).as_micros() as i64,
                );
                return packets;
            }
        }

        self.arrivals_us
            .entry(transport_sequence)
            .or_insert_with(|| now.duration_since(start).as_micros() as i64);

        if self.next_flush.is_some_and(|deadline| now >= deadline) {
            self.flush(now)
        } else {
            Vec::new()
        }
    }

    fn flush(&mut self, now: Instant) -> Vec<Box<dyn rtc::rtcp::Packet>> {
        let Some(base_sequence_number) = self.base_sequence_number else {
            return Vec::new();
        };
        if self.arrivals_us.is_empty() {
            return Vec::new();
        }

        let max_distance = self
            .arrivals_us
            .keys()
            .map(|seq| seq.wrapping_sub(base_sequence_number))
            .filter(|distance| *distance <= MANUAL_TWCC_MAX_STATUS_COUNT)
            .max()
            .unwrap_or(0);
        let packet_status_count = max_distance + 1;

        let first_arrival_us = *self
            .arrivals_us
            .get(&base_sequence_number)
            .or_else(|| self.arrivals_us.values().next())
            .unwrap_or(&0);
        let reference_time = first_arrival_us / 64_000;
        let mut last_timestamp_us = reference_time * 64_000;
        let mut symbols = Vec::with_capacity(packet_status_count as usize);
        let mut recv_deltas = Vec::new();

        for offset in 0..packet_status_count {
            let seq = base_sequence_number.wrapping_add(offset);
            if let Some(arrival_us) = self.arrivals_us.get(&seq) {
                let delta_250us = ((*arrival_us - last_timestamp_us)
                    + TYPE_TCC_DELTA_SCALE_FACTOR_US / 2)
                    / TYPE_TCC_DELTA_SCALE_FACTOR_US;
                let delta_250us = delta_250us.clamp(i16::MIN as i64, i16::MAX as i64);
                let delta_us_rounded = delta_250us * TYPE_TCC_DELTA_SCALE_FACTOR_US;
                let symbol = if (0..=u8::MAX as i64).contains(&delta_250us) {
                    SymbolTypeTcc::PacketReceivedSmallDelta
                } else {
                    SymbolTypeTcc::PacketReceivedLargeDelta
                };
                symbols.push(symbol);
                recv_deltas.push(RecvDelta {
                    type_tcc_packet: symbol,
                    delta: delta_us_rounded,
                });
                last_timestamp_us += delta_us_rounded;
            } else {
                symbols.push(SymbolTypeTcc::PacketNotReceived);
            }
        }

        let packet_chunks = encode_twcc_status_chunks(&symbols);
        let packet = TransportLayerCc {
            sender_ssrc: self.sender_ssrc,
            media_ssrc: self.media_ssrc,
            base_sequence_number,
            packet_status_count,
            reference_time: reference_time as u32,
            fb_pkt_count: self.fb_pkt_count,
            packet_chunks,
            recv_deltas,
        };

        self.fb_pkt_count = self.fb_pkt_count.wrapping_add(1);
        self.arrivals_us.clear();
        self.base_sequence_number = None;
        self.next_flush = Some(now + MANUAL_TWCC_INTERVAL);

        vec![Box::new(packet)]
    }
}

#[derive(Clone)]
pub(crate) struct SharedManualTwccFeedback {
    inner: Arc<Mutex<ManualTwccFeedback>>,
}

impl SharedManualTwccFeedback {
    pub(crate) fn new(twcc_ext_id: u8, native_twcc_bound: Option<Arc<AtomicBool>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManualTwccFeedback::new(
                twcc_ext_id,
                native_twcc_bound,
            ))),
        }
    }

    fn record(
        &self,
        media_ssrc: u32,
        transport_sequence: u16,
        now: Instant,
    ) -> Vec<Box<dyn rtc::rtcp::Packet>> {
        self.inner
            .lock()
            .expect("manual TWCC feedback lock poisoned")
            .record(media_ssrc, transport_sequence, now)
    }

    #[cfg(test)]
    fn flush(&self, now: Instant) -> Vec<Box<dyn rtc::rtcp::Packet>> {
        self.inner
            .lock()
            .expect("manual TWCC feedback lock poisoned")
            .flush(now)
    }
}

fn encode_twcc_status_chunks(symbols: &[SymbolTypeTcc]) -> Vec<PacketStatusChunk> {
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < symbols.len() {
        let remaining = &symbols[i..];
        let same_run_len = remaining
            .iter()
            .take_while(|symbol| **symbol == remaining[0])
            .count();
        if same_run_len >= 7 {
            let run_length = same_run_len.min(0x1fff);
            chunks.push(PacketStatusChunk::RunLengthChunk(RunLengthChunk {
                type_tcc: StatusChunkTypeTcc::RunLengthChunk,
                packet_status_symbol: remaining[0],
                run_length: run_length as u16,
            }));
            i += run_length;
            continue;
        }

        let count = remaining.len().min(7);
        chunks.push(PacketStatusChunk::StatusVectorChunk(StatusVectorChunk {
            type_tcc: StatusChunkTypeTcc::StatusVectorChunk,
            symbol_size: SymbolSizeTypeTcc::TwoBit,
            symbol_list: remaining[..count].to_vec(),
        }));
        i += count;
    }
    chunks
}

#[derive(Clone)]
pub(crate) enum PublishTrackRemote {
    Real {
        generation_id: u64,
        rid: String,
        kind: RtpCodecKind,
        codec: Codec,
        track: Arc<dyn TrackRemote>,
        rtp_broadcast: Arc<broadcast::Sender<ForwardData>>,
    },
    #[cfg(feature = "source")]
    Virtual(Arc<VirtualPublishTrack>),
}

impl PublishTrackRemote {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        stream: String,
        id: String,
        track: Arc<dyn TrackRemote>,
        connected_gate: Option<watch::Receiver<webrtc::peer_connection::RTCPeerConnectionState>>,
        twcc_ext_id: u8,
        native_twcc_bound: Arc<AtomicBool>,
        manual_twcc_feedback: Option<SharedManualTwccFeedback>,
        generation_id: u64,
    ) -> Self {
        let rtp_sender = new_broadcast_channel!(4096);
        let ssrcs = track.ssrcs().await;
        let first_ssrc = ssrcs.first().copied().unwrap_or(0);
        let rid = track
            .rid(first_ssrc)
            .await
            .map(|r| r.to_string())
            .unwrap_or_default();
        let kind = track.kind().await;

        let raw_codec = track.codec(first_ssrc).await.unwrap_or_default();
        let media: Vec<String> = raw_codec
            .mime_type
            .to_lowercase()
            .split('/')
            .map(|s| s.to_string())
            .collect();
        let codec = Codec {
            kind: media.first().cloned().unwrap_or_default(),
            codec: media.get(1).cloned().unwrap_or_default(),
            fmtp: raw_codec.sdp_fmtp_line,
        };

        tokio::spawn(Self::track_forward(
            stream.clone(),
            id.clone(),
            track.clone(),
            rtp_sender.clone(),
            connected_gate,
            twcc_ext_id,
            native_twcc_bound,
            manual_twcc_feedback,
        ));

        Self::Real {
            generation_id,
            rid,
            kind,
            codec,
            track,
            rtp_broadcast: Arc::new(rtp_sender),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn track_forward(
        stream: String,
        id: String,
        track: Arc<dyn TrackRemote>,
        rtp_sender: broadcast::Sender<ForwardData>,
        connected_gate: Option<watch::Receiver<webrtc::peer_connection::RTCPeerConnectionState>>,
        twcc_ext_id: u8,
        native_twcc_bound: Arc<AtomicBool>,
        manual_twcc_feedback: Option<SharedManualTwccFeedback>,
    ) {
        let kind = track.kind().await;
        let ssrcs = track.ssrcs().await;
        let first_ssrc = ssrcs.first().copied().unwrap_or(0);
        let rid = track
            .rid(first_ssrc)
            .await
            .map(|r| r.to_string())
            .unwrap_or_default();
        let codec = track.codec(first_ssrc).await;

        info!(
            "[{}] [{}] [track] kind: {:?}, rid: {}, ssrc: {:?}, codec: {:?} start forward",
            stream, id, kind, rid, ssrcs, codec,
        );

        // TWCC inbound probe: monitors transport-wide-cc header extension on
        // incoming RTP from the publisher, using the negotiated extmap ID.
        let twcc_seen = AtomicBool::new(false);
        let twcc_missing_count = AtomicU64::new(0);
        let packets_total = AtomicU64::new(0);
        let last_twcc_seq = AtomicU64::new(0);
        let probe_start = Instant::now();
        let mut probe_tick = Instant::now();
        let manual_twcc = manual_twcc_feedback.or_else(|| {
            (twcc_ext_id != 0)
                .then(|| ManualTwccFeedback::new_shared(twcc_ext_id, Some(native_twcc_bound)))
        });
        info!(
            "[{}] [{}] [twcc-probe] negotiated_twcc_ext_id={}",
            stream, id, twcc_ext_id,
        );

        if let Some(gate) = connected_gate
            && let Err(err) =
                wait_for_peer_connected(gate, PUBLISH_CONNECTED_TIMEOUT, "publish track forward")
                    .await
        {
            warn!(
                "[{}] [{}] [track] kind: {:?}, rid: {}, ssrc: {:?} stop before Connected: {:?}",
                stream, id, kind, rid, ssrcs, err,
            );
            return;
        }
        debug!(
            "[{}] [{}] [track] kind: {:?}, rid: {}, ssrc: {:?} Connected; starting poll loop",
            stream, id, kind, rid, ssrcs,
        );

        loop {
            match track.poll().await {
                Some(webrtc::media_stream::track_remote::TrackRemoteEvent::OnRtpPacket(
                    rtp_packet,
                )) => {
                    trace!(
                        "RTP packet - SSRC: {}, SeqNum: {}, Timestamp: {}",
                        rtp_packet.header.ssrc,
                        rtp_packet.header.sequence_number,
                        rtp_packet.header.timestamp
                    );

                    // --- TWCC inbound probe ---
                    let total = packets_total.fetch_add(1, Ordering::Relaxed) + 1;
                    let packet_ext_ids: Vec<u8> =
                        rtp_packet.header.extensions.iter().map(|e| e.id).collect();
                    let mut found_twcc = false;
                    // Only inspect the negotiated TWCC extension — avoids mis-parsing
                    // unrelated extensions as TransportCcExtension.
                    if twcc_ext_id != 0 {
                        let mut raw = rtp_packet.header.get_extension(twcc_ext_id);
                        if let Some(ref mut data) = raw
                            && let Ok(tcc) = rtc::rtp::extension::transport_cc_extension::TransportCcExtension::unmarshal(data)
                        {
                                found_twcc = true;
                                if !twcc_seen.swap(true, Ordering::Relaxed) {
                                    info!(
                                        "[{}] [{}] [twcc-probe] first TWCC ext seen: ext_id={}, transport_seq={}, ssrc={}",
                                        stream,
                                        id,
                                        twcc_ext_id,
                                        tcc.transport_sequence,
                                        rtp_packet.header.ssrc,
                                    );
                                }
                                last_twcc_seq
                                    .store(tcc.transport_sequence as u64, Ordering::Relaxed);
                                if let Some(feedback) = manual_twcc.as_ref() {
                                    let packets = feedback.record(
                                        rtp_packet.header.ssrc,
                                        tcc.transport_sequence,
                                        Instant::now(),
                                    );
                                    if !packets.is_empty() {
                                        let count = packets.len();
                                        match track.write_rtcp(packets).await {
                                            Ok(()) => trace!(
                                                "[{}] [{}] [twcc-probe] wrote manual TWCC feedback packets={}",
                                                stream, id, count,
                                            ),
                                            Err(err) => warn!(
                                                "[{}] [{}] [twcc-probe] failed to write manual TWCC feedback: {}",
                                                stream, id, err,
                                            ),
                                        }
                                    }
                                }
                        }
                    }
                    if !found_twcc {
                        twcc_missing_count.fetch_add(1, Ordering::Relaxed);
                    }
                    // Log periodic stats every 5 seconds
                    if probe_tick.elapsed() >= Duration::from_secs(5) {
                        probe_tick = Instant::now();
                        let twcc_present = twcc_seen.load(Ordering::Relaxed);
                        let missing = twcc_missing_count.load(Ordering::Relaxed);
                        let seq = last_twcc_seq.load(Ordering::Relaxed);
                        if twcc_present {
                            trace!(
                                "[{}] [{}] [twcc-probe] total={}, twcc_present=true, last_twcc_seq={}, missing_since_last={}, packet_ext_ids={:?}",
                                stream, id, total, seq, missing, packet_ext_ids,
                            );
                        } else if total > 50 && !twcc_present {
                            warn!(
                                "[{}] [{}] [twcc-probe] total={}, twcc_present=false, missing={}, packet_ext_ids={:?} — NO TWCC ext_id={} seen in {} packets over {:?}",
                                stream,
                                id,
                                total,
                                missing,
                                packet_ext_ids,
                                twcc_ext_id,
                                total,
                                probe_start.elapsed(),
                            );
                        }
                    }
                    // --- end TWCC probe ---

                    // Forward via bounded send; drop if channel is full to avoid
                    // backpressure from a slow subscriber stalling the publisher read loop.
                    // The publisher RTP read loop MUST drain continuously so the
                    // TwccReceiver interceptor keeps processing packets and generating
                    // TWCC feedback.
                    if rtp_sender.receiver_count() > 0 {
                        let _ = rtp_sender.send(Arc::new(rtp_packet));
                    }
                }
                Some(webrtc::media_stream::track_remote::TrackRemoteEvent::OnEnded) => {
                    debug!(
                        "[{}] [{}] [track] kind: {:?}, track ended",
                        stream, id, kind,
                    );
                    break;
                }
                Some(_) => {}
                None => {
                    debug!(
                        "[{}] [{}] [track] kind: {:?}, poll returned None",
                        stream, id, kind,
                    );
                    break;
                }
            }
        }

        info!(
            "[{}] [{}] [track] kind: {:?}, rid: {}, ssrc: {:?} stop forward",
            stream, id, kind, rid, ssrcs,
        );
    }

    pub(crate) fn kind(&self) -> RtpCodecKind {
        match self {
            Self::Real { kind, .. } => *kind,
            #[cfg(feature = "source")]
            Self::Virtual(v) => v.kind,
        }
    }

    pub(crate) fn rid(&self) -> &str {
        match self {
            Self::Real { rid, .. } => rid,
            #[cfg(feature = "source")]
            Self::Virtual(v) => &v.rid,
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<ForwardData> {
        match self {
            Self::Real { rtp_broadcast, .. } => rtp_broadcast.subscribe(),
            #[cfg(feature = "source")]
            Self::Virtual(v) => v.subscribe(),
        }
    }

    pub(crate) fn codec(&self) -> Codec {
        match self {
            Self::Real { codec, .. } => codec.clone(),
            #[cfg(feature = "source")]
            Self::Virtual(v) => v.codec(),
        }
    }

    #[cfg(feature = "rtsp")]
    pub(crate) async fn source_ssrc(&self) -> u32 {
        match self {
            Self::Real { track, .. } => track.ssrcs().await.first().copied().unwrap_or(0),
            #[cfg(feature = "source")]
            Self::Virtual(v) => v.ssrc(),
        }
    }

    #[cfg(feature = "source")]
    pub(crate) fn inject_rtp(&self, packet: Arc<Packet>) -> Result<(), String> {
        match self {
            Self::Virtual(v) => v.inject_rtp(packet),
            Self::Real { .. } => Err("Cannot inject RTP into real track".to_string()),
        }
    }

    #[cfg(feature = "source")]
    pub(crate) fn generate_sender_report(
        &self,
    ) -> Option<Box<dyn rtc_rtcp::packet::Packet + Send + Sync>> {
        match self {
            Self::Virtual(v) => v.generate_sender_report(),
            Self::Real { .. } => None,
        }
    }

    pub(crate) fn generation_id(&self) -> u64 {
        match self {
            Self::Real { generation_id, .. } => *generation_id,
            #[cfg(feature = "source")]
            Self::Virtual(_) => 0,
        }
    }
}

#[cfg(feature = "source")]
pub struct VirtualPublishTrack {
    pub rid: String,
    pub kind: RtpCodecKind,
    pub codec_params: RTCRtpCodecParameters,
    pub rtp_broadcast: Arc<broadcast::Sender<ForwardData>>,
    stream_id: String,
    actual_ssrc: Arc<AtomicU32>,
    packets_sent: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    last_ntp_time_ms: Arc<AtomicU64>,
    sequence_number: Arc<AtomicU32>,
    clock_rate: u32,
    start_time: SystemTime,
}

#[cfg(feature = "source")]
impl VirtualPublishTrack {
    pub fn new(stream_id: String, kind: RtpCodecKind, codec_params: RTCRtpCodecParameters) -> Self {
        let rtp_sender = new_broadcast_channel!(4096);

        debug!(
            "[{}] Created virtual {:?} track with codec: {}",
            stream_id,
            kind,
            codec_string(&codec_params),
        );

        Self {
            rid: String::new(),
            kind,
            codec_params: codec_params.clone(),
            rtp_broadcast: Arc::new(rtp_sender),
            stream_id,
            actual_ssrc: Arc::new(AtomicU32::new(0)),
            packets_sent: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            last_ntp_time_ms: Arc::new(AtomicU64::new(0)),
            sequence_number: Arc::new(AtomicU32::new(rand::random::<u16>() as u32)),
            clock_rate: codec_params.rtp_codec.clock_rate,
            start_time: SystemTime::now(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ForwardData> {
        self.rtp_broadcast.subscribe()
    }

    pub fn codec(&self) -> Codec {
        let media: Vec<String> = self
            .codec_params
            .rtp_codec
            .mime_type
            .clone()
            .to_lowercase()
            .split('/')
            .map(|s| s.to_string())
            .collect();

        Codec {
            kind: media.first().cloned().unwrap_or_default(),
            codec: media.get(1).cloned().unwrap_or_default(),
            fmtp: self.codec_params.rtp_codec.sdp_fmtp_line.clone(),
        }
    }

    pub fn inject_rtp(&self, packet: Arc<Packet>) -> Result<(), String> {
        if self.actual_ssrc.load(Ordering::Relaxed) == 0 {
            self.actual_ssrc
                .store(packet.header.ssrc, Ordering::Relaxed);
            info!(
                "[{}] Detected {:?} SSRC: {}",
                self.stream_id, self.kind, packet.header.ssrc
            );
        }

        let mut packet_mut = (*packet).clone();

        let seq = self.sequence_number.fetch_add(1, Ordering::Relaxed) as u16;
        packet_mut.header.sequence_number = seq;

        let packet_count = self.packets_sent.fetch_add(1, Ordering::Relaxed) + 1;
        self.bytes_sent
            .fetch_add(packet_mut.payload.len() as u64, Ordering::Relaxed);

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_ntp_time_ms.store(now_ms, Ordering::Relaxed);

        if packet_count % 100 == 1 {
            debug!(
                "[{}] Sent {:?} packet #{}, SSRC: {}, seq: {}, ts: {}",
                self.stream_id,
                self.kind,
                packet_count,
                packet_mut.header.ssrc,
                seq,
                packet_mut.header.timestamp
            );
        }

        match self.rtp_broadcast.send(Arc::new(packet_mut)) {
            Ok(sent_count) => {
                if packet_count % 100 == 1 {
                    trace!(
                        "[{}] Sent {:?} packet to {} receivers",
                        self.stream_id, self.kind, sent_count
                    );
                }
                Ok(())
            }
            Err(e) => {
                error!(
                    "[{}] Failed to broadcast {:?} packet #{}: {}",
                    self.stream_id, self.kind, packet_count, e
                );
                Err(format!("Failed to send RTP: {}", e))
            }
        }
    }

    pub fn ssrc(&self) -> u32 {
        self.actual_ssrc.load(Ordering::Relaxed)
    }

    pub fn generate_sender_report(
        &self,
    ) -> Option<Box<dyn rtc_rtcp::packet::Packet + Send + Sync>> {
        let ssrc = self.actual_ssrc.load(Ordering::Relaxed);
        if ssrc == 0 {
            return None;
        }

        let last_ntp_ms = self.last_ntp_time_ms.load(Ordering::Relaxed);

        if last_ntp_ms == 0 {
            return None;
        }

        let ntp_time = UNIX_EPOCH + std::time::Duration::from_millis(last_ntp_ms);

        let elapsed = SystemTime::now()
            .duration_since(self.start_time)
            .unwrap_or_default();
        let rtp_time = (elapsed.as_secs_f64() * self.clock_rate as f64) as u32;

        Some(Box::new(rtc_rtcp::sender_report::SenderReport {
            ssrc,
            ntp_time: system_time_to_ntp(ntp_time),
            rtp_time,
            packet_count: self.packets_sent.load(Ordering::Relaxed) as u32,
            octet_count: self.bytes_sent.load(Ordering::Relaxed) as u32,
            ..Default::default()
        }))
    }
}

#[cfg(feature = "source")]
fn system_time_to_ntp(time: SystemTime) -> u64 {
    const UNIX_TO_NTP_EPOCH: u64 = 2_208_988_800;

    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();

    let seconds = duration.as_secs() + UNIX_TO_NTP_EPOCH;
    let fraction = ((duration.subsec_nanos() as u64) << 32) / 1_000_000_000;

    (seconds << 32) | fraction
}

#[cfg(test)]
mod manual_twcc_tests {
    use super::*;

    #[test]
    fn manual_twcc_feedback_marks_missing_packets() {
        let start = Instant::now();
        let mut feedback = ManualTwccFeedback::new(4, None);

        assert!(feedback.record(99, 10, start).is_empty());
        assert!(
            feedback
                .record(99, 12, start + Duration::from_millis(20))
                .is_empty()
        );
        let packets = feedback.flush(start + Duration::from_millis(120));

        assert_eq!(packets.len(), 1);
        let tcc = packets[0]
            .as_any()
            .downcast_ref::<rtc::rtcp::transport_feedbacks::transport_layer_cc::TransportLayerCc>()
            .expect("manual feedback must generate TransportLayerCc");
        assert_eq!(tcc.media_ssrc, 99);
        assert_eq!(tcc.base_sequence_number, 10);
        assert_eq!(tcc.packet_status_count, 3);
        assert_eq!(tcc.recv_deltas.len(), 2);
    }

    #[test]
    fn manual_twcc_feedback_is_disabled_when_native_twcc_is_bound() {
        let start = Instant::now();
        let native_twcc_bound = Arc::new(AtomicBool::new(true));
        let mut feedback = ManualTwccFeedback::new(4, Some(native_twcc_bound));

        assert!(feedback.record(99, 10, start).is_empty());
        assert!(
            feedback
                .flush(start + Duration::from_millis(120))
                .is_empty()
        );
    }

    #[test]
    fn shared_manual_twcc_feedback_aggregates_interleaved_media_sequences() {
        let start = Instant::now();
        let feedback = ManualTwccFeedback::new_shared(4, None);

        assert!(feedback.record(100, 10, start).is_empty());
        assert!(
            feedback
                .record(200, 11, start + Duration::from_millis(10))
                .is_empty()
        );
        assert!(
            feedback
                .record(100, 12, start + Duration::from_millis(20))
                .is_empty()
        );
        let packets = feedback.flush(start + Duration::from_millis(120));

        assert_eq!(packets.len(), 1);
        let tcc = packets[0]
            .as_any()
            .downcast_ref::<rtc::rtcp::transport_feedbacks::transport_layer_cc::TransportLayerCc>()
            .expect("manual feedback must generate TransportLayerCc");
        assert_eq!(tcc.base_sequence_number, 10);
        assert_eq!(tcc.packet_status_count, 3);
        assert_eq!(tcc.recv_deltas.len(), 3);
    }
}
