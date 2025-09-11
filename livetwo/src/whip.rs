use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::{sync::Arc, time::Duration, vec};

use anyhow::{Result, anyhow};
use cli::{codec_from_str, create_child};
use libwish::Client;
use scopeguard::defer;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
};
use tracing::{debug, error, info, trace, warn};
use url::Url;
use webrtc::{
    api::{APIBuilder, interceptor_registry::register_default_interceptors, media_engine::*},
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{RTCPeerConnection, configuration::RTCConfiguration},
    rtp::packet::Packet,
    rtp_transceiver::{
        rtp_codec::RTCRtpCodecCapability, rtp_codec::RTPCodecType, rtp_sender::RTCRtpSender,
    },
    track::track_local::{TrackLocalWriter, track_local_static_rtp::TrackLocalStaticRTP},
    util::Unmarshal,
};

use crate::payload;
use crate::rtspclient::{RtspMode, setup_rtsp_session};
use crate::utils;

use crate::{PREFIX_LIB, SCHEME_RTSP_CLIENT, SCHEME_RTSP_SERVER};

pub async fn into(
    target_url: String,
    whip_url: String,
    token: Option<String>,
    command: Option<String>,
) -> Result<()> {
    let input = utils::parse_input_url(&target_url)?;
    info!("Processing input URL: {}", input);

    let (mut target_host, listen_host) = utils::parse_host(&input);
    info!("Target host: {}, Listen host: {}", target_host, listen_host);

    let video_port = input.port().unwrap_or(0);
    debug!("Parsed port from input URL: {}", video_port);

    let (complete_tx, mut complete_rx) = unbounded_channel();
    let mut client = Client::new(whip_url.clone(), Client::get_auth_header_map(token.clone()));
    debug!("WHIP client created");

    let child = if let Some(command) = &command {
        info!("Creating child process with command: {}", command);
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

    let (media_info, host, tx, rx) = match input.scheme() {
        SCHEME_RTSP_SERVER => {
            let (media_info, tx, rx) =
                rtsp_server_mode(video_port, &listen_host, complete_tx.clone()).await?;
            (media_info, target_host, tx, rx)
        }
        SCHEME_RTSP_CLIENT => {
            let (media_info, tx, rx) = rtsp_client_mode(&target_url, &target_host).await?;
            (media_info, target_host, tx, rx)
        }
        _ => {
            let (media_info, host) = rtp_mode(&target_url).await?;
            (media_info, host, None, None)
        }
    };
    target_host = host;
    info!("Media info: {:?}", media_info);

    let peer = setup_rtp_handlers(
        &mut client,
        &media_info,
        complete_tx.clone(),
        input,
        target_host,
        rx,
        tx,
    )
    .await?;

    wait_for_completion(
        child.clone(),
        complete_tx,
        &mut complete_rx,
        &mut client,
        peer,
    )
    .await
}

async fn rtsp_server_mode(
    video_port: u16,
    listen_host: &str,
    complete_tx: UnboundedSender<()>,
) -> Result<(
    rtsp::MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    info!("Starting RTSP server mode");
    let (tx, mut rx) = unbounded_channel::<rtsp::MediaInfo>();

    let (interleaved_tx, interleaved_rx) = unbounded_channel::<(u8, Vec<u8>)>();
    let (rtcp_interleaved_tx, rtcp_interleaved_rx) = unbounded_channel::<(u8, Vec<u8>)>();
    let mut handler = rtsp::Handler::new(tx, complete_tx);

    handler.set_interleaved_sender(interleaved_tx);
    handler.set_interleaved_receiver(rtcp_interleaved_rx);

    let host2 = listen_host.to_string();
    debug!("Binding RTSP server to {}:{}", host2, video_port);
    tokio::spawn(async move {
        let listener = TcpListener::bind(format!("{}:{}", host2.clone(), video_port))
            .await
            .unwrap();
        info!("RTSP server started: {}", listener.local_addr().unwrap());
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            match rtsp::process_socket(socket, &mut handler).await {
                Ok(_) => debug!("RTSP client connection processed successfully"),
                Err(e) => error!("RTSP client connection processing failed: {}", e),
            };
            warn!("RTSP client connection closed");
        }
    });

    let media_info = rx.recv().await.unwrap();

    let uses_tcp = media_info
        .video_transport
        .as_ref()
        .is_some_and(|t| matches!(t, rtsp::TransportInfo::Tcp { .. }))
        || media_info
            .audio_transport
            .as_ref()
            .is_some_and(|t| matches!(t, rtsp::TransportInfo::Tcp { .. }));

    if uses_tcp {
        Ok((media_info, Some(rtcp_interleaved_tx), Some(interleaved_rx)))
    } else {
        Ok((media_info, None, None))
    }
}

