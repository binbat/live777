use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters};
use rtc::shared::marshal::{Marshal, MarshalSize};

use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::RtspConfig;
use crate::forward::rtcp::RtcpMessage;
use crate::forward::track::PublishTrackRemote;
use crate::stream::manager::Manager;
use rtc_rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc_rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;

const DEFAULT_STREAM: &str = "rtsp";

pub async fn start_rtsp_server(
    manager: Arc<Manager>,
    config: RtspConfig,
    cancel: CancellationToken,
) {
    info!("Starting RTSP server on {}", config.listen);
    let listen_addr = format_bind_addr(config.listen);
    let handler = RtspHandler { manager };

    tokio::spawn(async move {
        if let Err(e) = rtsp::setup_rtsp_server_with_handler(
            &listen_addr,
            rtsp::SessionMode::Mixed,
            true,
            handler,
            cancel,
        )
        .await
        {
            error!("RTSP server error: {}", e);
        }
    });
}

struct RtspHandler {
    manager: Arc<Manager>,
}

#[async_trait::async_trait]
impl rtsp::server::SessionHandler for RtspHandler {
    async fn on_announce(&self, path: String, sdp: Vec<u8>) -> Result<()> {
        let stream_id = stream_id_from_path(path);
        let media_info = rtsp::parse_media_info_from_sdp(&sdp)?;

        // Recreate the stream so a new publisher always starts from a clean state.
        let _ = self.manager.stream_delete(stream_id.clone()).await;
        self.manager.stream_create(stream_id.clone()).await?;
        let forward = self.manager.get_or_create_forward(&stream_id).await;

        if let Some(video) = &media_info.video_codec {
            let codec = video_codec_to_rtc(video);
            if let Err(e) = forward.add_virtual_track(RtpCodecKind::Video, codec).await {
                warn!(
                    "Failed to add virtual video track for {}: {:?}",
                    stream_id, e
                );
            }
        }
        if let Some(audio) = &media_info.audio_codec {
            let codec = audio_codec_to_rtc(audio);
            if let Err(e) = forward.add_virtual_track(RtpCodecKind::Audio, codec).await {
                warn!(
                    "Failed to add virtual audio track for {}: {:?}",
                    stream_id, e
                );
            }
        }

        info!("RTSP push accepted for stream {}", stream_id);
        Ok(())
    }

    async fn on_describe(&self, path: String) -> Result<Vec<u8>> {
        let stream_id = stream_id_from_path(path);
        let forward = wait_for_forward(&self.manager, &stream_id)
            .await
            .map_err(|e| anyhow!("Stream {} not available: {}", stream_id, e))?;
        let tracks = wait_for_tracks(&forward)
            .await
            .map_err(|e| anyhow!("Stream {} has no tracks: {}", stream_id, e))?;
        Ok(build_sdp_from_tracks(&tracks)?.into_bytes())
    }

