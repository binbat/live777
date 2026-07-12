use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use libwish::Client;
use rtc::peer_connection::configuration::interceptor_registry::{
    configure_nack, configure_rtcp_reports, configure_simulcast_extension_headers, configure_twcc,
};
use rtc::peer_connection::configuration::media_engine::{
    MIME_TYPE_AV1, MIME_TYPE_HEVC, MediaEngine,
};
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RtpCodecKind,
};
use rtc::statistics::StatsSelector;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use webrtc::media_stream::Track;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceConnectionState, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState,
    RTCSignalingState, Registry,
};

use crate::source::{AudioCodec, MediaFrame, VideoCodec, extract_h265_sprop};
use crate::utils;
use crate::whipsynth::SessionStats;
use crate::whipsynth::packetizer::{Packetizer, PacketizerConfig};
use crate::whipsynth::source::{frame_generator_config, spawn_rsmpeg_source};

const WAIT_FOR_PEER_CONNECTED_TIMEOUT: Duration = Duration::from_secs(15);

/// Configuration for a [`Publisher`].
#[derive(Debug, Clone)]
pub struct PublisherConfig {
    pub whip_url: String,
    pub token: Option<String>,
    pub video_codec: VideoCodec,
    pub audio_codec: Option<AudioCodec>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration: Option<Duration>,
    /// STUN server URL used for ICE gathering.
    pub stun_server: String,
}

/// Direct WHIP publisher that feeds encoded frames from a local source into a
/// `webrtc` PeerConnection without an intermediate RTP/UDP bridge.
pub struct Publisher {
    config: PublisherConfig,
}

impl Publisher {
    /// Create a new publisher from the supplied configuration.
    pub fn new(config: PublisherConfig) -> Self {
        Self { config }
    }

    /// Run the publisher until cancelled or the configured duration expires.
    ///
    /// Returns the final session statistics on success.
    pub async fn run(self, ct: CancellationToken) -> Result<SessionStats> {
        let input_id = format!("whipsynth-{}", rand::random::<u64>());

        let mut client = Client::new(
            self.config.whip_url.clone(),
            Client::get_auth_header_map(self.config.token.clone()),
        );

        let gather_complete = Arc::new(Notify::new());
        let (peer, mut packetizer, state_rx, diagnostics) =
            create_peer(&self.config, gather_complete.clone(), ct.clone()).await?;

        let video_track = packetizer
            .video_track(&input_id)
            .context("video track required")?;
        let audio_track = packetizer.audio_track(&input_id);

        peer.add_track(video_track.clone())
            .await
            .map_err(|error| anyhow!("{:?}", error))?;

        if let Some(ref audio) = audio_track {
            peer.add_track(audio.clone())
                .await
                .map_err(|error| anyhow!("{:?}", error))?;
        }

        info!("WHIP publisher peer created; starting signaling");
        utils::webrtc::setup_connection(peer.clone(), &mut client, gather_complete).await?;
        info!(
            "Local SDP offer summary:\n{}",
            peer.local_description()
                .await
                .map(|description| utils::webrtc::summarize_sdp(&description.sdp))
                .unwrap_or_else(|| "<no local description>".to_string())
        );
        info!(
            "Remote SDP answer summary:\n{}",
            peer.remote_description()
                .await
                .map(|description| utils::webrtc::summarize_sdp(&description.sdp))
                .unwrap_or_else(|| "<no remote description>".to_string())
        );

        wait_for_peer_connected(peer.clone(), state_rx.clone(), diagnostics.clone()).await?;
        info!("WHIP publisher peer connected");

        // After SDP negotiation the answer may have remapped the dynamic payload
        // types; update the packetizer so the outbound RTP headers match.
        update_payload_types(&peer, &mut packetizer, &video_track, &audio_track).await;

        let connected_at = std::time::Instant::now();
        let stats = Arc::new(Mutex::new(SessionStats::default()));

        let frame_config = frame_generator_config(
            self.config.video_codec,
            self.config.audio_codec,
            self.config.width,
            self.config.height,
            self.config.fps,
            self.config.duration,
        );

        let (frame_rx, source_handle) = spawn_rsmpeg_source(frame_config, ct.child_token())?;
        let write_handle = tokio::spawn(run_write_loop(
            frame_rx,
            packetizer,
            video_track,
            audio_track,
            stats.clone(),
            connected_at,
            ct.clone(),
        ));

        let result: Result<()> = tokio::select! {
            _ = ct.cancelled() => {
                info!("Shutdown signal received, stopping WHIP publisher");
                Ok(())
            }
            result = wait_for_unexpected_peer_end(peer.clone(), state_rx, diagnostics.clone()) => {
                ct.cancel();
                result
            }
            result = write_handle => {
                ct.cancel();
                result.map_err(|e| anyhow!("publisher write task panicked: {}", e))?;
                Ok(())
            }
        };

        // Collect RTCP feedback counters from outbound RTP stats before tearing
        // down the peer connection.
        let rtcp_feedback = collect_rtcp_feedback(&peer).await;

        // Teardown. Propagate source errors so a failed encoder does not look
        // like a successful session.
        let source_result = source_handle.stop().await;
        if let Err(e) = &source_result {
            error!(error = ?e, "WHIP publisher source task failed");
        }
        if let Err(e) = peer.close().await {
            warn!("Failed to close WHIP publisher peer: {}", e);
        }
        if let Err(e) = client.remove_resource().await {
            debug!("Failed to remove WHIP resource: {}", e);
        }

        let mut final_stats = stats.lock().map(|s| s.clone()).unwrap_or_default();
        final_stats.nack_count = rtcp_feedback.nack_count;
        final_stats.pli_count = rtcp_feedback.pli_count;
        // Compute connected_duration here so it is always set, even when the
        // write loop exits early (e.g. due to cancellation) and skips its own
        // duration update.
        final_stats.connected_duration = connected_at.elapsed();
        info!(
            packets_sent = final_stats.packets_sent,
            connected_ms = final_stats.connected_duration.as_millis() as u64,
            "WHIP publisher session ended"
        );

        // Propagate peer/write-loop errors first, then source errors.
        result.and(source_result).map(|_| final_stats)
    }
}

