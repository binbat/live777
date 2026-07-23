use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use libwish::Client;
use rtc::peer_connection::configuration::media_engine::{MIME_TYPE_AV1, MIME_TYPE_HEVC};
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters};
use rtc::statistics::StatsSelector;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use webrtc::media_stream::Track;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::peer_connection::PeerConnection;

use crate::source::{AudioCodec, MediaFrame, VideoCodec, extract_h265_sprop};
use crate::utils;
use crate::utils::shutdown::graceful_shutdown;
use crate::whip::core::{self, PublishPeerOptions};
use crate::whipsynth::SessionStats;
use crate::whipsynth::packetizer::{Packetizer, PacketizerConfig, VIDEO_RTCP_FEEDBACK};
use crate::whipsynth::source::{frame_generator_config, spawn_rsmpeg_source};

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

/// Outcome of a [`Publisher::run`] call.
#[derive(Debug)]
pub enum PublishOutcome {
    /// The peer connected and published until stopped; carries the final
    /// session statistics.
    Completed(SessionStats),
    /// Cancelled before the peer reached `Connected`; the WHIP resource was
    /// cleaned up and nothing was published.
    Cancelled,
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

    #[cfg(test)]
    pub(crate) fn config_for_test(&self) -> PublisherConfig {
        self.config.clone()
    }

    /// Run the publisher until cancelled or the configured duration expires.
    ///
    /// Returns [`PublishOutcome::Completed`] with the final session statistics
    /// when the peer connected, or [`PublishOutcome::Cancelled`] when the
    /// token fired before the peer reached `Connected`.
    pub async fn run(self, ct: CancellationToken) -> Result<PublishOutcome> {
        let input_id = format!("whipsynth-{}", rand::random::<u64>());

        let mut client = Client::new(
            self.config.whip_url.clone(),
            Client::get_auth_header_map(self.config.token.clone())?,
        );

        // Compute codec-specific SDP parameters up front so they can be used
        // both for the offer (extra MediaEngine codec registrations) and for
        // the packetizer's track metadata.
        let extra_video_codecs = extra_video_codecs(&self.config);

        let gather_complete = Arc::new(Notify::new());
        let publish = core::create_publish_peer(
            gather_complete.clone(),
            PublishPeerOptions {
                stun_server: Some(self.config.stun_server.clone()),
                extra_video_codecs,
            },
        )
        .await?;
        let peer = publish.peer;
        let state_rx = publish.state_rx;
        let diagnostics = publish.diagnostics;

        let h265_sprop = h265_sprop(&self.config);
        let mut packetizer_config =
            PacketizerConfig::new(self.config.video_codec, self.config.audio_codec);
        packetizer_config.width = self.config.width;
        packetizer_config.height = self.config.height;
        packetizer_config.fps = self.config.fps;
        packetizer_config.h265_sprop = h265_sprop;
        let mut packetizer = Packetizer::new(&packetizer_config)?;

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
        // Race the connect phase (WHIP POST/answer + ICE gather + waiting for
        // `Connected`) against cancellation so a session stopped while
        // connecting can still clean up and report back instead of hanging
        // until the wait times out.
        let connect = async {
            utils::webrtc::setup_connection(peer.clone(), &mut client, gather_complete).await?;
            let local_summary = peer
                .local_description()
                .await
                .map(|description| utils::webrtc::summarize_sdp(&description.sdp))
                .unwrap_or_else(|| "<no local description>".to_string());
            let remote_summary = peer
                .remote_description()
                .await
                .map(|description| utils::webrtc::summarize_sdp(&description.sdp))
                .unwrap_or_else(|| "<no remote description>".to_string());
            info!("Local SDP offer summary:\n{}", local_summary);
            info!("Remote SDP answer summary:\n{}", remote_summary);
            diagnostics.set_sdp_summaries(local_summary, remote_summary);

            core::wait_for_peer_connected(peer.clone(), state_rx.clone(), diagnostics.clone()).await
        };

        tokio::select! {
            result = connect => {
                if let Err(e) = result {
                    // Connect failed (e.g. ICE connect timeout) after the WHIP
                    // POST may already have created a server-side session:
                    // clean up like the cancel path (best-effort WHIP resource
                    // DELETE, then peer close) so the session does not leak.
                    graceful_shutdown("WHIP publisher", &mut client, peer).await;
                    return Err(e);
                }
            }
            _ = ct.cancelled() => {
                // Cancelled before connecting: clean up like the normal
                // shutdown path (best-effort WHIP resource DELETE when the
                // resource URL is already known, then peer close) so the
                // server-side session does not leak until ICE timeout.
                info!("WHIP publisher cancelled before connecting, cleaning up");
                graceful_shutdown("WHIP publisher", &mut client, peer).await;
                return Ok(PublishOutcome::Cancelled);
            }
        }
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
            result = core::wait_for_unexpected_peer_end(peer.clone(), state_rx, diagnostics.clone()) => {
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
        graceful_shutdown("WHIP publisher", &mut client, peer).await;

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
        result
            .and(source_result)
            .map(|_| PublishOutcome::Completed(final_stats))
    }
}

/// H265 sprop parameters for the configured stream, when applicable.
fn h265_sprop(config: &PublisherConfig) -> Option<String> {
    (config.video_codec == VideoCodec::H265)
        .then(|| extract_h265_sprop(config.width, config.height, config.fps))
        .flatten()
}

/// Extra video codec registrations for the MediaEngine, applied before the
/// default codecs so the generated SDP offer prefers them.
///
/// - H265: browsers such as Safari need the sprop-VPS/SPS/PPS in the offer to
///   initialize an H265 WebRTC receiver.
/// - AV1: without a resolution-derived level-idx the offer only advertises
///   `profile-id=0` and the receiver infers level-idx=5 (720p30);
///   higher-resolution streams may then be dropped once they exceed that level.
fn extra_video_codecs(config: &PublisherConfig) -> Vec<RTCRtpCodecParameters> {
    let mut codecs = Vec::new();

    if let Some(sprop) = h265_sprop(config) {
        codecs.push(RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_HEVC.to_owned(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line: sprop,
                rtcp_feedback: VIDEO_RTCP_FEEDBACK.clone(),
            },
            payload_type: 126,
        });
    }