async fn rtsp_client_mode(
    target_url: &str,
    target_host: &str,
) -> Result<(
    rtsp::MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    info!("Starting RTSP client mode");
    setup_rtsp_session(target_url, None, target_host, RtspMode::Pull).await
}

async fn rtp_mode(target_url: &str) -> Result<(rtsp::MediaInfo, String)> {
    info!("Processing RTP mode");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let path = Path::new(target_url);
    let sdp = sdp_types::Session::parse(&fs::read(path).unwrap()).unwrap();
    let mut host = String::new();

    if let Some(connection_info) = &sdp.connection {
        let addr: IpAddr = connection_info
            .connection_address
            .parse()
            .map_err(|e| anyhow!("Invalid IP address in SDP: {}", e))?;
        host.clone_from(&addr.to_string());
    }
    info!("SDP file parsed successfully");

    let video_track = sdp.medias.iter().find(|md| md.media == "video");
    let audio_track = sdp.medias.iter().find(|md| md.media == "audio");
    let (codec_vid, codec_aud) = parse_codecs(&video_track, &audio_track);

    let media_info = rtsp::MediaInfo {
        video_transport: video_track.map(|track| rtsp::TransportInfo::Udp {
            rtp_send_port: None,
            rtp_recv_port: Some(track.port),
            rtcp_send_port: None,
            rtcp_recv_port: None,
        }),
        audio_transport: audio_track.map(|track| rtsp::TransportInfo::Udp {
            rtp_send_port: None,
            rtp_recv_port: Some(track.port),
            rtcp_send_port: None,
            rtcp_recv_port: None,
        }),
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
    };

    Ok((media_info, host))
}

fn parse_codecs(
    video_track: &Option<&sdp_types::Media>,
    audio_track: &Option<&sdp_types::Media>,
) -> (String, String) {
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

    (codec_vid, codec_aud)
}

async fn setup_rtp_listeners(
    media_info: &rtsp::MediaInfo,
    listen_host: &str,
) -> Result<(Option<UdpSocket>, Option<UdpSocket>)> {
    let mut video_listener = None;
    if let Some(rtsp::TransportInfo::Udp {
        rtp_recv_port: Some(video_port),
        ..
    }) = media_info.video_transport
    {
        info!(
            "Creating video RTP listener: {}:{}",
            listen_host, video_port
        );
        video_listener = Some(UdpSocket::bind(format!("{listen_host}:{video_port}")).await?);
    }

    let mut audio_listener = None;
    if let Some(rtsp::TransportInfo::Udp {
        rtp_recv_port: Some(audio_port),
        ..
    }) = media_info.audio_transport
    {
        info!(
            "Creating audio RTP listener: {}:{}",
            listen_host, audio_port
        );
        audio_listener = Some(UdpSocket::bind(format!("{listen_host}:{audio_port}")).await?);
    }

    Ok((video_listener, audio_listener))
}

async fn setup_webrtc(
    client: &mut Client,
    media_info: &rtsp::MediaInfo,
    complete_tx: UnboundedSender<()>,
    input: Url,
) -> Result<(
    Arc<RTCPeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
)> {
    let (peer, video_sender, audio_sender) = webrtc_start(
        client,
        media_info.video_codec.map(|c| c.into()),
        media_info.audio_codec.map(|c| c.into()),
        complete_tx,
        input.to_string(),
    )
    .await
    .map_err(|error| anyhow!(format!("[{}] {}", PREFIX_LIB, error)))?;

    Ok((peer, video_sender, audio_sender))
}