/// Pull the negotiated payload types from the peer's RTP senders and apply
/// them to the packetizer. This ensures outbound RTP headers use the dynamic
/// payload types assigned by the WHIP answer rather than our default values.
async fn update_payload_types(
    peer: &Arc<dyn PeerConnection>,
    packetizer: &mut Packetizer,
    video_track: &Arc<TrackLocalStaticRTP>,
    audio_track: &Option<Arc<TrackLocalStaticRTP>>,
) {
    let senders = peer.get_senders().await;
    for sender in senders {
        let sender_track = sender.track().clone();
        let Ok(params) = sender.get_parameters().await else {
            continue;
        };
        let Some(codec) = params.rtp_parameters.codecs.first() else {
            continue;
        };
        let payload_type = codec.payload_type;

        let sender_track_id = sender_track.track_id().await;
        if sender_track_id == video_track.track_id().await {
            debug!("whipsynth video payload type negotiated: {}", payload_type);
            packetizer.set_video_payload_type(payload_type);
        } else if let Some(audio) = audio_track
            && sender_track_id == audio.track_id().await
        {
            debug!("whipsynth audio payload type negotiated: {}", payload_type);
            packetizer.set_audio_payload_type(payload_type);
        }
    }
}

async fn run_write_loop(
    mut frame_rx: tokio::sync::mpsc::Receiver<MediaFrame>,
    mut packetizer: Packetizer,
    video_track: Arc<TrackLocalStaticRTP>,
    audio_track: Option<Arc<TrackLocalStaticRTP>>,
    stats: Arc<Mutex<SessionStats>>,
    _connected_at: std::time::Instant,
    ct: CancellationToken,
) {
    let mut first_video = true;
    let mut first_audio = true;

    loop {
        tokio::select! {
            frame = frame_rx.recv() => {
                match frame {
                    Some(MediaFrame::Video(encoded)) => {
                        match packetizer.packetize_video(&encoded) {
                            Ok(packets) => {
                                for packet in packets {
                                    if first_video {
                                        info!("First video RTP packet written to WebRTC sender");
                                        first_video = false;
                                    }
                                    let payload_len = packet.payload.len();
                                    if let Err(e) = video_track.write_rtp(packet).await {
                                        if let Ok(mut s) = stats.lock() {
                                            s.failed_writes += 1;
                                            if s.failed_writes == 1 {
                                                warn!("Failed to write video RTP: {}", e);
                                            } else {
                                                debug!("Failed to write video RTP: {}", e);
                                            }
                                        }
                                    } else if let Ok(mut s) = stats.lock() {
                                        s.packets_sent += 1;
                                        s.bytes_sent += (12 + payload_len) as u64;
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Failed to packetize video frame: {}", e);
                            }
                        }
                    }
                    Some(MediaFrame::Audio(encoded)) => {
                        if let Some(ref audio) = audio_track {
                            match packetizer.packetize_audio(&encoded) {
                                Ok(packets) => {
                                    for packet in packets {
                                        if first_audio {
                                            info!("First audio RTP packet written to WebRTC sender");
                                            first_audio = false;
                                        }
                                        let payload_len = packet.payload.len();
                                        if let Err(e) = audio.write_rtp(packet).await {
                                            if let Ok(mut s) = stats.lock() {
                                                s.failed_writes += 1;
                                                if s.failed_writes == 1 {
                                                    warn!("Failed to write audio RTP: {}", e);
                                                } else {
                                                    debug!("Failed to write audio RTP: {}", e);
                                                }
                                            }
                                        } else if let Ok(mut s) = stats.lock() {
                                            s.packets_sent += 1;
                                            s.bytes_sent += (12 + payload_len) as u64;
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to packetize audio frame: {}", e);
                                }
                            }
                        }
                    }
                    None => {
                        debug!("Frame source ended, exiting publisher write loop");
                        break;
                    }
                }
            }
            _ = ct.cancelled() => {
                debug!("Publisher write loop cancelled");
                break;
            }
        }
    }
}

async fn create_peer(
    config: &PublisherConfig,
    gather_complete: Arc<Notify>,
    ct: CancellationToken,
) -> Result<(
    Arc<dyn PeerConnection>,
    Packetizer,
    watch::Receiver<RTCPeerConnectionState>,
    Arc<PublisherDiagnostics>,
)> {
    let mut m = MediaEngine::default();

    // Compute H265 sprop parameters up front so we can use them both for the
    // SDP offer (via the MediaEngine) and for the track codec metadata.
    let h265_sprop = (config.video_codec == VideoCodec::H265)
        .then(|| extract_h265_sprop(config.width, config.height, config.fps))
        .flatten();

    // Register a custom H265 codec with sprop parameters before the defaults so
    // the generated SDP offer includes them. Browsers such as Safari need the
    // sprop-VPS/SPS/PPS in the offer to initialize an H265 WebRTC receiver.
    if let Some(ref sprop) = h265_sprop {
        m.register_codec(
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_HEVC.to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: sprop.clone(),
                    rtcp_feedback: vec![
                        RTCPFeedback {
                            typ: "goog-remb".to_owned(),
                            parameter: "".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "ccm".to_owned(),
                            parameter: "fir".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "nack".to_owned(),
                            parameter: "".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "nack".to_owned(),
                            parameter: "pli".to_owned(),
                        },
                    ],
                },
                payload_type: 126,
            },
            RtpCodecKind::Video,
        )?;
    }

    // Register a custom AV1 codec with a resolution-derived level-idx before the
    // defaults so the generated SDP offer accurately describes the SVT-AV1
    // stream. Without this the offer only advertises `profile-id=0` and the
    // receiver infers level-idx=5 (720p30); higher-resolution streams may then
    // be dropped once they exceed that level.
    if config.video_codec == VideoCodec::Av1 {
        let level_idx =
            crate::whipsynth::packetizer::av1_level_idx(config.width, config.height, config.fps);
        m.register_codec(
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_AV1.to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: format!("profile-id=0;level-idx={level_idx};tier=0"),
                    rtcp_feedback: vec![
                        RTCPFeedback {
                            typ: "goog-remb".to_owned(),
                            parameter: "".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "ccm".to_owned(),
                            parameter: "fir".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "nack".to_owned(),
                            parameter: "".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "nack".to_owned(),
                            parameter: "pli".to_owned(),
                        },
                    ],
                },
                payload_type: 41,
            },
            RtpCodecKind::Video,
        )?;
    }

    m.register_default_codecs()?;

    let registry = Registry::new();
    let registry = configure_nack(registry, &mut m);
    let registry = configure_rtcp_reports(registry);
    configure_simulcast_extension_headers(&mut m)?;
    let registry = configure_twcc(registry, &mut m)?;
    info!("WHIP publisher configured with NACK, RTCP reports, and TWCC");

    let mut packetizer_config = PacketizerConfig::new(config.video_codec, config.audio_codec);
    packetizer_config.width = config.width;
    packetizer_config.height = config.height;
    packetizer_config.fps = config.fps;
    packetizer_config.h265_sprop = h265_sprop;
    let packetizer = Packetizer::new(&packetizer_config)?;

    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
    let diagnostics = Arc::new(PublisherDiagnostics::default());
    let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(PublisherHandler {
        ct,
        gather_complete,
        state_tx,
        diagnostics: diagnostics.clone(),
    });

    let ice_config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer {
            urls: vec![config.stun_server.clone()],
            username: "".to_string(),
            credential: "".to_string(),
        }])
        .build();

    let peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_udp_addrs(utils::webrtc::ice_udp_addrs())
            .with_configuration(ice_config)
            .build()
            .await
            .map_err(|error| anyhow!("{:?}: {}", error, error))?,
    );

    Ok((peer, packetizer, state_rx, diagnostics))
}

