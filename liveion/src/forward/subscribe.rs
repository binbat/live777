use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use chrono::Utc;
use rtc::media_stream::MediaStreamTrack;
use rtc::rtp_transceiver::PayloadType;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tokio::sync::{RwLock, broadcast};
use tokio::time::{Duration, Instant, sleep};
use tracing::{debug, info, warn};
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::peer_connection::{PeerConnection, RTCPeerConnectionState};
use webrtc::rtp_transceiver::RtpSender;

use crate::error::AppError;
use crate::forward::message::SessionInfo;
use crate::forward::rtcp::RtcpMessage;
use crate::new_broadcast_channel;
use crate::{constant, result::Result};

use super::get_peer_id;
use super::media::MediaInfo;
use super::message::CascadeInfo;
use super::track::ForwardData;
use super::track::PublishTrackRemote;

use super::codec_compat::*;

type SelectLayerBody = (RtpCodecKind, String);
type OptionalRtpSender = Option<Arc<dyn RtpSender>>;

const TRACK_BIND_RETRY_DELAY: Duration = Duration::from_millis(20);
const TRACK_BIND_RETRY_TIMEOUT: Duration = Duration::from_secs(3);

struct BoundPublishTrack {
    recv: broadcast::Receiver<ForwardData>,
    track: Arc<dyn TrackLocal>,
    payload_type: Option<PayloadType>,
    source_codec: RTCRtpCodec,
    selected_codec: RTCRtpCodec,
}

struct SubscribeForwardChannel {
    publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
    select_layer_recv: broadcast::Receiver<SelectLayerBody>,
    publish_track_change: broadcast::Receiver<()>,
    connection_state: Arc<std::sync::RwLock<RTCPeerConnectionState>>,
    generation_id: u64,
}

pub(crate) struct SubscribeRuntime {
    pub(crate) connection_state: Arc<std::sync::RwLock<RTCPeerConnectionState>>,
    pub(crate) generation_id: u64,
}

pub(crate) struct SubscribeRTCPeerConnection {
    pub(crate) id: String,
    pub(crate) cascade: Option<CascadeInfo>,
    pub(crate) peer: Arc<dyn PeerConnection>,
    pub(crate) create_at: i64,
    select_layer_sender: broadcast::Sender<SelectLayerBody>,
    pub(crate) media_info: MediaInfo,
    connection_state: Arc<std::sync::RwLock<RTCPeerConnectionState>>,
}

impl SubscribeRTCPeerConnection {
    pub(crate) async fn new(
        cascade: Option<CascadeInfo>,
        stream: String,
        (peer, media_info): (Arc<dyn PeerConnection>, MediaInfo),
        publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
        (publish_tracks, publish_track_change): (
            Arc<RwLock<Vec<PublishTrackRemote>>>,
            broadcast::Sender<()>, // use subscribe
        ),
        (video_sender, audio_sender): (OptionalRtpSender, OptionalRtpSender),
        runtime: SubscribeRuntime,
    ) -> Self {
        let select_layer_sender = new_broadcast_channel!(1);
        let id = get_peer_id(&peer);
        let connection_state = runtime.connection_state;
        let track_binding_publish_rid = Arc::new(RwLock::new(HashMap::new()));
        for (sender, kind) in [
            (video_sender, RtpCodecKind::Video),
            (audio_sender, RtpCodecKind::Audio),
        ] {
            if sender.is_none() {
                continue;
            }
            let sender = sender.unwrap();
            tokio::spawn(Self::sender_forward_rtp(
                stream.clone(),
                id.clone(),
                sender,
                kind,
                track_binding_publish_rid.clone(),
                publish_tracks.clone(),
                SubscribeForwardChannel {
                    publish_rtcp_sender: publish_rtcp_sender.clone(),
                    select_layer_recv: select_layer_sender.subscribe(),
                    publish_track_change: publish_track_change.subscribe(),
                    connection_state: connection_state.clone(),
                    generation_id: runtime.generation_id,
                },
            ));
        }
        let _ = publish_track_change.send(());
        Self {
            id,
            cascade,
            peer,
            create_at: Utc::now().timestamp_millis(),
            select_layer_sender,
            media_info,
            connection_state,
        }
    }

    pub(crate) async fn info(&self) -> SessionInfo {
        let state = self
            .connection_state
            .read()
            .map(|s| *s)
            .unwrap_or(RTCPeerConnectionState::New);
        SessionInfo {
            id: self.id.clone(),
            create_at: self.create_at,
            leave_at: 0,
            state,
            cascade: self.cascade.clone(),
            has_data_channel: self.media_info.has_data_channel,
        }
    }