    async fn on_session(
        &self,
        path: String,
        mode: rtsp::SessionMode,
        media_info: rtsp::MediaInfo,
        endpoint: rtsp::SessionEndpoint,
    ) -> Result<()> {
        let stream_id = stream_id_from_path(path);

        match mode {
            rtsp::SessionMode::Push => {
                let mut rx = match endpoint {
                    rtsp::SessionEndpoint::Push(rx) => rx,
                    _ => return Err(anyhow!("Expected push endpoint")),
                };

                let manager = self.manager.clone();
                tokio::spawn(async move {
                    while let Some((channel, data)) = rx.recv().await {
                        if channel % 2 != 0 {
                            // RTCP: not handled yet.
                            continue;
                        }
                        let forward = manager.get_or_create_forward(&stream_id).await;
                        if forward.inject_video_rtp(&data).await.is_ok() {
                            continue;
                        }
                        let _ = forward.inject_audio_rtp(&data).await;
                    }
                    info!("RTSP push forward stopped for {}", stream_id);
                    if let Err(e) = manager.stream_delete(stream_id.clone()).await {
                        debug!(
                            "Failed to delete stream {} on push disconnect: {}",
                            stream_id, e
                        );
                    }
                });
            }
            rtsp::SessionMode::Pull => {
                let tx = match endpoint {
                    rtsp::SessionEndpoint::Pull(tx) => tx,
                    _ => return Err(anyhow!("Expected pull endpoint")),
                };

                let manager = self.manager.clone();
                tokio::spawn(async move {
                    let forward = match wait_for_forward(&manager, &stream_id).await {
                        Ok(f) => f,
                        Err(e) => {
                            error!("RTSP pull forward failed for {}: {}", stream_id, e);
                            return;
                        }
                    };
                    let tracks = match wait_for_tracks(&forward).await {
                        Ok(t) => t,
                        Err(e) => {
                            error!("RTSP pull no tracks for {}: {}", stream_id, e);
                            return;
                        }
                    };

                    // Use the negotiated TCP interleaved channels when available;
                    // fall back to the fixed UDP channel map (video=0/1, audio=2/3).
                    let channel_for = |kind| match kind {
                        RtpCodecKind::Video => media_info
                            .video_transport
                            .as_ref()
                            .and_then(|t| t.tcp_channels())
                            .unwrap_or((0, 1)),
                        RtpCodecKind::Audio => media_info
                            .audio_transport
                            .as_ref()
                            .and_then(|t| t.tcp_channels())
                            .unwrap_or((2, 3)),
                        _ => (0, 1),
                    };

                    for track in tracks {
                        let (rtp_channel, rtcp_channel) = channel_for(track.kind());

                        // RTP forward.
                        let tx_clone = tx.clone();
                        let mut packet_rx = track.subscribe();
                        tokio::spawn(async move {
                            while let Ok(packet) = packet_rx.recv().await {
                                let mut buf = vec![0u8; packet.marshal_size()];
                                if Marshal::marshal_to(&*packet, &mut buf).is_err() {
                                    continue;
                                }
                                if tx_clone.send((rtp_channel, buf)).is_err() {
                                    break;
                                }
                            }
                        });

                        // RTCP sender reports.
                        let tx_clone = tx.clone();
                        let track = track.clone();
                        tokio::spawn(async move {
                            let mut interval = tokio::time::interval(Duration::from_secs(5));
                            loop {
                                interval.tick().await;
                                if let Some(packet) = track.generate_sender_report() {
                                    match packet.marshal() {
                                        Ok(buf) => {
                                            if tx_clone.send((rtcp_channel, buf.to_vec())).is_err()
                                            {
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            debug!("Failed to marshal sender report: {}", e);
                                        }
                                    }
                                }
                            }
                        });
                    }
                });
            }
            rtsp::SessionMode::Mixed => unreachable!("session mode must be resolved"),
        }

        Ok(())
    }

    async fn on_rtcp(&self, path: String, data: Vec<u8>) -> Result<()> {
        let stream_id = stream_id_from_path(path);
        let packets = match rtc_rtcp::packet::unmarshal(&mut data.as_slice()) {
            Ok(packets) => packets,
            Err(e) => {
                debug!("Failed to parse RTCP for stream {}: {}", stream_id, e);
                return Ok(());
            }
        };

        let Some(forward) = self.manager.get_forward(&stream_id).await else {
            return Ok(());
        };

        for packet in packets {
            let any = packet.as_any();
            let (is_pli, media_ssrc) =
                if let Some(pli) = any.downcast_ref::<PictureLossIndication>() {
                    (true, pli.media_ssrc)
                } else if let Some(fir) = any.downcast_ref::<FullIntraRequest>() {
                    (false, fir.media_ssrc)
                } else {
                    continue;
                };

            let tracks = forward.internal.publish_tracks.read().await;
            let mut matches = false;
            for t in tracks.iter() {
                if t.kind() == RtpCodecKind::Video && t.source_ssrc().await == media_ssrc {
                    matches = true;
                    break;
                }
            }
            drop(tracks);

            if matches {
                let msg = if is_pli {
                    RtcpMessage::PictureLossIndication
                } else {
                    RtcpMessage::_FullIntraRequest
                };
                let _ = forward
                    .internal
                    .publish_rtcp_channel
                    .send((msg, media_ssrc));
                debug!(
                    "Forwarded RTCP keyframe request {:?} for stream {} ssrc {}",
                    msg, stream_id, media_ssrc
                );
            }
        }

        Ok(())
    }
}

fn stream_id_from_path(path: String) -> String {
    if path.is_empty() {
        DEFAULT_STREAM.to_string()
    } else {
        path
    }
}

async fn wait_for_forward(
    manager: &Manager,
    stream_id: &str,
) -> Result<crate::forward::PeerForward> {
    for _ in 0..300 {
        if let Some(forward) = manager.get_forward(stream_id).await {
            return Ok(forward);
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    Err(anyhow!("Timeout waiting for forward {}", stream_id))
}

async fn wait_for_tracks(forward: &crate::forward::PeerForward) -> Result<Vec<PublishTrackRemote>> {
    for _ in 0..300 {
        {
            let tracks = forward.internal.publish_tracks.read().await;
            if !tracks.is_empty() {
                return Ok(tracks.clone());
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    Err(anyhow!("Timeout waiting for publish tracks"))
}

fn build_sdp_from_tracks(tracks: &[PublishTrackRemote]) -> Result<String> {
    let mut lines = vec![
        "v=0".to_string(),
        "o=- 0 0 IN IP4 127.0.0.1".to_string(),
        "s=liveion".to_string(),
        "t=0 0".to_string(),
    ];

    for track in tracks {
        let codec = track.codec();
        let (media, pt, clock_rate, channels) = match codec.codec.as_str() {
            "h264" | "h265" | "hevc" | "vp8" | "vp9" | "av1" => {
                ("video", codec.payload_type, codec.clock_rate, None)
            }
            "opus" | "g722" | "pcma" | "pcmu" => (
                "audio",
                codec.payload_type,
                codec.clock_rate,
                Some(codec.channels as u8),
            ),
            _ => continue,
        };

        lines.push(format!("m={} 0 RTP/AVP {}", media, pt));
        if let Some(ch) = channels {
            lines.push(format!(
                "a=rtpmap:{} {}/{}/{}",
                pt,
                codec.codec.to_uppercase(),
                clock_rate,
                ch
            ));
        } else {
            lines.push(format!(
                "a=rtpmap:{} {}/{}",
                pt,
                codec.codec.to_uppercase(),
                clock_rate
            ));
        }
        if !codec.fmtp.is_empty() {
            lines.push(format!("a=fmtp:{} {}", pt, codec.fmtp));
        }
        let control_id = match track.kind() {
            RtpCodecKind::Video => "video",
            RtpCodecKind::Audio => "audio",
            _ => continue,
        };
        lines.push(format!("a=control:{control_id}"));
    }

    Ok(lines.join("\r\n") + "\r\n")
}

fn format_bind_addr(addr: SocketAddr) -> String {
    match addr {
        SocketAddr::V4(v4) => format!("{}:{}", v4.ip(), v4.port()),
        SocketAddr::V6(v6) => format!("[{}]:{}", v6.ip(), v6.port()),
    }
}

fn video_codec_to_rtc(codec: &rtsp::VideoCodecParams) -> RTCRtpCodecParameters {
    use rtsp::VideoCodecParams;

    match codec {
        VideoCodecParams::H264 {
            payload_type,
            clock_rate,
            sps,
            pps,
            ..
        } => {
            let mut sdp_fmtp_line =
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                    .to_string();
            if !sps.is_empty() && !pps.is_empty() {
                let sps_b64 = BASE64.encode(sps);
                let pps_b64 = BASE64.encode(pps);
                sdp_fmtp_line.push_str(&format!(";sprop-parameter-sets={},{}", sps_b64, pps_b64));
            }
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: "video/H264".to_string(),
                    clock_rate: *clock_rate,
                    channels: 0,
                    sdp_fmtp_line,
                    rtcp_feedback: vec![
                        RTCPFeedback {
                            typ: "goog-remb".to_owned(),
                            parameter: "".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "transport-cc".to_owned(),
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
                payload_type: *payload_type,
            }
        }
        VideoCodecParams::H265 {
            payload_type,
            clock_rate,
            vps,
            sps,
            pps,
            ..
        } => {
            let mut parts = Vec::new();
            if !vps.is_empty() {
                parts.push(format!("sprop-vps={}", BASE64.encode(vps)));
            }
            if !sps.is_empty() {
                parts.push(format!("sprop-sps={}", BASE64.encode(sps)));
            }
            if !pps.is_empty() {
                parts.push(format!("sprop-pps={}", BASE64.encode(pps)));
            }
            let sdp_fmtp_line = if parts.is_empty() {
                String::new()
            } else {
                parts.join(";")
            };
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: "video/H265".to_string(),
                    clock_rate: *clock_rate,
                    channels: 0,
                    sdp_fmtp_line,
                    rtcp_feedback: vec![
                        RTCPFeedback {
                            typ: "goog-remb".to_owned(),
                            parameter: "".to_owned(),
                        },
                        RTCPFeedback {
                            typ: "transport-cc".to_owned(),
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
                payload_type: *payload_type,
            }
        }
        VideoCodecParams::VP8 {
            payload_type,
            clock_rate,
        } => RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/VP8".to_string(),
                clock_rate: *clock_rate,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "goog-remb".to_owned(),
                        parameter: "".to_owned(),
                    },
                    RTCPFeedback {
                        typ: "transport-cc".to_owned(),
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
            payload_type: *payload_type,
        },
        VideoCodecParams::VP9 {
            payload_type,
            clock_rate,
        } => RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/VP9".to_string(),
                clock_rate: *clock_rate,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_string(),
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "goog-remb".to_owned(),
                        parameter: "".to_owned(),
                    },
                    RTCPFeedback {
                        typ: "transport-cc".to_owned(),
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
            payload_type: *payload_type,
        },
        VideoCodecParams::AV1 {
            payload_type,
            clock_rate,
        } => RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/AV1".to_string(),
                clock_rate: *clock_rate,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_string(),
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "goog-remb".to_owned(),
                        parameter: "".to_owned(),
                    },
                    RTCPFeedback {
                        typ: "transport-cc".to_owned(),
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
            payload_type: *payload_type,
        },
    }
}

fn audio_codec_to_rtc(codec: &rtsp::AudioCodecParams) -> RTCRtpCodecParameters {
    let mime_type = format!("audio/{}", codec.codec.to_uppercase());

    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type,
            clock_rate: codec.clock_rate,
            channels: codec.channels,
            sdp_fmtp_line: if codec.codec.to_lowercase() == "opus" {
                "minptime=10;useinbandfec=1".to_string()
            } else {
                String::new()
            },
            rtcp_feedback: vec![],
        },
        payload_type: codec.payload_type,
    }
}
