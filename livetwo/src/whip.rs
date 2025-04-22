use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::{sync::Arc, time::Duration, vec};

use anyhow::{anyhow, Result};
use scopeguard::defer;
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use tracing::{debug, error, info, trace, warn};
use url::{Host, Url};
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
    ice_transport::{ice_credential_type::RTCIceCredentialType, ice_server::RTCIceServer},
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        RTCPeerConnection,
    },
    rtcp,
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
    info!("=== Received Input: {} ===", input);

    let mut host = match input.host() {
        Some(Host::Domain(_)) | Some(Host::Ipv4(_)) => Ipv4Addr::UNSPECIFIED.to_string(),
        Some(Host::Ipv6(_)) => Ipv6Addr::UNSPECIFIED.to_string(),
        None => {
            eprintln!("Invalid host for {}, using default.", input);
            Ipv4Addr::UNSPECIFIED.to_string()
        }
    };

    let original_host = match input.host() {
        Some(Host::Ipv4(ip)) => ip.to_string(),
        Some(Host::Ipv6(ip)) => ip.to_string(),
        Some(Host::Domain(_)) | None => Ipv4Addr::LOCALHOST.to_string(),
    };

    let video_port = input.port().unwrap_or(0);
    let media_info;

    let (complete_tx, mut complete_rx) = unbounded_channel();
    let mut client = Client::new(whip_url.clone(), Client::get_auth_header_map(token.clone()));

    let child = if let Some(command) = &command {
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
        let (tx, mut rx) = unbounded_channel::<rtsp::MediaInfo>();
        let mut handler = rtsp::Handler::new(tx, complete_tx.clone());

        let host2 = host.to_string();
        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("{}:{}", host2.clone(), video_port))
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
        media_info = setup_rtsp_session(&target_url).await?;
    } else {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let path = Path::new(&target_url);
        let sdp = sdp_types::Session::parse(&fs::read(path).unwrap()).unwrap();
        if let Some(connection_info) = &sdp.connection {
            host.clone_from(&connection_info.connection_address);
        }
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
    debug!("media info: {:?}", media_info);
    let mut video_listener = None;
    if let Some(video_port) = media_info.video_rtp_server {
        video_listener = Some(UdpSocket::bind(format!("{}:{}", host, video_port)).await?);
    }
    let mut audio_listener = None;
    if let Some(audio_port) = media_info.audio_rtp_server {
        audio_listener = Some(UdpSocket::bind(format!("{}:{}", host, audio_port)).await?);
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
        info!(
            "=== video listener started : {} ===",
            video_listener.local_addr().unwrap()
        );
        tokio::spawn(rtp_listener(video_listener, video_sender));
    }
    if let Some(audio_listener) = audio_listener {
        info!(
            "=== audio listener started : {} ===",
            audio_listener.local_addr().unwrap()
        );
        tokio::spawn(rtp_listener(audio_listener, audio_sender));
    }

    if let Some(port) = media_info.video_rtp_server {
        tokio::spawn(rtcp_listener(host.clone(), port + 1, peer.clone()));
    }

    let senders = peer.get_senders().await;
    if let Some(video_rtcp_port) = media_info.video_rtcp_client {
        for sender in &senders {
            if let Some(track) = sender.track().await {
                if track.kind() == RTPCodecType::Video {
                    tokio::spawn(read_rtcp(
                        sender.clone(),
                        host.clone(),
                        original_host.clone(),
                        video_rtcp_port,
                    ));
                }
            }
        }
    }

    if let Some(audio_rtcp_port) = media_info.audio_rtcp_client {
        for sender in &senders {
            if let Some(track) = sender.track().await {
                if track.kind() == RTPCodecType::Audio {
                    tokio::spawn(read_rtcp(
                        sender.clone(),
                        host.clone(),
                        original_host.clone(),
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
        while let Ok((n, _)) = socker.recv_from(&mut inbound_rtp_packet).await {
            let data = inbound_rtp_packet[..n].to_vec();
            let _ = sender.send(data);
        }
    }
}

async fn rtcp_listener(host: String, rtcp_port: u16, peer: Arc<RTCPeerConnection>) {
    let rtcp_listener = UdpSocket::bind(format!("{}:{}", host, rtcp_port))
        .await
        .unwrap();
    info!(
        "RTCP listener bound to: {}",
        rtcp_listener.local_addr().unwrap()
    );
    let mut rtcp_buf = vec![0u8; 1500];

    loop {
        let (len, addr) = rtcp_listener.recv_from(&mut rtcp_buf).await.unwrap();
        if len > 0 {
            debug!("Received {} bytes of RTCP data from {}", len, addr);
            let mut rtcp_data = &rtcp_buf[..len];

            if let Ok(rtcp_packets) = rtcp::packet::unmarshal(&mut rtcp_data) {
                for packet in rtcp_packets {
                    debug!("Received RTCP packet from {}: {:?}", addr, packet);
                    if let Err(err) = peer.write_rtcp(&[packet]).await {
                        warn!("Failed to send RTCP packet: {}", err);
                    }
                }
            }
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
                            debug!("Forwarded RTCP packet to {}:{}", bind_host, port);
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
    let offer = peer.create_offer(None).await?;

    let mut gather_complete = peer.gathering_complete_promise().await;
    peer.set_local_description(offer).await?;
    let _ = gather_complete.recv().await;

    let (answer, ice_servers) = client
        .wish(peer.local_description().await.unwrap().sdp)
        .await?;

    debug!("Get http header link ice servers: {:?}", ice_servers);
    let mut current_config = peer.get_configuration().await;
    current_config.ice_servers.clone_from(&ice_servers);
    peer.set_configuration(current_config.clone()).await?;

    peer.set_remote_description(answer)
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

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

    let pc = peer.clone();
    peer.on_peer_connection_state_change(Box::new(move |s| {
        let pc = pc.clone();
        let complete_tx = complete_tx.clone();
        tokio::spawn(async move {
            warn!("Connection state changed: {}", s);
            match s {
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                    let _ = pc.close().await;
                }
                RTCPeerConnectionState::Closed => {
                    let _ = complete_tx.send(());
                }
                v => debug!("{}", v),
            };
        });
        Box::pin(async {})
    }));

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