    /// Try to bind to an existing publish track. Returns (new_recv, new_track) if successful.
    #[allow(clippy::too_many_arguments)]
    async fn try_bind_publish_track(
        stream: &str,
        id: &str,
        sender: &Arc<dyn RtpSender>,
        kind: RtpCodecKind,
        sender_ssrc: u32,
        track_binding_publish_rid: &Arc<RwLock<HashMap<String, String>>>,
        publish_tracks: &Arc<RwLock<Vec<PublishTrackRemote>>>,
        forward_channel: &SubscribeForwardChannel,
        _virtual_sender: &broadcast::Sender<ForwardData>,
    ) -> Option<BoundPublishTrack> {
        // Read the current binding and then clone the publish tracks under
        // their respective read locks without an await point in between. This
        // avoids seeing a binding that was updated after the publish track
        // snapshot (or vice versa) due to a task yield. The publish tracks are
        // cloned (they are Arc-based) so the RwLock read guard can be dropped
        // before any await points below.
        let current_rid = {
            let binding = track_binding_publish_rid.read().await;
            binding.get(&kind.to_string()).cloned()
        };

        if current_rid
            .as_ref()
            .is_some_and(|r| r == constant::RID_DISABLE)
        {
            return None;
        }

        let publish_tracks: Vec<PublishTrackRemote> = {
            let tracks = publish_tracks.read().await;
            tracks.iter().cloned().collect()
        };

        if publish_tracks.is_empty() {
            return None;
        }

        for publish_track in publish_tracks.iter() {
            if publish_track.kind() != kind {
                continue;
            }

            if publish_track.generation_id() != forward_channel.generation_id {
                info!(
                    "[{}] [{}] {} subscriber session marked stale because codec changed: subscriber_generation={}, publish_generation={}",
                    stream,
                    id,
                    kind,
                    forward_channel.generation_id,
                    publish_track.generation_id(),
                );
                return None;
            }

            let publisher_codec = match publish_track {
                PublishTrackRemote::Real { track, .. } => {
                    let ssrcs = track.ssrcs().await;
                    let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                    track.codec(first_ssrc).await.unwrap_or_default()
                }
                #[cfg(feature = "source")]
                PublishTrackRemote::Virtual(v) => v.codec_params.rtp_codec.clone(),
            };

            // Normalize the publisher codec to match what the sender was
            // created with: zero constraint byte in profile-level-id, strip
            // sprop-parameter-sets.  Without this, rtp_codecs_match won't
            // find the exact match (PT 103) and select_compatible_codec
            // falls back to the first H264 in MediaEngine order (PT 119,
            // High Profile) which Chrome rejects.
            let publisher_codec = Self::normalize_publisher_codec(publisher_codec);

            let (codec, payload_type) =
                Self::select_sender_codec(stream, id, kind, sender, publisher_codec.clone()).await;

            let sender_track = sender.track();
            let sender_track_codec = sender_track.codec(sender_ssrc).await;
            let track: Arc<dyn TrackLocal> = if sender_track_codec.as_ref().is_some_and(
                |sender_track_codec| sender_track_codec_compatible(sender_track_codec, &codec),
            ) {
                info!(
                    "[{}] [{}] {} subscribe reusing bound sender track: sender_codec={}, selected_codec={}, payload_type={:?}, ssrc={}",
                    stream,
                    id,
                    kind,
                    Self::format_codec(sender_track_codec.as_ref().expect("checked above")),
                    Self::format_codec(&codec),
                    payload_type,
                    sender_ssrc,
                );
                sender_track.clone()
            } else {
                let new_track = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
                    "webrtc".to_string(),
                    format!("{}-{}", "webrtc", kind),
                    "webrtc".to_string(),
                    kind,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(sender_ssrc),
                            ..Default::default()
                        },
                        codec: codec.clone(),
                        ..Default::default()
                    }],
                )));

                if let Err(e) = sender.replace_track(new_track.clone()).await {
                    debug!("[{}] [{}] {} track replace err: {}", stream, id, kind, e);
                    break;
                }

                info!(
                    "[{}] [{}] {} subscribe replaced sender track: previous_codec={}, selected_codec={}, payload_type={:?}, ssrc={}",
                    stream,
                    id,
                    kind,
                    sender_track_codec
                        .as_ref()
                        .map(Self::format_codec)
                        .unwrap_or_else(|| "<none>".to_string()),
                    Self::format_codec(&codec),
                    payload_type,
                    sender_ssrc,
                );
                new_track
            };

            let new_recv = publish_track.subscribe();

            let ssrc = match publish_track {
                PublishTrackRemote::Real { track, .. } => {
                    let ssrcs = track.ssrcs().await;
                    ssrcs.first().copied().unwrap_or(0)
                }
                #[cfg(feature = "source")]
                PublishTrackRemote::Virtual(v) => v.ssrc(),
            };

            let _ = forward_channel
                .publish_rtcp_sender
                .send((RtcpMessage::PictureLossIndication, ssrc));

            {
                let mut binding = track_binding_publish_rid.write().await;
                // Validate: only update if the binding hasn't been changed
                // by a concurrent handler since we read current_rid above.
                // (Currently the handlers are sequential within this task,
                // but this guards against future refactors that introduce
                // parallelism.)
                if binding.get(&kind.to_string()).map(|r| r.as_str()) == current_rid.as_deref() {
                    binding.insert(kind.to_string(), publish_track.rid().to_string());
                }
            }
            return Some(BoundPublishTrack {
                recv: new_recv,
                track,
                payload_type,
                source_codec: publisher_codec,
                selected_codec: codec,
            });
        }
        None
    }

    fn format_codec(codec: &RTCRtpCodec) -> String {
        format!(
            "{}/{}/channels={}/fmtp={}",
            codec.mime_type, codec.clock_rate, codec.channels, codec.sdp_fmtp_line
        )
    }

    /// Normalize a publisher codec so it can be matched against the
    /// subscriber's sender codecs.  Zeroes the H264 profile-level-id
    /// constraint byte and strips sprop keys (browsers get SPS/PPS
    /// in-band).  The sender was already created with the normalized
    /// codec; this ensures `rtp_codecs_match` finds the exact match.
    fn normalize_publisher_codec(mut codec: RTCRtpCodec) -> RTCRtpCodec {
        use crate::forward::codec_compat::{is_av1_codec, is_h264_codec, is_h265_codec};
        if is_h264_codec(&codec) || is_h265_codec(&codec) || is_av1_codec(&codec) {
            // H264: strip sprop-parameter-sets (browsers receive SPS/PPS
            // in-band).  H265: keep sprop-vps/sps/pps — they are required
            // for webrtc-rs sender codec matching in internal.rs.
            let sprop_keys: &[&str] = if is_h264_codec(&codec) {
                &["sprop-parameter-sets"]
            } else {
                &[]
            };
            let plid_key: Option<&str> =
                if is_h264_codec(&codec) { Some("profile-level-id") } else { None };
            codec.sdp_fmtp_line = codec
                .sdp_fmtp_line
                .split(';')
                .filter_map(|part| {
                    let trimmed = part.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    if let Some((k, v)) = trimmed.split_once('=') {
                        let key = k.trim();
                        if sprop_keys.iter().any(|sk| key.eq_ignore_ascii_case(sk)) {
                            return None;
                        }
                        if let Some(plk) = plid_key {
                            if key.eq_ignore_ascii_case(plk) && v.len() == 6 {
                                let normalized = format!("{}00{}", &v[0..2], &v[4..6]).to_ascii_lowercase();
                                return Some(format!("{}={}", key, normalized));
                            }
                        }
                    }
                    Some(trimmed.to_owned())
                })
                .collect::<Vec<_>>()
                .join(";");
        }
        codec
    }

    async fn select_sender_codec(
        stream: &str,
        id: &str,
        kind: RtpCodecKind,
        sender: &Arc<dyn RtpSender>,
        publisher_codec: RTCRtpCodec,
    ) -> (RTCRtpCodec, Option<PayloadType>) {
        let Ok(params) = sender.get_parameters().await else {
            return (publisher_codec, None);
        };

        if params.rtp_parameters.codecs.is_empty() {
            return (publisher_codec, None);
        }

        // Log the full sender codec list to diagnose PT selection issues.
        let codec_list: Vec<String> = params
            .rtp_parameters
            .codecs
            .iter()
            .map(|c| {
                format!(
                    "pt={}/{}",
                    c.payload_type,
                    c.rtp_codec.mime_type
                )
            })
            .collect();
        info!(
            "[{}] [{}] {} sender codecs ({}): {:?}; publisher={}",
            stream,
            id,
            kind,
            codec_list.len(),
            codec_list,
            Self::format_codec(&publisher_codec)
        );

        let matched = select_compatible_codec(&publisher_codec, &params.rtp_parameters.codecs);

        let selected = match matched {
            Some(c) => c,
            None => {
                warn!(
                    "[{}] [{}] {} publisher codec {} is not compatible with subscriber send_codecs",
                    stream,
                    id,
                    kind,
                    Self::format_codec(&publisher_codec)
                );
                return (publisher_codec, None);
            }
        };

        if selected.rtp_codec.mime_type.is_empty() {
            return (publisher_codec, None);
        }

        // Use the selected codec as-is without merging publisher sprop.
        // Browsers receive SPS/PPS/VPS in-band from the RTP stream.
        let selected_rtp_codec = selected.rtp_codec.clone();

        let mut updated_params = params.clone();
        for encoding in updated_params.encodings.iter_mut() {
            encoding.codec = selected_rtp_codec.clone();
        }
        if let Err(e) = sender.set_parameters(updated_params, None).await {
            debug!(
                "[{}] [{}] {} failed to update encoding codec: {}",
                stream, id, kind, e
            );
        }

        (selected_rtp_codec, Some(selected.payload_type))
    }

    fn is_transient_track_write_error(err: &impl Display) -> bool {
        let message = err.to_string();
        message.contains("local_srtp_context is not set yet")
            || message.contains("track is not binding yet")
    }

    fn current_connection_state(
        connection_state: &Arc<std::sync::RwLock<RTCPeerConnectionState>>,
    ) -> RTCPeerConnectionState {
        connection_state
            .read()
            .map(|state| *state)
            .unwrap_or(RTCPeerConnectionState::New)
    }

    fn is_terminal_connection_state(state: RTCPeerConnectionState) -> bool {
        matches!(
            state,
            RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
        )
    }

    fn spawn_startup_pli_burst(
        stream: String,
        id: String,
        kind: RtpCodecKind,
        publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
        source_ssrc: u32,
    ) {
        if kind != RtpCodecKind::Video || source_ssrc == 0 {
            return;
        }

        tokio::spawn(async move {
            for delay in [
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
            ] {
                sleep(delay).await;
                if publish_rtcp_sender
                    .send((RtcpMessage::PictureLossIndication, source_ssrc))
                    .is_err()
                {
                    debug!(
                        "[{}] [{}] {} startup PLI burst stopped for source ssrc {}",
                        stream, id, kind, source_ssrc
                    );
                    break;
                }
                debug!(
                    "[{}] [{}] {} startup PLI burst sent for source ssrc {}",
                    stream, id, kind, source_ssrc
                );
            }
        });
    }

    async fn sender_forward_rtp(
        stream: String,
        id: String,
        sender: Arc<dyn RtpSender>,
        kind: RtpCodecKind,
        track_binding_publish_rid: Arc<RwLock<HashMap<String, String>>>,
        publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
        mut forward_channel: SubscribeForwardChannel,
    ) {
        info!("[{}] [{}] {} up", stream, id, kind);

        let sender_ssrc = match sender.get_parameters().await {
            Ok(params) => params
                .encodings
                .first()
                .and_then(|e| e.rtp_coding_parameters.ssrc)
                .unwrap_or_else(rand::random::<u32>),
            Err(_) => rand::random::<u32>(),
        };

        let mut pre_rid: Option<String> = None;
        let virtual_sender = new_broadcast_channel!(1);
        let mut recv = virtual_sender.subscribe();
        let mut track = None;
        let mut first_packet = true;
        let mut transient_write_error_since = None;
        let mut source_codec = None;
        let mut selected_codec = None;

        // Check for existing publish tracks immediately at startup,
        // so we don't depend on a potentially-missed publish_track_change event.
        let mut payload_type = None;

        if let Some(bound) = Self::try_bind_publish_track(
            &stream,
            &id,
            &sender,
            kind,
            sender_ssrc,
            &track_binding_publish_rid,
            &publish_tracks,
            &forward_channel,
            &virtual_sender,
        )
        .await
        {
            recv = bound.recv;
            track = Some(bound.track);
            payload_type = bound.payload_type;
            source_codec = Some(bound.source_codec);
            selected_codec = Some(bound.selected_codec);
            transient_write_error_since = None;
        }

        loop {
            tokio::select! {
                publish_change = forward_channel.publish_track_change.recv() => {
                    debug!("{} {} recv publish track_change", stream, id);

                    if publish_change.is_err() {
                        continue;
                    }

                    {
                        let mut rid_map = track_binding_publish_rid.write().await;
                        let pts = publish_tracks.read().await;
                        let current_rid = rid_map.get(&kind.to_string());

                        if pts.is_empty() {
                            debug!("{} {} publish track len 0 , probably offline", stream, id);
                            recv = virtual_sender.subscribe();
                            track = None;
                            payload_type = None;
                            source_codec = None;
                            selected_codec = None;
                            pre_rid = None;

                            if current_rid.is_some() && current_rid.cloned().unwrap() != constant::RID_DISABLE {
                                rid_map.remove(&kind.to_string());
                            }
                            continue;
                        }

                        if track.is_some() {
                            continue;
                        }

                        if current_rid.is_some() && current_rid.cloned().unwrap() == constant::RID_DISABLE {
                            continue;
                        }
                    }

                    if let Some(bound) = Self::try_bind_publish_track(
                        &stream, &id, &sender, kind, sender_ssrc,
                        &track_binding_publish_rid, &publish_tracks, &forward_channel, &virtual_sender,
                    ).await {
                        recv = bound.recv;
                        track = Some(bound.track);
                        payload_type = bound.payload_type;
                        source_codec = Some(bound.source_codec);
                        selected_codec = Some(bound.selected_codec);
                        transient_write_error_since = None;
                    }
                }

                rtp_result = recv.recv() => {
                    match rtp_result {
                        Ok(packet) => {
                            match track {
                                None => continue,
                                Some(ref track) => {
                                    let mut packet = packet.as_ref().clone();
                                    let source_ssrc = packet.header.ssrc;
                                    let input_payload_type = packet.header.payload_type;
                                    // Rewrite SSRC to match the sender's SSRC.
                                    // The rtc-layer write_rtp validates that packet.ssrc
                                    // is in sender.track().ssrcs(), so it must match.
                                    packet.header.ssrc = sender_ssrc;
                                    if let Some(payload_type) = payload_type {
                                        packet.header.payload_type = payload_type;
                                    }
                                    let outgoing_payload_type = packet.header.payload_type;
                                    // Header extension ids are negotiated per PeerConnection.
                                    // Publisher-side MID/RID/TWCC extension ids may not match
                                    // the WHEP subscriber's extmap, while the subscriber already
                                    // has this SSRC declared in its SDP answer.

                                    packet.header.extension = false;
                                    packet.header.extension_profile = 0;
                                    packet.header.extensions.clear();
                                    packet.header.extensions_padding = 0;

                                    // Fix H264 STAP-A NRI header.
                                    // ffmpeg's RTSP muxer sets NRI=0 in
                                    // the STAP-A header byte, but
                                    // RFC 6184 §5.6 requires NRI to be
                                    // the maximum of all aggregated NAL
                                    // units.  Chrome enforces this and
                                    // silently drops STAP-A packets
                                    // with incorrect NRI.
                                    if packet.payload.len() > 1
                                        && (packet.payload[0] & 0x1F) == 24
                                    {
                                        let mut max_nri: u8 = 0;
                                        let mut pos = 1usize;
                                        let payload = &packet.payload;
                                        while pos + 2 <= payload.len() {
                                            let size =
                                                u16::from_be_bytes([
                                                    payload[pos],
                                                    payload[pos + 1],
                                                ]) as usize;
                                            pos += 2;
                                            if size == 0
                                                || pos + size > payload.len()
                                            {
                                                break;
                                            }
                                            let nri =
                                                (payload[pos] >> 5) & 0x03;
                                            if nri > max_nri {
                                                max_nri = nri;
                                            }
                                            pos += size;
                                        }
                                        if max_nri > 0 {
                                            let mut payload_mut =
                                                packet.payload.to_vec();
                                            payload_mut[0] = (payload_mut[0]
                                                & 0x9F)
                                                | (max_nri << 5);
                                            packet.payload =
                                                bytes::Bytes::from(payload_mut);
                                        }
                                    }

                                    // H264 STAP-A NRI fix applied above when needed.

                                    if let Err(err) = track.write_rtp(packet).await {
                                        if Self::is_transient_track_write_error(&err) {
                                            let now = Instant::now();
                                            let started = transient_write_error_since.get_or_insert(now);
                                            let elapsed = started.elapsed();
                                            let state = Self::current_connection_state(
                                                &forward_channel.connection_state,
                                            );
                                            if Self::is_terminal_connection_state(state) {
                                                warn!(
                                                    "[{}] [{}] {} track write stopped after transient error because peer is {}: {}",
                                                    stream, id, kind, state, err
                                                );
                                                break;
                                            }
                                            if elapsed >= TRACK_BIND_RETRY_TIMEOUT {
                                                warn!(
                                                    "[{}] [{}] {} track write still not ready after {}ms, state={}, source_codec={}, selected_codec={}, payload_type={:?}, input_payload_type={}, outgoing_payload_type={}, ssrc={}: {}",
                                                    stream,
                                                    id,
                                                    kind,
                                                    elapsed.as_millis(),
                                                    state,
                                                    source_codec
                                                        .as_ref()
                                                        .map(Self::format_codec)
                                                        .unwrap_or_else(|| "<none>".to_string()),
                                                    selected_codec
                                                        .as_ref()
                                                        .map(Self::format_codec)
                                                        .unwrap_or_else(|| "<none>".to_string()),
                                                    payload_type,
                                                    input_payload_type,
                                                    outgoing_payload_type,
                                                    sender_ssrc,
                                                    err
                                                );
                                                break;
                                            }
                                            debug!(
                                                "[{}] [{}] {} track write deferred for {}ms, state={}: {}",
                                                stream,
                                                id,
                                                kind,
                                                elapsed.as_millis(),
                                                state,
                                                err
                                            );
                                            sleep(TRACK_BIND_RETRY_DELAY).await;
                                            continue;
                                        }
                                        warn!("[{}] [{}] {} track write err: {}", stream, id, kind, err);
                                        break;
                                    }
                                    transient_write_error_since = None;
                                    if first_packet {
                                        info!(
                                            "[{}] [{}] {} first RTP packet written: source_codec={}, selected_codec={}, payload_type={:?}, ssrc={}",
                                            stream, id, kind,
                                            source_codec.as_ref().map(Self::format_codec).unwrap_or_else(|| "<none>".to_string()),
                                            selected_codec.as_ref().map(Self::format_codec).unwrap_or_else(|| "<none>".to_string()),
                                            payload_type, sender_ssrc,
                                        );
                                        if kind == RtpCodecKind::Video {
                                            let _ = forward_channel
                                                .publish_rtcp_sender
                                                .send((RtcpMessage::PictureLossIndication, source_ssrc));
                                            debug!(
                                                "[{}] [{}] {} sent first-packet PLI for source ssrc {}",
                                                stream, id, kind, source_ssrc
                                            );
                                            Self::spawn_startup_pli_burst(
                                                stream.clone(),
                                                id.clone(),
                                                kind,
                                                forward_channel.publish_rtcp_sender.clone(),
                                                source_ssrc,
                                            );
                                        }
                                        first_packet = false;
                                    } // if first_packet
                            } // Some(ref track)
                            } // match track
                        } // Ok(packet)
                        Err(err) => {
                            warn!("[{}] [{}] {} rtp receiver err: {}", stream, id, kind, err);
                        }
                    }
                }

                select_layer_result = forward_channel.select_layer_recv.recv() => {
                    match select_layer_result {
                        Ok(select_layer_body) => {
                            if select_layer_body.0 != kind {
                                continue;
                            }

                            let select_rid = select_layer_body.1;

                            // Read the current binding without taking a write lock.
                            let current_rid = {
                                let binding = track_binding_publish_rid.read().await;
                                binding.get(&kind.to_string()).cloned()
                            };

                            if current_rid == Some(select_rid.clone()) {
                                continue;
                            }

                            // Disabling the layer is a synchronous map update.
                            if select_rid == constant::RID_DISABLE {
                                if let Some(ref rid) = current_rid {
                                    recv = virtual_sender.subscribe();
                                    track = None;
                                    payload_type = None;
                                    source_codec = None;
                                    selected_codec = None;
                                    pre_rid = Some(rid.clone());
                                    {
                                        let mut binding = track_binding_publish_rid.write().await;
                                        if binding.get(&kind.to_string()).map(|r| r.as_str()) == current_rid.as_deref() {
                                            binding.insert(kind.to_string(), select_rid);
                                        }
                                    }
                                }
                                continue;
                            }

                            // Re-enable from disabled: pick the previously active RID or the
                            // first available publish track of this kind.
                            let new_rid: Option<String> = match &current_rid {
                                None => Some(select_rid.clone()),
                                Some(current_rid)
                                    if current_rid == constant::RID_DISABLE
                                        && select_rid == constant::RID_ENABLE =>
                                {
                                    let publish_tracks = publish_tracks.read().await;
                                    match &pre_rid {
                                        Some(pre_rid) => Some(pre_rid.clone()),
                                        None => publish_tracks
                                            .iter()
                                            .find(|t| t.kind() == kind)
                                            .map(|t| t.rid().to_string()),
                                    }
                                }
                                _ => Some(select_rid.clone()),
                            };

                            let Some(new_rid) = new_rid else { continue; };

                            // Find the target publish track without holding the write lock.
                            let publish_tracks = publish_tracks.read().await;
                            let publish_track = publish_tracks.iter().find(|t| {
                                t.kind() == kind
                                    && (t.rid() == new_rid || new_rid == constant::RID_ENABLE)
                            });
                            let Some(publish_track) = publish_track else { continue; };

                            if publish_track.generation_id() != forward_channel.generation_id {
                                info!(
                                    "[{}] [{}] {} subscriber session marked stale because codec changed: subscriber_generation={}, publish_generation={}",
                                    stream,
                                    id,
                                    kind,
                                    forward_channel.generation_id,
                                    publish_track.generation_id(),
                                );
                                continue;
                            }

                            let publisher_codec = match publish_track {
                                PublishTrackRemote::Real { track, .. } => {
                                    let ssrcs = track.ssrcs().await;
                                    let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                                    track.codec(first_ssrc).await.unwrap_or_default()
                                }
                                #[cfg(feature = "source")]
                                PublishTrackRemote::Virtual(v) => v.codec_params.rtp_codec.clone(),
                            };
                            let (codec, new_payload_type) =
                                Self::select_sender_codec(&stream, &id, kind, &sender, publisher_codec.clone()).await;

                            // Reuse the already-bound sender track when the codec is
                            // compatible to avoid webrtc-rs' replace_track, which does not
                            // re-bind the new track and causes 'track is not binding yet'.
                            let sender_track = sender.track();
                            let sender_track_codec = sender_track.codec(sender_ssrc).await;
                            let track_to_use: Arc<dyn TrackLocal> = if sender_track_codec.as_ref().is_some_and(
                                |sender_track_codec| {
                                    sender_track_codec_compatible(sender_track_codec, &codec)
                                }
                            ) {
                                info!(
                                    "[{}] [{}] {} layer switch reusing bound sender track: sender_codec={}, selected_codec={}, payload_type={:?}, ssrc={}",
                                    stream,
                                    id,
                                    kind,
                                    Self::format_codec(sender_track_codec.as_ref().expect("checked above")),
                                    Self::format_codec(&codec),
                                    new_payload_type,
                                    sender_ssrc,
                                );
                                sender_track.clone()
                            } else {
                                let new_track = Arc::new(TrackLocalStaticRTP::new(
                                    MediaStreamTrack::new(
                                        "webrtc".to_string(),
                                        format!("{}-{}", "webrtc", kind),
                                        "webrtc".to_string(),
                                        kind,
                                        vec![RTCRtpEncodingParameters {
                                            rtp_coding_parameters: RTCRtpCodingParameters {
                                                ssrc: Some(sender_ssrc),
                                                ..Default::default()
                                            },
                                            codec: codec.clone(),
                                            ..Default::default()
                                        }],
                                    ),
                                ));

                                if let Err(e) = sender.replace_track(new_track.clone() as Arc<dyn webrtc::media_stream::track_local::TrackLocal>).await {
                                    debug!("[{}] [{}] {} track replace err: {}", stream, id, kind, e);
                                    continue;
                                }

                                info!(
                                    "[{}] [{}] {} layer switch replaced sender track: previous_codec={}, selected_codec={}, payload_type={:?}, ssrc={}",
                                    stream,
                                    id,
                                    kind,
                                    sender_track_codec
                                        .as_ref()
                                        .map(Self::format_codec)
                                        .unwrap_or_else(|| "<none>".to_string()),
                                    Self::format_codec(&codec),
                                    new_payload_type,
                                    sender_ssrc,
                                );
                                new_track
                            };

                            recv = publish_track.subscribe();
                            track = Some(track_to_use);
                            payload_type = new_payload_type;
                            source_codec = Some(publisher_codec);
                            selected_codec = Some(codec);
                            transient_write_error_since = None;

                            let ssrc = match publish_track {
                                PublishTrackRemote::Real { track, .. } => {
                                    let ssrcs = track.ssrcs().await;
                                    ssrcs.first().copied().unwrap_or(0)
                                }
                                #[cfg(feature = "source")]
                                PublishTrackRemote::Virtual(v) => v.ssrc(),
                            };

                            let _ = forward_channel
                                .publish_rtcp_sender
                                .send((RtcpMessage::PictureLossIndication, ssrc));

                            {
                                let mut binding = track_binding_publish_rid.write().await;
                                // Validate: only update if the binding hasn't changed
                                // since we read current_rid above. Guards against
                                // future refactors that introduce parallel handlers.
                                if binding.get(&kind.to_string()).map(|r| r.as_str()) == current_rid.as_deref() {
                                    binding.insert(kind.to_string(), new_rid.clone());
                                }
                            }
                            info!("[{}] [{}] {} select layer to {}", stream, id, kind, new_rid);
                        }
                        Err(e) => {
                            debug!("select_layer_recv err : {:?}", e);
                            break;
                        }
                    }
                }
            }
        }

        debug!("[{}] [{}] {} down", stream, id, kind);
    }

    pub(crate) fn select_kind_rid(&self, kind: RtpCodecKind, rid: String) -> Result<()> {
        if let Err(err) = self.select_layer_sender.send((kind, rid)) {
            Err(AppError::throw(format!("select layer send err: {err}")))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters;

    #[test]
    fn track_not_binding_yet_is_a_transient_track_write_error() {
        assert!(SubscribeRTCPeerConnection::is_transient_track_write_error(
            &"track is not binding yet"
        ));
    }

    #[test]
    fn g722_codec_match_ignores_case_and_compares_clock_rate() {
        let track_codec = RTCRtpCodec {
            mime_type: "audio/G722".to_string(),
            clock_rate: 8000,
            channels: 0,
            sdp_fmtp_line: "".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "audio/g722".to_string(),
            clock_rate: 8000,
            channels: 0,
            sdp_fmtp_line: "".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(rtp_codecs_match(&track_codec, &selected_codec));
    }

    #[test]
    fn h264_codec_selection_prefers_matching_fmtp_over_first_h264() {
        let source_codec = RTCRtpCodec {
            mime_type: "video/H264".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                .to_string(),
            rtcp_feedback: vec![],
        };
        let high_profile = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H264".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=640032"
                        .to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 123,
        };
        let baseline_profile = RTCRtpCodecParameters {
            rtp_codec: source_codec.clone(),
            payload_type: 102,
        };

        let selected = select_compatible_codec(&source_codec, &[high_profile, baseline_profile])
            .expect("matching H264 codec should be selected");

        assert_eq!(selected.payload_type, 102);
        assert_eq!(selected.rtp_codec.sdp_fmtp_line, source_codec.sdp_fmtp_line);
    }

    #[test]
    fn h265_codec_selection_prefers_matching_profile_over_first_h265() {
        let source_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=123;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };
        let main_10_profile = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H265".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "level-id=180;profile-id=2;tier-flag=0;tx-mode=SRST".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 51,
        };
        let main_profile = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H265".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "level-id=180;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 49,
        };

        let selected = select_compatible_codec(&source_codec, &[main_10_profile, main_profile])
            .expect("matching H265 profile should be selected");

        assert_eq!(selected.payload_type, 49);
        assert!(selected.rtp_codec.sdp_fmtp_line.contains("profile-id=1"));
    }

    #[test]
    fn h265_sender_track_reuse_allows_lower_level_with_same_profile() {
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=180;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=123;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn h265_sender_track_reuse_rejects_insufficient_level() {
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=93;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=180;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(!sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn h265_codec_selection_accepts_subscriber_omitting_level_id() {
        // A subscriber offer that omits level-id (common from Safari/WebKit)
        // must not be rejected just because the publisher advertises a high
        // level. The level gate only applies when both sides declare level-id.
        let source_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=180;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };
        let subscriber = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H265".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 49,
        };

        let selected = select_compatible_codec(&source_codec, &[subscriber])
            .expect("subscriber omitting level-id should match a high-level publisher");
        assert_eq!(selected.payload_type, 49);
    }

    #[test]
    fn h265_codec_selection_rejects_insufficient_subscriber_level() {
        // When both sides declare level-id, a subscriber whose level is below
        // the publisher's cannot receive the stream.
        let source_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=180;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };
        let subscriber = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H265".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "level-id=93;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 49,
        };

        assert!(
            select_compatible_codec(&source_codec, &[subscriber]).is_none(),
            "subscriber at Level 3.1 cannot receive a Level 6.0 stream"
        );
    }

    #[test]
    fn h265_codec_selection_accepts_missing_candidate_profile_id() {
        // Browsers such as Safari/WebKit may omit profile-id from their offer;
        // the inferred default is Main profile (profile-id=1), so a Main-profile
        // publisher should still be compatible.
        let source_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=1;tier-flag=0;sprop-vps=QAEMAf//AWAAAAMAkAAAAwAAAwBaoA==;sprop-sps=QgEBAWAAAAMAkAAAAwAAAwBaoA==;sprop-pps=RAHAcYMS".to_string(),
            rtcp_feedback: vec![],
        };
        let webkit_codec = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H265".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "sprop-vps=QAEMAf//AWAAAAMAkAAAAwAAAwBaoA==;sprop-sps=QgEBAWAAAAMAkAAAAwAAAwBaoA==;sprop-pps=RAHAcYMS".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 35,
        };

        assert!(h265_codecs_are_compatible(
            &webkit_codec.rtp_codec,
            &source_codec
        ));
    }

    #[test]
    fn h265_codec_selection_rejects_incompatible_profile_id() {
        // Chromium may offer H265 with profile-id=2 (Main 10), which cannot
        // decode a Main-profile (profile-id=1) bitstream.
        let source_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=1;tier-flag=0;sprop-vps=QAEMAf//AWAAAAMAkAAAAwAAAwBaoA==;sprop-sps=QgEBAWAAAAMAkAAAAwAAAwBaoA==;sprop-pps=RAHAcYMS".to_string(),
            rtcp_feedback: vec![],
        };
        let main_10_profile = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H265".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=2;tier-flag=0;level-id=180;tx-mode=SRST".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 51,
        };

        assert!(!h265_codecs_are_compatible(
            &main_10_profile.rtp_codec,
            &source_codec
        ));
    }

    #[test]
    fn h265_merge_sprop_overrides_bitstream_params_from_publisher() {
        let publisher_fmtp =
            "profile-id=1;tier-flag=0;level-id=90;sprop-vps=VPS;sprop-sps=SPS;sprop-pps=PPS";
        let selected_fmtp = "tx-mode=SRST;profile-id=2;tier-flag=1;level-id=180;sprop-vps=OLD";
        let merged = merge_h265_sprop(publisher_fmtp, selected_fmtp);
        assert!(merged.contains("profile-id=1"));
        assert!(merged.contains("tier-flag=0"));
        assert!(merged.contains("level-id=90"));
        assert!(merged.contains("tx-mode=SRST"));
        assert!(merged.contains("sprop-vps=VPS"));
        assert!(merged.contains("sprop-sps=SPS"));
        assert!(merged.contains("sprop-pps=PPS"));
        assert!(!merged.contains("profile-id=2"));
        assert!(!merged.contains("sprop-vps=OLD"));
    }

    #[test]
    fn av1_sender_track_reuse_allows_lower_level_idx_with_same_profile() {
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=3;tier=0".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn av1_sender_track_reuse_accepts_higher_existing_profile() {
        // The bound sender track was negotiated with a higher profile than the
        // new selected stream, so it can be reused.
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=2;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=3;tier=0".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn av1_sender_track_reuse_rejects_incompatible_profile() {
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=2;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(!sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn av1_sender_track_reuse_accepts_profile_parameter_name() {
        // Chrome answers use `profile`, while rtc-rs uses `profile-id`.
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=1;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile=0;level-idx=3;tier=0".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn av1_sender_track_reuse_rejects_higher_tier() {
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=3;tier=1".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(!sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn video_codec_selection_does_not_fallback_to_different_codec_family() {
        let source_codec = RTCRtpCodec {
            mime_type: "video/H265".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-id=186;profile-id=1;tier-flag=0;tx-mode=SRST".to_string(),
            rtcp_feedback: vec![],
        };
        let subscriber_vp8 = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/VP8".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 96,
        };

        assert!(select_compatible_codec(&source_codec, &[subscriber_vp8],).is_none());
    }

    #[test]
    fn av1_codec_selection_prefers_matching_profile() {
        let source_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let profile_1 = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/AV1".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile=1;level-idx=5;tier=0".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 47,
        };
        let profile_0 = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/AV1".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=0;level-idx=5;tier=0".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 41,
        };

        let selected = select_compatible_codec(&source_codec, &[profile_1, profile_0])
            .expect("matching AV1 profile should be selected");

        assert_eq!(selected.payload_type, 41);
        assert!(selected.rtp_codec.sdp_fmtp_line.contains("profile-id=0"));
    }

    #[test]
    fn av1_codecs_with_mismatched_profile_are_incompatible() {
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile=1;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(!sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }

    #[test]
    fn av1_codecs_with_matching_profile_are_compatible() {
        let sender_track_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0;level-idx=5;tier=0".to_string(),
            rtcp_feedback: vec![],
        };
        let selected_codec = RTCRtpCodec {
            mime_type: "video/AV1".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "profile=0;level-idx=3;tier=0".to_string(),
            rtcp_feedback: vec![],
        };

        assert!(sender_track_codec_compatible(
            &sender_track_codec,
            &selected_codec
        ));
    }
}
