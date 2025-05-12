use std::net::{IpAddr, Ipv4Addr};

use anyhow::{anyhow, Result};
use cli::create_child;
use portpicker::pick_unused_port;
use scopeguard::defer;
use sdp::{description::media::RangedPort, SessionDescription};
use std::{
    fs::File,
    io::{Cursor, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::{debug, error, info, trace, warn};
use url::Url;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::{
    peer_connection::RTCPeerConnection,
    rtp_transceiver::{
        rtp_codec::RTPCodecType, rtp_transceiver_direction::RTCRtpTransceiverDirection,
        RTCRtpTransceiverInit,
    },
    util::MarshalSize,
};

use libwish::Client;

use crate::rtspclient::setup_rtsp_push_session;
use crate::utils;
use crate::{SCHEME_RTP_SDP, SCHEME_RTSP_CLIENT, SCHEME_RTSP_SERVER};

pub async fn from(
    target_url: String,
    whep_url: String,
    token: Option<String>,
    command: Option<String>,
) -> Result<()> {
    let input = Url::parse(&target_url).unwrap_or(
        Url::parse(&format!(
            "{}://{}:0/{}",
            SCHEME_RTP_SDP,
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            target_url
        ))
        .unwrap(),
    );
    info!("[WHEP] Processing output URL: {}", target_url);

    let (mut target_host, listen_host) = utils::parse_host(&input);
    info!(
        "[WHEP] Target host: {}, Listen host: {}",
        target_host, listen_host
    );

    let (complete_tx, mut complete_rx) = unbounded_channel();
    let mut media_info = rtsp::MediaInfo::default();
    let (video_send, video_recv) = unbounded_channel::<Vec<u8>>();
    let (audio_send, audio_recv) = unbounded_channel::<Vec<u8>>();
    let codec_info = Arc::new(tokio::sync::Mutex::new(rtsp::CodecInfo::new()));
    info!("[WHEP] Channels and codec info initialized");

    let mut client = Client::new(whep_url.clone(), Client::get_auth_header_map(token.clone()));
    info!("[WHEP] WHEP client created");

    let (peer, answer) = webrtc_start(
        &mut client,
        video_send,
        audio_send,
        complete_tx.clone(),
        codec_info.clone(),
    )
    .await?;
    info!("[WHEP] WebRTC connection established");

    tokio::time::sleep(Duration::from_secs(1)).await;
    let codec_info = codec_info.lock().await;
    debug!("[WHEP] Codec info: {:?}", codec_info);

    let filtered_sdp = match rtsp::filter_sdp(
        &answer.sdp,
        codec_info.video_codec.as_ref(),
        codec_info.audio_codec.as_ref(),
    ) {
        Ok(sdp) => sdp,
        Err(e) => {
            error!("[WHEP] Failed to filter SDP: {}", e);
            return Err(anyhow!("Failed to filter SDP: {}", e));
        }
    };
    info!("[WHEP] SDP filtered successfully");

    let child = Arc::new(tokio::sync::Mutex::new(None));

    if input.scheme() == SCHEME_RTSP_SERVER {
        let (tx, mut rx) = unbounded_channel::<rtsp::MediaInfo>();
        let mut handler = rtsp::Handler::new(tx, complete_tx.clone());
        handler.set_sdp(filtered_sdp.clone().into_bytes());

        let host2 = listen_host.to_string();
        let tcp_port = input.port().unwrap_or(0);
        let rtsp_child = child.clone();
        let command_clone = command.clone();
        let complete_tx_clone = complete_tx.clone();

        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("{}:{}", host2.clone(), tcp_port))
                .await
                .unwrap();
            warn!(
                "=== RTSP listener started : {} ===",
                listener.local_addr().unwrap()
            );

            if let Some(cmd) = command_clone {
                match create_child(Some(cmd)) {
                    Ok(child_proc) => {
                        let mut lock = rtsp_child.lock().await;
                        *lock = child_proc;
                        info!("[WHEP] Child process created for RTSP server");

                        let rtsp_child_monitor = rtsp_child.clone();
                        tokio::spawn(async move {
                            loop {
                                let exit_status = {
                                    let lock = rtsp_child_monitor.lock().await;
                                    if let Some(ref child_mutex) = *lock {
                                        if let Ok(mut child) = child_mutex.lock() {
                                            match child.try_wait() {
                                                Ok(Some(status)) => Some(status),
                                                Ok(None) => None,
                                                Err(_) => Some(std::process::ExitStatus::default()),
                                            }
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                };

                                if exit_status.is_some() {
                                    let _ = complete_tx_clone.send(());
                                    break;
                                }

                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        });
                    }
                    Err(e) => {
                        error!(
                            "[WHEP] Failed to create child process for RTSP server: {}",
                            e
                        );
                    }
                }
            }

            loop {
                let (socket, _) = listener.accept().await.unwrap();
                match rtsp::process_socket(socket, &mut handler).await {
                    Ok(_) => {}
                    Err(e) => error!("=== RTSP listener error: {} ===", e),
                };
                warn!("=== RTSP client socket closed ===");
            }
        });

        media_info = rx.recv().await.unwrap();
    } else if input.scheme() == SCHEME_RTSP_CLIENT {
        media_info =
            setup_rtsp_push_session(&target_url, filtered_sdp.clone(), &target_host).await?;
    } else {
        media_info.video_rtp_client = pick_unused_port();
        media_info.audio_rtp_client = pick_unused_port();

        let mut reader = Cursor::new(filtered_sdp.as_bytes());
        let mut session = SessionDescription::unmarshal(&mut reader).unwrap();
        target_host = session
            .clone()
            .connection_information
            .and_then(|conn_info| conn_info.address)
            .map(|address| address.to_string())
            .unwrap_or(Ipv4Addr::LOCALHOST.to_string());
        for media in &mut session.media_descriptions {
            if media.media_name.media == "video" {
                if let Some(port) = media_info.video_rtp_client {
                    media.media_name.port = RangedPort {
                        value: port as isize,
                        range: None,
                    };
                }
            } else if media.media_name.media == "audio" {
                if let Some(port) = media_info.audio_rtp_client {
                    media.media_name.port = RangedPort {
                        value: port as isize,
                        range: None,
                    };
                }
            }
        }
        let sdp = session.marshal();

        let file_path = Path::new(&target_url);
        debug!("SDP written to {:?}", file_path);
        let mut file = File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(file_path)?;
        file.write_all(sdp.as_bytes())?;

        if let Some(cmd) = command.clone() {
            match create_child(Some(cmd)) {
                Ok(child_proc) => {
                    let mut lock = child.lock().await;
                    *lock = child_proc;
                    info!("[WHEP] Child process created for RTP SDP");
                }
                Err(e) => {
                    error!("[WHEP] Failed to create child process for RTP SDP: {}", e);
                }
            }
        }
    }

    info!("media info : {:?}", media_info);
    tokio::spawn(utils::rtp_send(
        video_recv,
        listen_host.clone(),
        target_host.clone(),
        media_info.video_rtp_client,
        media_info.video_rtp_server,
    ));
    info!("[WHEP] Video RTP sender started");

    tokio::spawn(utils::rtp_send(
        audio_recv,
        listen_host.clone(),
        target_host.clone(),
        media_info.audio_rtp_client,
        media_info.audio_rtp_server,
    ));
    info!("[WHEP] Audio RTP sender started");

    defer!({
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let lock = child.lock().await;
                if let Some(ref child_mutex) = *lock {
                    if let Ok(mut child_proc) = child_mutex.lock() {
                        let _ = child_proc.kill();
                        info!("[WHEP] Child process killed during cleanup");
                    }
                }
            });
        });
    });

    let wait_child = child.clone();
    let complete_tx_monitor = complete_tx.clone();
    tokio::spawn(async move {
        loop {
            let exit_status = {
                let lock = wait_child.lock().await;
                if let Some(ref child_mutex) = *lock {
                    if let Ok(mut child) = child_mutex.lock() {
                        match child.try_wait() {
                            Ok(Some(status)) => Some(status),
                            Ok(None) => None,
                            Err(_) => Some(std::process::ExitStatus::default()),
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(status) = exit_status {
                info!("[WHEP] Child process exited with status: {}", status);
                let _ = complete_tx_monitor.send(());
                break;
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    if input.scheme() == SCHEME_RTSP_SERVER {
        info!("[WHEP] Starting RTCP listeners for RTSP server mode");
        tokio::spawn(utils::rtcp_listener(
            target_host.clone(),
            media_info.video_rtp_server,
            peer.clone(),
        ));
        tokio::spawn(utils::rtcp_listener(
            target_host.clone(),
            media_info.audio_rtp_server,
            peer.clone(),
        ));
    }

    tokio::select! {
        _ = complete_rx.recv() => { }
        msg = signal::wait_for_stop_signal() => warn!("Received signal: {}", msg)
    }

    let _ = client.remove_resource().await;
    let _ = peer.close().await;

    Ok(())
}
async fn webrtc_start(
    client: &mut Client,
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    complete_tx: UnboundedSender<()>,
    codec_info: Arc<tokio::sync::Mutex<rtsp::CodecInfo>>,
) -> Result<(Arc<RTCPeerConnection>, RTCSessionDescription)> {
    let peer = new_peer(
        video_send,
        audio_send,
        complete_tx.clone(),
        codec_info.clone(),
    )
    .await?;

    utils::setup_webrtc_connection(peer.clone(), client).await?;

    let answer = peer
        .remote_description()
        .await
        .ok_or_else(|| anyhow!("No remote description"))?;

    Ok((peer, answer))
}

async fn new_peer(
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    complete_tx: UnboundedSender<()>,
    codec_info: Arc<tokio::sync::Mutex<rtsp::CodecInfo>>,
) -> Result<Arc<RTCPeerConnection>> {
    let (api, config) = utils::create_webrtc_api().await?;

    let peer = Arc::new(
        api.build()
            .new_peer_connection(config)
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

    peer.add_transceiver_from_kind(
        RTPCodecType::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    peer.add_transceiver_from_kind(
        RTPCodecType::Audio,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    utils::setup_peer_connection_handlers(peer.clone(), complete_tx).await;

    peer.on_track(Box::new({
        let codec_info = codec_info.clone();
        move |track, _, _| {
            let video_sender = video_send.clone();
            let audio_sender = audio_send.clone();
            let codec = track.codec().clone();
            let track_kind = track.kind();

            let codec_info = codec_info.clone();
            tokio::spawn(async move {
                let mut codec_info = codec_info.lock().await;
                if track_kind == RTPCodecType::Video {
                    debug!("Updating video codec info: {:?}", codec);
                    codec_info.video_codec = Some(codec.clone());
                } else if track_kind == RTPCodecType::Audio {
                    debug!("Updating audio codec info: {:?}", codec);
                    codec_info.audio_codec = Some(codec.clone());
                }
            });

            let sender = match track_kind {
                RTPCodecType::Video => Some(video_sender),
                RTPCodecType::Audio => Some(audio_sender),
                _ => None,
            };

            if let Some(sender) = sender {
                tokio::spawn(async move {
                    let mut b = [0u8; 1500];
                    while let Ok((rtp_packet, _)) = track.read(&mut b).await {
                        trace!("Received RTP packet: {:?}", rtp_packet);
                        let size = rtp_packet.marshal_size();
                        let data = b[0..size].to_vec();
                        let _ = sender.send(data);
                    }
                });
            }
            Box::pin(async {})
        }
    }));

    Ok(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn create_test_sdp() -> String {
        r#"v=0

o=- 1234567890 1234567890 IN IP4 127.0.0.1
s=-
t=0 0
a=group:BUNDLE 0 1
m=video 9 UDP/TLS/RTP/SAVPF 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
a=rtcp-fb:96 nack
a=rtcp-fb:96 nack pli
a=rtcp-fb:96 goog-remb
a=mid:0
a=sendonly
m=audio 9 UDP/TLS/RTP/SAVPF 111
c=IN IP4 127.0.0.1
a=rtpmap:111 opus/48000/2
a=mid:1
a=sendonly"#
            .to_string()
    }

    #[tokio::test]
    async fn test_new_peer() {
        let (video_send, _) = unbounded_channel::<Vec<u8>>();
        let (audio_send, _) = unbounded_channel::<Vec<u8>>();
        let (complete_tx, _) = unbounded_channel();
        let codec_info = Arc::new(tokio::sync::Mutex::new(rtsp::CodecInfo::new()));

        let peer = new_peer(video_send, audio_send, complete_tx, codec_info.clone()).await;

        assert!(peer.is_ok(), "Failed to create peer connection");
        let peer = peer.unwrap();

        let transceivers = peer.get_transceivers().await;
        assert_eq!(transceivers.len(), 2, "Expected two transceivers");

        for transceiver in transceivers {
            let direction = transceiver.direction();
            assert_eq!(
                direction,
                RTCRtpTransceiverDirection::Recvonly,
                "Transceiver should be recvonly"
            );
        }
    }

    #[tokio::test]
    async fn test_sdp_creation() {
        let sdp = create_test_sdp();

        assert!(sdp.contains("m=video"), "SDP should contain video media");
        assert!(sdp.contains("m=audio"), "SDP should contain audio media");
        assert!(
            sdp.contains("rtpmap:96 VP8/90000"),
            "SDP should specify VP8 codec"
        );
        assert!(
            sdp.contains("rtpmap:111 opus/48000/2"),
            "SDP should specify Opus codec"
        );
    }

    #[test]
    fn test_sdp_filter() {
        let sdp = create_test_sdp();

        let video_codec = webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters {
            capability: webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: "video/VP8".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 96,
            ..Default::default()
        };

        let audio_codec = webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters {
            capability: webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: "audio/opus".to_string(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "".to_string(),
                rtcp_feedback: vec![],
            },
            payload_type: 111,
            ..Default::default()
        };

        let result = rtsp::filter_sdp(&sdp, Some(&video_codec), Some(&audio_codec));
        assert!(result.is_ok(), "SDP filtering should succeed");

        if let Ok(filtered_sdp) = result {
            assert!(
                filtered_sdp.contains("VP8/90000"),
                "Filtered SDP should contain video codec"
            );
            assert!(
                filtered_sdp.contains("opus/48000"),
                "Filtered SDP should contain audio codec"
            );
        }
    }
}