async fn setup_rtp_handlers(
    client: &mut Client,
    media_info: &rtsp::MediaInfo,
    complete_tx: UnboundedSender<()>,
    input: Url,
    host: String,
    interleaved_rx: Option<UnboundedReceiver<(u8, Vec<u8>)>>,
    interleaved_tx: Option<UnboundedSender<(u8, Vec<u8>)>>,
) -> Result<Arc<RTCPeerConnection>> {
    let listen_host = if host.parse::<Ipv6Addr>().is_ok() {
        Ipv6Addr::UNSPECIFIED.to_string()
    } else {
        Ipv4Addr::UNSPECIFIED.to_string()
    };
    let (video_listener, audio_listener) = setup_rtp_listeners(media_info, &listen_host).await?;

    let (peer, video_sender, audio_sender) =
        setup_webrtc(client, media_info, complete_tx, input).await?;

    if let Some(mut rx) = interleaved_rx {
        let video_rtp_channel = media_info.video_transport.as_ref().and_then(|t| {
            if let rtsp::TransportInfo::Tcp { rtp_channel, .. } = t {
                Some(*rtp_channel)
            } else {
                None
            }
        });

        let video_rtcp_channel = media_info.video_transport.as_ref().and_then(|t| {
            if let rtsp::TransportInfo::Tcp { rtcp_channel, .. } = t {
                Some(*rtcp_channel)
            } else {
                None
            }
        });

        let audio_rtp_channel = media_info.audio_transport.as_ref().and_then(|t| {
            if let rtsp::TransportInfo::Tcp { rtp_channel, .. } = t {
                Some(*rtp_channel)
            } else {
                None
            }
        });

        let audio_rtcp_channel = media_info.audio_transport.as_ref().and_then(|t| {
            if let rtsp::TransportInfo::Tcp { rtcp_channel, .. } = t {
                Some(*rtcp_channel)
            } else {
                None
            }
        });

        let video_sender_clone = video_sender.clone();
        let audio_sender_clone = audio_sender.clone();
        let peer_clone = peer.clone();

        tokio::spawn(async move {
            info!("Starting RTP/RTCP TCP interleaved handler");
            debug!(
                "Video channels - RTP: {:?}, RTCP: {:?}",
                video_rtp_channel, video_rtcp_channel
            );
            debug!(
                "Audio channels - RTP: {:?}, RTCP: {:?}",
                audio_rtp_channel, audio_rtcp_channel
            );

            while let Some((channel, data)) = rx.recv().await {
                trace!(
                    "Received interleaved data on channel {}, {} bytes",
                    channel,
                    data.len()
                );

                if Some(channel) == video_rtp_channel {
                    if let Some(sender) = &video_sender_clone {
                        trace!("Forwarding video RTP data to WebRTC");
                        if let Err(e) = sender.send(data) {
                            error!("Failed to forward video RTP data: {}", e);
                        }
                    }
                } else if Some(channel) == video_rtcp_channel {
                    let mut cursor = Cursor::new(data.clone());
                    if let Ok(rtcp_packets) = webrtc::rtcp::packet::unmarshal(&mut cursor) {
                        trace!("Forwarding video RTCP data to WebRTC");
                        for packet in rtcp_packets {
                            if let Err(e) = peer_clone.write_rtcp(&[packet]).await {
                                error!("Failed to write video RTCP packet: {}", e);
                            }
                        }
                    } else {
                        warn!("Failed to parse RTCP packet on channel {}", channel);
                    }
                } else if Some(channel) == audio_rtp_channel {
                    if let Some(sender) = &audio_sender_clone {
                        trace!("sending audio RTP data");
                        if let Err(e) = sender.send(data) {
                            error!("Failed to send audio RTP data: {}", e);
                        }
                    }
                } else if Some(channel) == audio_rtcp_channel {
                    let mut cursor = Cursor::new(data.clone());
                    if let Ok(rtcp_packets) = webrtc::rtcp::packet::unmarshal(&mut cursor) {
                        trace!("Forwarding audio RTCP data to WebRTC");
                        for packet in rtcp_packets {
                            if let Err(e) = peer_clone.write_rtcp(&[packet]).await {
                                error!("Failed to write audio RTCP packet: {}", e);
                            }
                        }
                    } else {
                        warn!("Failed to parse RTCP packet on channel {}", channel);
                    }
                } else {
                    warn!("Received data on unknown channel: {}", channel);
                }
            }

            warn!("TCP interleaved data handler stopped");
        });

        if let Some(rtcp_tx) = interleaved_tx {
            let senders = peer.get_senders().await;

            if let Some(rtcp_channel) = video_rtcp_channel {
                for sender in &senders {
                    let sender_clone = sender.clone();
                    if let Some(track) = sender.track().await {
                        if track.kind() == RTPCodecType::Video {
                            let rtcp_tx_clone = rtcp_tx.clone();
                            let channel = rtcp_channel;

                            tokio::spawn(async move {
                                info!(
                                    "Starting video RTCP reader for sending to RTSP client on channel {}",
                                    channel
                                );

                                loop {
                                    match sender_clone.read_rtcp().await {
                                        Ok((packets, _)) => {
                                            for packet in packets {
                                                debug!(
                                                    "Received video RTCP from WebRTC to forward to RTSP client: {:?}",
                                                    packet
                                                );

                                                if let Ok(data) = packet.marshal() {
                                                    let data_vec = data.to_vec();
                                                    if let Err(e) =
                                                        rtcp_tx_clone.send((channel, data_vec))
                                                    {
                                                        error!(
                                                            "Failed to forward WebRTC video RTCP to RTSP client: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!("Error reading video RTCP from WebRTC: {}", e);
                                            break;
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            }

            if let Some(rtcp_channel) = audio_rtcp_channel {
                for sender in &senders {
                    let sender_clone = sender.clone();
                    if let Some(track) = sender.track().await {
                        if track.kind() == RTPCodecType::Audio {
                            let rtcp_tx_clone = rtcp_tx.clone();
                            let channel = rtcp_channel;

                            tokio::spawn(async move {
                                info!(
                                    "Starting audio RTCP reader for sending to RTSP client on channel {}",
                                    channel
                                );

                                loop {
                                    match sender_clone.read_rtcp().await {
                                        Ok((packets, _)) => {
                                            for packet in packets {
                                                debug!(
                                                    "Received audio RTCP from WebRTC to forward to RTSP client: {:?}",
                                                    packet
                                                );

                                                if let Ok(data) = packet.marshal() {
                                                    let data_vec = data.to_vec();
                                                    if let Err(e) =
                                                        rtcp_tx_clone.send((channel, data_vec))
                                                    {
                                                        error!(
                                                            "Failed to forward WebRTC audio RTCP to RTSP client: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!("Error reading audio RTCP from WebRTC: {}", e);
                                            break;
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            }
        }
    }

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

    if let Some(rtsp::TransportInfo::Udp {
        rtcp_recv_port: Some(port),
        ..
    }) = &media_info.video_transport
    {
        debug!("Setting up video RTCP listener on port {}", port);
        tokio::spawn(utils::rtcp_listener(
            listen_host.to_string(),
            *port,
            peer.clone(),
        ));
    }

    if let Some(rtsp::TransportInfo::Udp {
        rtcp_recv_port: Some(port),
        ..
    }) = &media_info.audio_transport
    {
        debug!("Setting up audio RTCP listener on port {}", port);
        tokio::spawn(utils::rtcp_listener(
            listen_host.to_string(),
            *port,
            peer.clone(),
        ));
    }

    let senders = peer.get_senders().await;

    if let Some(rtsp::TransportInfo::Udp {
        rtcp_send_port,
        rtcp_recv_port,
        ..
    }) = &media_info.video_transport
    {
        if rtcp_send_port.is_some() && rtcp_recv_port.is_some() {
            if let Some(video_rtcp_port) = rtcp_recv_port {
                debug!(
                    "Setting up video RTCP reader - port: {}, listen host: {}, target_host: {}",
                    video_rtcp_port, listen_host, host
                );
                for sender in &senders {
                    if let Some(track) = sender.track().await {
                        if track.kind() == RTPCodecType::Video {
                            tokio::spawn(read_rtcp(
                                sender.clone(),
                                listen_host.to_string(),
                                host.to_string(),
                                *video_rtcp_port,
                            ));
                        }
                    }
                }
            }
        }
    }

    if let Some(rtsp::TransportInfo::Udp {
        rtcp_send_port,
        rtcp_recv_port,
        ..
    }) = &media_info.audio_transport
    {
        if rtcp_send_port.is_some() && rtcp_recv_port.is_some() {
            if let Some(audio_rtcp_port) = rtcp_recv_port {
                debug!(
                    "Setting up audio RTCP reader - port: {}, listen host: {}, target_host: {}",
                    audio_rtcp_port, listen_host, host
                );
                for sender in &senders {
                    if let Some(track) = sender.track().await {
                        if track.kind() == RTPCodecType::Audio {
                            tokio::spawn(read_rtcp(
                                sender.clone(),
                                listen_host.to_string(),
                                host.to_string(),
                                *audio_rtcp_port,
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(peer)
}

async fn wait_for_completion(
    child: Arc<Option<Mutex<std::process::Child>>>,
    complete_tx: UnboundedSender<()>,
    complete_rx: &mut UnboundedReceiver<()>,
    client: &mut Client,
    peer: Arc<RTCPeerConnection>,
) -> Result<()> {
    match child.as_ref() {
        Some(child_mutex) => loop {
            if let Ok(mut child) = child_mutex.lock() {
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

async fn rtp_listener(socket: UdpSocket, sender: Option<UnboundedSender<Vec<u8>>>) {
    if let Some(sender) = sender {
        let mut inbound_rtp_packet = vec![0u8; 1600];
        while let Ok((n, addr)) = socket.recv_from(&mut inbound_rtp_packet).await {
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
    let udp_socket = UdpSocket::bind(format!("{host}:0")).await?;
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
                            .send_to(&buf, format!("{bind_host}:{port}"))
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
        setup_video_track(peer.clone(), video_codec, input.clone()).await?
    } else {
        None
    };

    let audio_tx = if let Some(audio_codec) = audio_codec {
        setup_audio_track(peer.clone(), audio_codec, input).await?
    } else {
        None
    };

    Ok((peer, video_tx, audio_tx))
}

async fn setup_video_track(
    peer: Arc<RTCPeerConnection>,
    video_codec: RTCRtpCodecCapability,
    input: String,
) -> Result<Option<UnboundedSender<Vec<u8>>>> {
    let video_track_id = format!("{input}-video");
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
        let mut handler: Box<dyn payload::RePayload + Send> = match video_codec.mime_type.as_str() {
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

    Ok(Some(video_tx))
}

async fn setup_audio_track(
    peer: Arc<RTCPeerConnection>,
    audio_codec: RTCRtpCodecCapability,
    input: String,
) -> Result<Option<UnboundedSender<Vec<u8>>>> {
    let audio_track_id = format!("{input}-audio");
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
        let mut handler: Box<dyn payload::RePayload + Send> = match audio_codec.mime_type.as_str() {
            MIME_TYPE_OPUS => Box::new(payload::RePayloadCodec::new(audio_codec.mime_type.clone())),
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

    Ok(Some(audio_tx))
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

    #[tokio::test]
    async fn test_new_peer_with_codecs() {
        let video_codec = Some(RTCRtpCodecCapability {
            mime_type: "video/H264".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "".to_string(),
            rtcp_feedback: vec![],
        });
        let audio_codec = Some(RTCRtpCodecCapability {
            mime_type: "audio/opus".to_string(),
            clock_rate: 48000,
            channels: 2,
            sdp_fmtp_line: "".to_string(),
            rtcp_feedback: vec![],
        });
        let (complete_tx, _complete_rx) = mpsc::unbounded_channel();
        let input = "test-input".to_string();

        let result = new_peer(video_codec, audio_codec, complete_tx, input).await;
        assert!(result.is_ok());

        let (peer, video_tx, audio_tx) = result.unwrap();
        assert!(video_tx.is_some(), "Video sender should be created");
        assert!(audio_tx.is_some(), "Audio sender should be created");

        let senders = peer.get_senders().await;
        assert_eq!(senders.len(), 2, "Should have two tracks (video + audio)");
    }

    #[tokio::test]
    async fn test_new_peer_no_codecs() {
        let (complete_tx, _complete_rx) = mpsc::unbounded_channel();
        let input = "test-input".to_string();

        let result = new_peer(None, None, complete_tx, input).await;
        assert!(result.is_ok());

        let (peer, video_tx, audio_tx) = result.unwrap();
        assert!(video_tx.is_none(), "No video sender should be created");
        assert!(audio_tx.is_none(), "No audio sender should be created");

        let senders = peer.get_senders().await;
        assert_eq!(senders.len(), 0, "Should have no tracks");
    }
}
