use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::{sync::Arc, time::Duration, vec};

use anyhow::{anyhow, Result};
use scopeguard::defer;
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use tracing::{debug, error, info, trace, warn};
use url::Url;
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
    ice_transport::{ice_credential_type::RTCIceCredentialType, ice_server::RTCIceServer},
    interceptor::registry::Registry,
    peer_connection::{configuration::RTCConfiguration, RTCPeerConnection},
    rtp::packet::Packet,
    rtp_transceiver::{
        rtp_codec::RTCRtpCodecCapability, rtp_codec::RTPCodecType, rtp_sender::RTCRtpSender,
    },
    track::track_local::{track_local_static_rtp::TrackLocalStaticRTP, TrackLocalWriter},
    util::Unmarshal,
};

use cli::{codec_from_str, create_child};
use libwish::Client;

use crate::payload;
use crate::rtspclient::setup_rtsp_session;
use crate::utils::{self, rtcp_listener};

use crate::{PREFIX_LIB, SCHEME_RTP_SDP, SCHEME_RTSP_CLIENT, SCHEME_RTSP_SERVER};

pub async fn into(
    target_url: String,
    whip_url: String,
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
    info!("[WHIP] Processing input URL: {}", input);

    let (target_host, mut listen_host) = utils::parse_host(&input);

    info!(
        "[WHIP] Target host: {}, Listen host: {}",
        target_host, listen_host
    );

    let video_port = input.port().unwrap_or(0);
    debug!("[WHIP] Parsed port from input URL: {}", video_port);
    let media_info;

    let (complete_tx, mut complete_rx) = unbounded_channel();
    let mut client = Client::new(whip_url.clone(), Client::get_auth_header_map(token.clone()));
    info!("[WHIP] WHIP client created");

    let child = if let Some(command) = &command {
        info!("[WHIP] Creating child process with command: {}", command);
        Arc::new(create_child(Some(command.to_string()))?)
    } else {
        Default::default()
    };
    defer!({
        if let Some(child) = child.as_ref() {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }
    });

    if input.scheme() == SCHEME_RTSP_SERVER {
        info!("[WHIP] Starting RTSP server mode");
        let (tx, mut rx) = unbounded_channel::<rtsp::MediaInfo>();
        let mut handler = rtsp::Handler::new(tx, complete_tx.clone());

        let host2 = listen_host.to_string();
        debug!("[WHIP] Binding RTSP server to {}:{}", host2, video_port);
        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("{}:{}", host2.clone(), video_port))
                .await
                .unwrap();
            info!(
                "[WHIP] RTSP server started: {}",
                listener.local_addr().unwrap()
            );
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                match rtsp::process_socket(socket, &mut handler).await {
                    Ok(_) => debug!("[WHIP] RTSP client connection processed successfully"),
                    Err(e) => error!("[WHIP] RTSP client connection processing failed: {}", e),
                };
                warn!("[WHIP] RTSP client connection closed");
            }
        });

        media_info = rx.recv().await.unwrap();
    } else if input.scheme() == SCHEME_RTSP_CLIENT {
        info!("[WHIP] Starting RTSP client mode");
        media_info = setup_rtsp_session(&target_url).await?;
    } else {
        info!("[WHIP] Processing RTP mode");
        tokio::time::sleep(Duration::from_secs(1)).await;
        let path = Path::new(&target_url);
        let sdp = sdp_types::Session::parse(&fs::read(path).unwrap()).unwrap();
        if let Some(connection_info) = &sdp.connection {
            listen_host.clone_from(&connection_info.connection_address);
        }
        info!("[WHIP] SDP file parsed successfully");
        let video_track = sdp.medias.iter().find(|md| md.media == "video");
        let audio_track = sdp.medias.iter().find(|md| md.media == "audio");

        let codec_vid = if let Some(video_track) = video_track {
            video_track
                .attributes
                .iter()
                .find_map(|attr| {
                    if attr.attribute == "rtpmap" {
                        let parts: Vec<&str> = attr.value.as_ref()?.split_whitespace().collect();
                        if parts.len() > 1 {
                            Some(parts[1].split('/').next().unwrap_or("").to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        };

        let codec_aud = if let Some(audio_track) = audio_track {
            audio_track
                .attributes
                .iter()
                .find_map(|attr| {
                    if attr.attribute == "rtpmap" {
                        let parts: Vec<&str> = attr.value.as_ref()?.split_whitespace().collect();
                        if parts.len() > 1 {
                            Some(parts[1].split('/').next().unwrap_or("").to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        };

        media_info = rtsp::MediaInfo {
            video_rtp_client: None,
            audio_rtp_client: None,
            video_codec: if codec_vid != "unknown" {
                Some(codec_from_str(&codec_vid)?)
            } else {
                None
            },
            audio_codec: if codec_aud != "unknown" {
                Some(codec_from_str(&codec_aud)?)
            } else {
                None
            },
            video_rtp_server: video_track.map(|track| track.port),
            audio_rtp_server: audio_track.map(|track| track.port),
            video_rtcp_client: None,
            audio_rtcp_client: None,
        };
    }
    info!("[WHIP] Media info: {:?}", media_info);
    let mut video_listener = None;
    if let Some(video_port) = media_info.video_rtp_server {
        info!(
            "[WHIP] Creating video RTP listener: {}:{}",
            listen_host, video_port
        );
        video_listener = Some(UdpSocket::bind(format!("{}:{}", listen_host, video_port)).await?);
    }
    let mut audio_listener = None;
    if let Some(audio_port) = media_info.audio_rtp_server {
        info!(
            "[WHIP] Creating audio RTP listener: {}:{}",
            listen_host, audio_port
        );
        audio_listener = Some(UdpSocket::bind(format!("{}:{}", listen_host, audio_port)).await?);
    }

    let (peer, video_sender, audio_sender) = webrtc_start(
        &mut client,
        media_info.video_codec.map(|c| c.into()),
        media_info.audio_codec.map(|c| c.into()),
        complete_tx.clone(),
        target_url,
    )
    .await
    .map_err(|error| anyhow!(format!("[{}] {}", PREFIX_LIB, error)))?;

    if let Some(video_listener) = video_listener {
        debug!(
            "=== video rtp listener started : {} ===",
            video_listener.local_addr().unwrap()
        );
        tokio::spawn(rtp_listener(video_listener, video_sender));
    }
    if let Some(audio_listener) = audio_listener {
        debug!(
            "=== audio rtp listener started : {} ===",
            audio_listener.local_addr().unwrap()
        );
        tokio::spawn(rtp_listener(audio_listener, audio_sender));
    }

    if let Some(port) = media_info.video_rtp_server {
        tokio::spawn(rtcp_listener(
            listen_host.clone(),
            Some(port + 1),
            peer.clone(),
        ));
    }

    let senders = peer.get_senders().await;
    if let Some(video_rtcp_port) = media_info.video_rtcp_client {
        debug!(
            "Video RTCP client port: {}, listen host: {}, target_host: {}",
            video_rtcp_port, listen_host, target_host
        );
        for sender in &senders {
            if let Some(track) = sender.track().await {
                if track.kind() == RTPCodecType::Video {
                    tokio::spawn(read_rtcp(
                        sender.clone(),
                        listen_host.clone(),
                        target_host.clone(),
                        video_rtcp_port,
                    ));
                }
            }
        }
    }

    if let Some(audio_rtcp_port) = media_info.audio_rtcp_client {
        debug!(
            "Audio RTCP client port: {}, listen host: {}, target_host: {}",
            audio_rtcp_port, listen_host, target_host
        );
        for sender in &senders {
            if let Some(track) = sender.track().await {
                if track.kind() == RTPCodecType::Audio {
                    tokio::spawn(read_rtcp(
                        sender.clone(),
                        listen_host.clone(),
                        target_host.clone(),
                        audio_rtcp_port,
                    ));
                }
            }
        }
    }

    let wait_child = child.clone();
    match wait_child.as_ref() {
        Some(child) => loop {
            if let Ok(mut child) = child.lock() {
                if let Ok(wait) = child.try_wait() {
                    if wait.is_some() {
                        let _ = complete_tx.send(());
                        return Ok(());
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        },
        None => info!("No child process"),
    }

    tokio::select! {
        _ = complete_rx.recv() => {}
        msg = signal::wait_for_stop_signal() => {warn!("Received signal: {}", msg)}
    }
    warn!("RTP listener closed");
    let _ = client.remove_resource().await;
    let _ = peer.close().await;
    Ok(())
}

async fn rtp_listener(socker: UdpSocket, sender: Option<UnboundedSender<Vec<u8>>>) {
    if let Some(sender) = sender {
        let mut inbound_rtp_packet = vec![0u8; 1600];
        while let Ok((n, addr)) = socker.recv_from(&mut inbound_rtp_packet).await {
            let data = inbound_rtp_packet[..n].to_vec();
            trace!("Received RTP packet from {} ({} bytes)", addr, n);
            let _ = sender.send(data);
        }
    }
}

async fn read_rtcp(
    sender: Arc<RTCRtpSender>,
    host: String,
    bind_host: String,
    port: u16,
) -> Result<()> {
    let udp_socket = UdpSocket::bind(format!("{}:0", host)).await?;
    info!(
        "UDP socket for RTCP bound to: {}",
        udp_socket.local_addr().unwrap()
    );

    loop {
        match sender.read_rtcp().await {
            Ok((packets, _attributes)) => {
                for packet in packets {
                    debug!("Received RTCP packet from remote peer: {:?}", packet);

                    let mut buf = vec![];
                    if let Ok(serialized_packet) = packet.marshal() {
                        buf.extend_from_slice(&serialized_packet);
                    }
                    if !buf.is_empty() {
                        if let Err(err) = udp_socket
                            .send_to(&buf, format!("{}:{}", bind_host, port))
                            .await
                        {
                            warn!("Failed to forward RTCP packet: {}", err);
                        } else {
                            trace!("Forwarded RTCP packet to {}:{}", bind_host, port);
                        }
                    }
                }
            }
            Err(err) => {
                warn!("Error reading RTCP packet from remote peer: {}", err);
                break Ok(());
            }
        }
    }
}

async fn webrtc_start(
    client: &mut Client,
    video_codec: Option<RTCRtpCodecCapability>,
    audio_codec: Option<RTCRtpCodecCapability>,
    complete_tx: UnboundedSender<()>,
    input: String,
) -> Result<(
    Arc<RTCPeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
)> {
    let (peer, video_sender, audio_sender) =
        new_peer(video_codec, audio_codec, complete_tx.clone(), input).await?;

    utils::setup_webrtc_connection(peer.clone(), client).await?;

    utils::setup_peer_connection_handlers(peer.clone(), complete_tx).await;

    Ok((peer, video_sender, audio_sender))
}
async fn new_peer(
    video_codec: Option<RTCRtpCodecCapability>,
    audio_codec: Option<RTCRtpCodecCapability>,
    complete_tx: UnboundedSender<()>,
    input: String,
) -> Result<(
    Arc<RTCPeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
)> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();
    let config = RTCConfiguration {
        ice_servers: vec![{
            RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                username: "".to_string(),
                credential: "".to_string(),
                credential_type: RTCIceCredentialType::Unspecified,
            }
        }],
        ..Default::default()
    };

    let peer = Arc::new(
        api.new_peer_connection(config)
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );
    utils::setup_peer_connection_handlers(peer.clone(), complete_tx).await;

    let video_tx = if let Some(video_codec) = video_codec {
        let video_track_id = format!("{}-video", input);
        let video_track = Arc::new(TrackLocalStaticRTP::new(
            video_codec.clone(),
            video_track_id.to_owned(),
            input.to_owned(),
        ));
        let _ = peer
            .add_track(video_track.clone())
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

        let (video_tx, mut video_rx) = unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            debug!("Video codec: {}", video_codec.mime_type);
            let mut handler: Box<dyn payload::RePayload + Send> =
                match video_codec.mime_type.as_str() {
                    MIME_TYPE_VP8 => Box::new(payload::RePayloadCodec::new(video_codec.mime_type)),
                    MIME_TYPE_VP9 => Box::new(payload::RePayloadCodec::new(video_codec.mime_type)),
                    MIME_TYPE_H264 => Box::new(payload::RePayloadCodec::new(video_codec.mime_type)),
                    _ => Box::new(payload::Forward::new()),
                };

            while let Some(data) = video_rx.recv().await {
                if let Ok(packet) = Packet::unmarshal(&mut data.as_slice()) {
                    trace!("Received video packet: {}", packet);
                    for packet in handler.payload(packet) {
                        trace!("Sending video packet: {}", packet);
                        let _ = video_track.write_rtp(&packet).await;
                    }
                }
            }
        });
        Some(video_tx)
    } else {
        None
    };

    let audio_tx = if let Some(audio_codec) = audio_codec {
        let audio_track_id = format!("{}-audio", input);
        let audio_track = Arc::new(TrackLocalStaticRTP::new(
            audio_codec.clone(),
            audio_track_id.to_owned(),
            input.to_owned(),
        ));
        let _ = peer
            .add_track(audio_track.clone())
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

        let (audio_tx, mut audio_rx) = unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            debug!("Audio codec: {}", audio_codec.mime_type);
            let mut handler: Box<dyn payload::RePayload + Send> =
                match audio_codec.mime_type.as_str() {
                    MIME_TYPE_OPUS => {
                        Box::new(payload::RePayloadCodec::new(audio_codec.mime_type.clone()))
                    }
                    _ => Box::new(payload::Forward::new()),
                };

            while let Some(data) = audio_rx.recv().await {
                if audio_codec.mime_type == MIME_TYPE_G722 {
                    let _ = audio_track.write(&data).await;
                } else if let Ok(packet) = Packet::unmarshal(&mut data.as_slice()) {
                    trace!("Received audio packet: {}", packet);
                    for packet in handler.payload(packet) {
                        trace!("Sending audio packet: {}", packet);
                        let _ = audio_track.write_rtp(&packet).await;
                    }
                }
            }
        });
        Some(audio_tx)
    } else {
        None
    };

    Ok((peer, video_tx, audio_tx))
}
