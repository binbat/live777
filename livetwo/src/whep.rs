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

    if input.scheme() == SCHEME_RTSP_SERVER {
        let (tx, mut rx) = unbounded_channel::<rtsp::MediaInfo>();
        let mut handler = rtsp::Handler::new(tx, complete_tx.clone());
        handler.set_sdp(filtered_sdp.clone().into_bytes());

        let host2 = listen_host.to_string();
        let tcp_port = input.port().unwrap_or(0);
        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("{}:{}", host2.clone(), tcp_port))
                .await
                .unwrap();
            warn!(
                "=== RTSP listener started : {} ===",
                listener.local_addr().unwrap()
            );
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

    let child = Arc::new(create_child(command)?);
    info!("[WHEP] Child process created");
    defer!({
        if let Some(child) = child.as_ref() {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }
    });

    let wait_child = child.clone();
    tokio::spawn(async move {
        match wait_child.as_ref() {
            Some(child) => loop {
                if let Ok(mut child) = child.lock() {
                    if let Ok(wait) = child.try_wait() {
                        if wait.is_some() {
                            let _ = complete_tx.send(());
                            return;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            },
            None => info!("No child process"),
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
    // let mut m = MediaEngine::default();

    // m.register_default_codecs()?;

    // let mut registry = Registry::new();
    // registry = register_default_interceptors(registry, &mut m)?;
    // let api = APIBuilder::new()
    //     .with_media_engine(m)
    //     .with_interceptor_registry(registry)
    //     .build();

    // let config = RTCConfiguration {
    //     ice_servers: vec![RTCIceServer {
    //         urls: vec!["stun:stun.l.google.com:19302".to_string()],
    //         username: "".to_string(),
    //         credential: "".to_string(),
    //         credential_type: RTCIceCredentialType::Unspecified,
    //     }],
    //     ..Default::default()
    // };
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