#[derive(Default, Clone, Copy)]
struct RtcpFeedbackCounters {
    nack_count: u64,
    pli_count: u64,
}

async fn collect_rtcp_feedback(peer: &Arc<dyn PeerConnection>) -> RtcpFeedbackCounters {
    let report = peer
        .get_stats(std::time::Instant::now(), StatsSelector::None)
        .await;
    let mut counters = RtcpFeedbackCounters::default();
    for outbound in report.outbound_rtp_streams() {
        counters.nack_count += outbound.nack_count as u64;
        counters.pli_count += outbound.pli_count as u64;
    }
    counters
}

async fn wait_for_peer_connected(
    peer: Arc<dyn PeerConnection>,
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<PublisherDiagnostics>,
) -> Result<()> {
    let result = tokio::time::timeout(WAIT_FOR_PEER_CONNECTED_TIMEOUT, async {
        loop {
            let state = *state_rx.borrow_and_update();
            match state {
                RTCPeerConnectionState::Connected => return Ok(()),
                RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected => {
                    return Err(anyhow!(
                        "WHIP publisher peer connection ended before becoming connected: state={state}"
                    ));
                }
                _ => {}
            }

            state_rx
                .changed()
                .await
                .map_err(|_| anyhow!("WHIP publisher peer connection state channel closed"))?;
        }
    })
    .await;

    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let ice_stats = crate::whip::format_ice_stats(peer).await;
            Err(anyhow!(
                "{error}, {}, ice_stats=[{}]",
                diagnostics.format(),
                ice_stats
            ))
        }
        Err(_) => {
            let ice_stats = crate::whip::format_ice_stats(peer).await;
            Err(anyhow!(
                "WHIP publisher peer connection timed out waiting for connected after {:?}: {}, ice_stats=[{}]",
                WAIT_FOR_PEER_CONNECTED_TIMEOUT,
                diagnostics.format(),
                ice_stats
            ))
        }
    }
}