    if config.video_codec == VideoCodec::Av1 {
        let level_idx =
            crate::whipsynth::packetizer::av1_level_idx(config.width, config.height, config.fps);
        codecs.push(RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_AV1.to_owned(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line: format!("profile-id=0;level-idx={level_idx};tier=0"),
                rtcp_feedback: VIDEO_RTCP_FEEDBACK.clone(),
            },
            payload_type: 41,
        });
    }

    codecs
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
                                write_packets(&video_track, packets, &stats, &mut first_video, "video").await;
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
                                    write_packets(audio, packets, &stats, &mut first_audio, "audio").await;
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

/// Write a batch of RTP packets to a track, updating session stats.
///
/// Shared by the video and audio write paths. `first` is flipped to `false`
/// after the first packet so the "first packet" log fires once per kind.
async fn write_packets(
    track: &Arc<TrackLocalStaticRTP>,
    packets: Vec<rtc::rtp::packet::Packet>,
    stats: &Arc<Mutex<SessionStats>>,
    first: &mut bool,
    kind: &str,
) {
    for packet in packets {
        if *first {
            info!("First {kind} RTP packet written to WebRTC sender");
            *first = false;
        }
        let payload_len = packet.payload.len();
        if let Err(e) = track.write_rtp(packet).await {
            if let Ok(mut s) = stats.lock() {
                s.failed_writes += 1;
                if s.failed_writes == 1 {
                    warn!("Failed to write {kind} RTP: {}", e);
                } else {
                    debug!("Failed to write {kind} RTP: {}", e);
                }
            }
        } else if let Ok(mut s) = stats.lock() {
            s.packets_sent += 1;
            s.bytes_sent += (12 + payload_len) as u64;
        }
    }
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
