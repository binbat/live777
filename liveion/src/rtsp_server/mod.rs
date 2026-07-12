use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters};
use rtc::shared::marshal::{Marshal, MarshalSize};

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::RtspConfig;
use crate::forward::track::PublishTrackRemote;
use crate::stream::manager::Manager;

const DEFAULT_PUSH_STREAM: &str = "rtsp-push";
const DEFAULT_PULL_STREAM: &str = "rtsp-pull";

pub async fn start_rtsp_server(
    manager: Arc<Manager>,
    config: RtspConfig,
    cancel: CancellationToken,
) {
    let push_manager = manager.clone();
    let pull_manager = manager.clone();
    let push_cancel = cancel.clone();
    let pull_cancel = cancel;

    tokio::spawn(async move {
        if let Err(e) = run_push_server(push_manager, config.push_listen, push_cancel).await {
            error!("RTSP push server error: {}", e);
        }
    });

    tokio::spawn(async move {
        if let Err(e) = run_pull_server(pull_manager, config.pull_listen, pull_cancel).await {
            error!("RTSP pull server error: {}", e);
        }
    });
}

async fn run_push_server(
    manager: Arc<Manager>,
    listen: SocketAddr,
    cancel: CancellationToken,
) -> Result<()> {
    info!("Starting RTSP push server on {}", listen);
    let listen_addr = format_bind_addr(listen);
    let handler = PushHandler { manager };
    rtsp::setup_rtsp_server_with_handler(
        &listen_addr,
        rtsp::SessionMode::Push,
        true,
        handler,
        cancel,
    )
    .await
}

async fn run_pull_server(
    manager: Arc<Manager>,
    listen: SocketAddr,
    cancel: CancellationToken,
) -> Result<()> {
    info!("Starting RTSP pull server on {}", listen);
    let listen_addr = format_bind_addr(listen);
    let handler = PullHandler { manager };
    rtsp::setup_rtsp_server_with_handler(
        &listen_addr,
        rtsp::SessionMode::Pull,
        true,
        handler,
        cancel,
    )
    .await
}

struct PushHandler {
    manager: Arc<Manager>,
}

#[async_trait::async_trait]
impl rtsp::server::SessionHandler for PushHandler {
    async fn on_announce(&self, path: String, sdp: Vec<u8>) -> Result<()> {
        let stream_id = stream_id_from_path(path, DEFAULT_PUSH_STREAM);
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

    async fn on_describe(&self, _path: String) -> Result<Vec<u8>> {
        Err(anyhow!("DESCRIBE is not supported on the push server"))
    }

    async fn on_session(
        &self,
        path: String,
        _mode: rtsp::SessionMode,
        _media_info: rtsp::MediaInfo,
        endpoint: rtsp::SessionEndpoint,
    ) -> Result<()> {
        let stream_id = stream_id_from_path(path, DEFAULT_PUSH_STREAM);
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
        });

        Ok(())
    }
}

struct PullHandler {
    manager: Arc<Manager>,
}

#[async_trait::async_trait]
impl rtsp::server::SessionHandler for PullHandler {
    async fn on_announce(&self, _path: String, _sdp: Vec<u8>) -> Result<()> {
        Err(anyhow!("ANNOUNCE is not supported on the pull server"))
    }

    async fn on_describe(&self, path: String) -> Result<Vec<u8>> {
        let stream_id = stream_id_from_path(path, DEFAULT_PULL_STREAM);
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
        _mode: rtsp::SessionMode,
        _media_info: rtsp::MediaInfo,
        endpoint: rtsp::SessionEndpoint,
    ) -> Result<()> {
        let stream_id = stream_id_from_path(path, DEFAULT_PULL_STREAM);
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

            // Assign fixed interleaved channels: video RTP=0, video RTCP=1,
            // audio RTP=2, audio RTCP=3.
            for track in tracks {
                let channel = match track.kind() {
                    RtpCodecKind::Video => 0u8,
                    RtpCodecKind::Audio => 2u8,
                    _ => continue,
                };
                let tx_clone = tx.clone();
                let mut packet_rx = track.subscribe();
                tokio::spawn(async move {
                    while let Ok(packet) = packet_rx.recv().await {
                        let mut buf = vec![0u8; packet.marshal_size()];
                        if Marshal::marshal_to(&*packet, &mut buf).is_err() {
                            continue;
                        }
                        if tx_clone.send((channel, buf)).is_err() {
                            break;
                        }
                    }
                });
            }
        });

        Ok(())
    }
}

fn stream_id_from_path(path: String, default: &str) -> String {
    if path.is_empty() {
        default.to_string()
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
            "h264" => ("video", 96, 90000, None),
            "h265" | "hevc" => ("video", 97, 90000, None),
            "vp8" => ("video", 98, 90000, None),
            "vp9" => ("video", 99, 90000, None),
            "av1" => ("video", 100, 90000, None),
            "opus" => ("audio", 111, 48000, Some(2)),
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
        if media == "video" {
            lines.push(format!("a=fmtp:{} packetization-mode=1", pt));
        }
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
            ..
        } => RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H264".to_string(),
                clock_rate: *clock_rate,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                        .to_string(),
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
        VideoCodecParams::H265 {
            payload_type,
            clock_rate,
            ..
        } => RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H265".to_string(),
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