async fn wait_for_unexpected_peer_end(
    peer: Arc<dyn PeerConnection>,
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<PublisherDiagnostics>,
) -> Result<()> {
    let mut saw_connected = *state_rx.borrow() == RTCPeerConnectionState::Connected;

    loop {
        let state = *state_rx.borrow();
        if state == RTCPeerConnectionState::Connected {
            saw_connected = true;
        }

        if matches!(
            state,
            RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
        ) {
            let ice_stats = crate::whip::format_ice_stats(peer).await;
            return Err(anyhow!(
                "WHIP publisher peer connection ended before shutdown: state={state}, connected_before={saw_connected}, {}, ice_stats=[{}]",
                diagnostics.format(),
                ice_stats
            ));
        }

        state_rx
            .changed()
            .await
            .map_err(|_| anyhow!("WHIP publisher peer connection state channel closed"))?;
    }
}

#[derive(Default)]
pub struct PublisherDiagnostics {
    connection_states: Mutex<Vec<String>>,
    ice_connection_states: Mutex<Vec<String>>,
    ice_gathering_states: Mutex<Vec<String>>,
    signaling_states: Mutex<Vec<String>>,
}

impl PublisherDiagnostics {
    pub fn format(&self) -> String {
        format!(
            "connection_states=[{}], ice_connection_states=[{}], ice_gathering_states=[{}], signaling_states=[{}]",
            join_states(&self.connection_states),
            join_states(&self.ice_connection_states),
            join_states(&self.ice_gathering_states),
            join_states(&self.signaling_states),
        )
    }
}

fn join_states(states: &Mutex<Vec<String>>) -> String {
    states
        .lock()
        .map(|s| s.join(" -> "))
        .unwrap_or_else(|poisoned| format!("{}(poisoned)", poisoned.into_inner().join(" -> ")))
}

fn push_state(states: &Mutex<Vec<String>>, state: impl std::fmt::Display) {
    match states.lock() {
        Ok(mut states) => states.push(state.to_string()),
        Err(poisoned) => {
            // Recover the inner data after a panic so diagnostics are still
            // available for error reporting.
            let mut states = poisoned.into_inner();
            states.push(state.to_string());
        }
    }
}

struct PublisherHandler {
    ct: CancellationToken,
    gather_complete: Arc<Notify>,
    state_tx: watch::Sender<RTCPeerConnectionState>,
    diagnostics: Arc<PublisherDiagnostics>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PublisherHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("WHIP publisher connection state changed: {}", state);
        push_state(&self.diagnostics.connection_states, state);
        let _ = self.state_tx.send(state);
        if matches!(
            state,
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
        ) {
            self.ct.cancel();
        }
    }

    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        info!("WHIP publisher ICE connection state changed: {}", state);
        push_state(&self.diagnostics.ice_connection_states, state);
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        info!("WHIP publisher ICE gathering state changed: {}", state);
        push_state(&self.diagnostics.ice_gathering_states, state);
        if state == RTCIceGatheringState::Complete {
            info!("WHIP publisher ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }

    async fn on_signaling_state_change(&self, state: RTCSignalingState) {
        info!("WHIP publisher signaling state changed: {}", state);
        push_state(&self.diagnostics.signaling_states, state);
    }
}
