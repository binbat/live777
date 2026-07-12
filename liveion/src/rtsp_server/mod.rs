use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters};
use rtc::shared::marshal::{Marshal, MarshalSize};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::RtspConfig;
use crate::stream::manager::Manager;

const PUSH_STREAM_ID: &str = "rtsp-push";
const PULL_STREAM_ID: &str = "rtsp-pull";

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

    let (media_info, channels, mut port_update_rx) =
        rtsp::setup_rtsp_server_session(&listen_addr, Vec::new(), rtsp::SessionMode::Push, true)
            .await?;

    let (tx, rx) = channels.ok_or_else(|| anyhow!("RTSP push requires TCP interleaved mode"))?;

    setup_push_stream(&manager, media_info).await?;
    spawn_push_forward(manager.clone(), rx, tx);

    tokio::spawn(async move {
        while let Some(port_update) = port_update_rx.recv().await {
            info!(
                "RTSP push port update connection #{}",
                port_update.connection_id
            );
            // New push connections reuse the existing virtual tracks; the
            // internal server already re-broadcasts RTP to the latest client.
        }
    });

    cancel.cancelled().await;
    info!("RTSP push server shutting down");
    Ok(())
}

async fn setup_push_stream(manager: &Manager, media_info: rtsp::MediaInfo) -> Result<()> {
    let _ = manager.stream_create(PUSH_STREAM_ID.to_string()).await;
    let forward = manager.get_or_create_forward(PUSH_STREAM_ID).await;

    if let Some(video) = &media_info.video_codec {
        let codec = video_codec_to_rtc(video);
        if let Err(e) = forward.add_virtual_track(RtpCodecKind::Video, codec).await {
            warn!("Failed to add virtual video track: {:?}", e);
        }
    }
    if let Some(audio) = &media_info.audio_codec {
        let codec = audio_codec_to_rtc(audio);
        if let Err(e) = forward.add_virtual_track(RtpCodecKind::Audio, codec).await {
            warn!("Failed to add virtual audio track: {:?}", e);
        }
    }

    Ok(())
}

fn spawn_push_forward(
    manager: Arc<Manager>,
    mut rx: UnboundedReceiver<rtsp::InterleavedData>,
    _tx: tokio::sync::mpsc::UnboundedSender<rtsp::InterleavedData>,
) {
    tokio::spawn(async move {
        while let Some((channel, data)) = rx.recv().await {
            let forward = manager.get_or_create_forward(PUSH_STREAM_ID).await;
            // Channel numbers are not known until the first SETUP handshake,
            // so dispatch based on payload inspection for the first packets and
            // then cache the mapping.
            if channel % 2 == 0 {
                // Even channels carry RTP in interleaved mode.
                if forward.inject_video_rtp(&data).await.is_ok() {
                    continue;
                }
                let _ = forward.inject_audio_rtp(&data).await;
            }
            // Odd channels carry RTCP; ignored for now.
        }
        info!("RTSP push forward stopped");
    });
}

async fn run_pull_server(
    manager: Arc<Manager>,
    listen: SocketAddr,
    cancel: CancellationToken,
) -> Result<()> {
    info!("Starting RTSP pull server on {}", listen);
    let listen_addr = format_bind_addr(listen);

    // Wait for the pull source stream to exist and have tracks.
    let forward = wait_for_forward(&manager, PULL_STREAM_ID).await?;
    let tracks = wait_for_tracks(&forward).await?;
    let sdp = build_sdp_from_tracks(&tracks)?;

    let (_media_info, channels, mut port_update_rx) = rtsp::setup_rtsp_server_session(
        &listen_addr,
        sdp.into_bytes(),
        rtsp::SessionMode::Pull,
        true,
    )
    .await?;

    let (tx, mut rx) =
        channels.ok_or_else(|| anyhow!("RTSP pull requires TCP interleaved mode"))?;

    spawn_pull_forward(tracks, tx);

    tokio::spawn(async move {
        // Drain RTCP channel to keep connection alive.
        while rx.recv().await.is_some() {}
    });

    tokio::spawn(async move {
        while let Some(port_update) = port_update_rx.recv().await {
            info!(
                "RTSP pull port update connection #{}",
                port_update.connection_id
            );
        }
    });

    cancel.cancelled().await;
    info!("RTSP pull server shutting down");
    Ok(())
}

fn spawn_pull_forward(
    tracks: Vec<crate::forward::track::PublishTrackRemote>,
    tx: tokio::sync::mpsc::UnboundedSender<rtsp::InterleavedData>,
) {
    // Assign fixed interleaved channels: video RTP=0, video RTCP=1, audio RTP=2, audio RTCP=3.
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

async fn wait_for_tracks(
    forward: &crate::forward::PeerForward,
) -> Result<Vec<crate::forward::track::PublishTrackRemote>> {
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

fn build_sdp_from_tracks(tracks: &[crate::forward::track::PublishTrackRemote]) -> Result<String> {
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
