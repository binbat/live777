use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::{debug, info, trace};
use webrtc::rtp::packet::Packet;

#[cfg(feature = "source")]
use tracing::error;
#[cfg(feature = "source")]
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::track::track_remote::TrackRemote;

#[cfg(feature = "source")]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
#[cfg(feature = "source")]
use std::time::{SystemTime, UNIX_EPOCH};

use super::message::Codec;
use crate::new_broadcast_channel;

fn codec_string(params: webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters) -> String {
    format!(
        "{}[{}],{}",
        params.capability.mime_type, params.payload_type, params.capability.sdp_fmtp_line,
    )
}

pub(crate) type ForwardData = Arc<Packet>;

#[derive(Clone)]
pub(crate) enum PublishTrackRemote {
    Real {
        rid: String,
        kind: RTPCodecType,
        track: Arc<TrackRemote>,
        rtp_broadcast: Arc<broadcast::Sender<ForwardData>>,
    },
    #[cfg(feature = "source")]
    Virtual(Arc<VirtualPublishTrack>),
}

impl PublishTrackRemote {
    pub async fn new(stream: String, id: String, track: Arc<TrackRemote>) -> Self {
        let rtp_sender = new_broadcast_channel!(128);
        let rid = track.rid().to_owned();
        let kind = track.kind();

        tokio::spawn(Self::track_forward(
            stream,
            id,
            track.clone(),
            rtp_sender.clone(),
        ));

        Self::Real {
            rid,
            kind,
            track,
            rtp_broadcast: Arc::new(rtp_sender),
        }
    }

    async fn track_forward(
        stream: String,
        id: String,
        track: Arc<TrackRemote>,
        rtp_sender: broadcast::Sender<ForwardData>,
    ) {
        info!(
            "[{}] [{}] [track] kind: {:?}, rid: {}, ssrc: {}, codec: {} start forward",
            stream,
            id,
            track.kind(),
            track.rid(),
            track.ssrc(),
            codec_string(track.codec()),
        );
        trace!("codec: {:?}", track.codec());

        let mut b = vec![0u8; 1500];

        loop {
            match track.read(&mut b).await {
                Ok((rtp_packet, _)) => {
                    trace!(
                        "RTP packet - SSRC: {}, SeqNum: {}, Timestamp: {}",
                        rtp_packet.header.ssrc,
                        rtp_packet.header.sequence_number,
                        rtp_packet.header.timestamp
                    );

                    if let Err(err) = rtp_sender.send(Arc::new(rtp_packet)) {
                        debug!(
                            "[{}] [{}] [track] kind: {:?}, rid: {}, rtp broadcast error : {}",
                            stream,
                            id,
                            track.kind(),
                            track.rid(),
                            err
                        );
                        break;
                    }
                }
                Err(err) => {
                    debug!(
                        "[{}] [{}] [track] kind: {:?}, {} read error : {}",
                        stream,
                        id,
                        track.kind(),
                        track.rid(),
                        err
                    );
                    break;
                }
            }
        }

        info!(
            "[{}] [{}] [track] kind: {:?}, rid :{}, ssrc: {} stop forward",
            stream,
            id,
            track.kind(),
            track.rid(),
            track.ssrc()
        );
    }

    pub(crate) fn kind(&self) -> RTPCodecType {
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
            Self::Real { track, .. } => {
                let codec = track.codec();
                let media: Vec<String> = codec
                    .capability
                    .mime_type
                    .clone()
                    .to_lowercase()
                    .split('/')
                    .map(|s| s.to_string())
                    .collect();

                Codec {
                    kind: media.first().cloned().unwrap(),
                    codec: media.get(1).cloned().unwrap(),
                    fmtp: codec.capability.sdp_fmtp_line,
                }
            }
            #[cfg(feature = "source")]
            Self::Virtual(v) => v.codec(),
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
    ) -> Option<Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>> {
        match self {
            Self::Virtual(v) => v.generate_sender_report(),
            Self::Real { .. } => None,
        }
    }
}

#[cfg(feature = "source")]
pub struct VirtualPublishTrack {
    pub rid: String,
    pub kind: RTPCodecType,
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
    pub fn new(stream_id: String, kind: RTPCodecType, codec_params: RTCRtpCodecParameters) -> Self {
        let rtp_sender = new_broadcast_channel!(128);

        debug!(
            "[{}] Created virtual {:?} track with codec: {}",
            stream_id,
            kind,
            codec_string(codec_params.clone()),
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
            clock_rate: codec_params.capability.clock_rate,
            start_time: SystemTime::now(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ForwardData> {
        self.rtp_broadcast.subscribe()
    }

    pub fn codec(&self) -> Codec {
        let media: Vec<String> = self
            .codec_params
            .capability
            .mime_type
            .clone()
            .to_lowercase()
            .split('/')
            .map(|s| s.to_string())
            .collect();

        Codec {
            kind: media.first().cloned().unwrap(),
            codec: media.get(1).cloned().unwrap(),
            fmtp: self.codec_params.capability.sdp_fmtp_line.clone(),
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
    ) -> Option<Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>> {
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

        Some(Box::new(webrtc::rtcp::sender_report::SenderReport {
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
