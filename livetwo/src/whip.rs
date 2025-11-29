use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow};
use cli::{codec_from_str, create_child};
use libwish::Client;
use scopeguard::defer;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio::{
    net::UdpSocket,
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
use crate::payload::RePayload;
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
        if let Some(child) = child.as_ref()
            && let Ok(mut child) = child.lock()
        {
            let _ = child.kill();
        }
    });

    // Get media info based on input scheme
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
    debug!("Media info: {:?}", media_info);

    // Setup WebRTC peer connection
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

    // Wait for completion
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
    _complete_tx: UnboundedSender<()>,
) -> Result<(
    rtsp::MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    info!("Starting RTSP server mode (WHIP)");

    let listen_addr = format!("{}:{}", listen_host, video_port);

    // Use unified RTSP server session
    rtsp::setup_rtsp_server_session(
        &listen_addr,
        Vec::new(), // SDP will be received from client
        rtsp::SessionMode::Push,
        true, // Use TCP for now
    )
    .await
}

async fn rtsp_client_mode(
    target_url: &str,
    target_host: &str,
) -> Result<(
    rtsp::MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    info!("Starting RTSP client mode for WHIP");

    let url = url::Url::parse(target_url)?;
    let use_tcp = url
        .query_pairs()
        .find(|(key, _)| key == "transport")
        .map(|(_, value)| value.to_lowercase() == "tcp")
        .unwrap_or(false);

    info!(
        "RTSP transport mode: {}",
        if use_tcp { "TCP" } else { "UDP" }
    );

    let mut clean_url = url.clone();
    clean_url.set_query(None);
    let clean_url_str = clean_url.to_string();

    // Pull mode: pull from RTSP and push to WebRTC
    rtsp::setup_rtsp_session(
        &clean_url_str,
        None,
        target_host,
        rtsp::RtspMode::Pull,
        use_tcp,
    )
    .await
}

async fn rtp_mode(target_url: &str) -> Result<(rtsp::MediaInfo, String)> {
    info!("Processing RTP mode");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let path = Path::new(target_url);
    let sdp_bytes = fs::read(path).map_err(|e| anyhow!("Failed to read SDP file: {}", e))?;
    let sdp =
        sdp_types::Session::parse(&sdp_bytes).map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;
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
        video_transport: video_track.map(|track| {
            let port = track.port;
            rtsp::TransportInfo::Udp {
                rtp_send_port: None,
                rtp_recv_port: Some(port),
                rtcp_send_port: None,
                rtcp_recv_port: Some(port + 1),
                server_addr: None,
            }
        }),
        audio_transport: audio_track.map(|track| {
            let port = track.port;
            rtsp::TransportInfo::Udp {
                rtp_send_port: None,
                rtp_recv_port: Some(port),
                rtcp_send_port: None,
                rtcp_recv_port: Some(port + 1),
                server_addr: None,
            }
        }),
        video_codec: if !codec_vid.is_empty() && codec_vid != "unknown" {
            Some(codec_from_str(&codec_vid)?.into())
        } else {
            None
        },
        audio_codec: if !codec_aud.is_empty() && codec_aud != "unknown" {
            Some(codec_from_str(&codec_aud)?.into())
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

    let (peer, video_sender, audio_sender) =
        setup_webrtc(client, media_info, complete_tx, input.clone()).await?;

    if let Some(mut rx) = interleaved_rx {
        info!("Setting up TCP interleaved data handler");

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

        // Task 1: Process data from RTSP (RTP/RTCP)
        tokio::spawn(async move {
            info!("TCP interleaved receiver started");

            while let Some((channel, data)) = rx.recv().await {
                trace!("Received data on channel {}: {} bytes", channel, data.len());

                if Some(channel) == video_rtp_channel {
                    if let Some(sender) = &video_sender_clone {
                        trace!("Forwarding video RTP to WebRTC");
                        if let Err(e) = sender.send(data) {
                            error!("Failed to forward video RTP: {}", e);
                            break;
                        }
                    }
                } else if Some(channel) == video_rtcp_channel {
                    trace!("Processing video RTCP from RTSP");
                    let mut cursor = Cursor::new(data);
                    match webrtc::rtcp::packet::unmarshal(&mut cursor) {
                        Ok(packets) => {
                            if let Err(e) = peer_clone.write_rtcp(&packets).await {
                                error!("Failed to write video RTCP to WebRTC: {}", e);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse video RTCP: {}", e);
                        }
                    }
                } else if Some(channel) == audio_rtp_channel {
                    if let Some(sender) = &audio_sender_clone {
                        trace!("Forwarding audio RTP to WebRTC");
                        if let Err(e) = sender.send(data) {
                            error!("Failed to forward audio RTP: {}", e);
                            break;
                        }
                    }
                } else if Some(channel) == audio_rtcp_channel {
                    trace!("Processing audio RTCP from RTSP");
                    let mut cursor = Cursor::new(data);
                    match webrtc::rtcp::packet::unmarshal(&mut cursor) {
                        Ok(packets) => {
                            if let Err(e) = peer_clone.write_rtcp(&packets).await {
                                error!("Failed to write audio RTCP to WebRTC: {}", e);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse audio RTCP: {}", e);
                        }
                    }
                }
            }

            warn!("TCP interleaved receiver stopped");
        });

        // Task 2: Send WebRTC RTCP back to RTSP
        if let Some(tx) = interleaved_tx {
            let senders = peer.get_senders().await;

            // Video RTCP sender
            if let Some(rtcp_channel) = video_rtcp_channel {
                for sender in &senders {
                    if let Some(track) = sender.track().await
                        && track.kind() == RTPCodecType::Video
                    {
                        let tx_clone = tx.clone();
                        let sender_clone = sender.clone();

                        tokio::spawn(async move {
                            info!("Video RTCP sender started (channel {})", rtcp_channel);
                            loop {
                                match sender_clone.read_rtcp().await {
                                    Ok((packets, _)) => {
                                        for packet in packets {
                                            debug!("Sending video RTCP to RTSP: {:?}", packet);
                                            if let Ok(data) = packet.marshal()
                                                && let Err(e) =
                                                    tx_clone.send((rtcp_channel, data.to_vec()))
                                            {
                                                error!("Failed to send video RTCP: {}", e);
                                                return;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Video RTCP read error: {}", e);
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
            }

            // Audio RTCP sender
            if let Some(rtcp_channel) = audio_rtcp_channel {
                for sender in &senders {
                    if let Some(track) = sender.track().await
                        && track.kind() == RTPCodecType::Audio
                    {
                        let tx_clone = tx.clone();
                        let sender_clone = sender.clone();

                        tokio::spawn(async move {
                            info!("Audio RTCP sender started (channel {})", rtcp_channel);
                            loop {
                                match sender_clone.read_rtcp().await {
                                    Ok((packets, _)) => {
                                        for packet in packets {
                                            debug!("Sending audio RTCP to RTSP: {:?}", packet);
                                            if let Ok(data) = packet.marshal()
                                                && let Err(e) =
                                                    tx_clone.send((rtcp_channel, data.to_vec()))
                                            {
                                                error!("Failed to send audio RTCP: {}", e);
                                                return;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Audio RTCP read error: {}", e);
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
            }
        }
    } else {
        info!("Setting up UDP mode handlers");
        setup_udp_handlers(
            media_info,
            &listen_host,
            &host,
            video_sender,
            audio_sender,
            peer.clone(),
        )
        .await?;
    }

    Ok(peer)
}

async fn setup_udp_handlers(
    media_info: &rtsp::MediaInfo,
    listen_host: &str,
    host: &str,
    video_sender: Option<UnboundedSender<Vec<u8>>>,
    audio_sender: Option<UnboundedSender<Vec<u8>>>,
    peer: Arc<RTCPeerConnection>,
) -> Result<()> {
    if let Some(rtsp::TransportInfo::Udp {
        rtp_recv_port: Some(video_port),
        ..
    }) = media_info.video_transport
        && let Some(sender) = video_sender
    {
        let video_socket = UdpSocket::bind(format!("{}:{}", listen_host, video_port)).await?;
        info!(
            "Video UDP RTP listener started on {}",
            video_socket.local_addr()?
        );

        tokio::spawn(async move {
            let mut buf = vec![0u8; 2000];

            loop {
                match video_socket.recv_from(&mut buf).await {
                    Ok((n, addr)) => {
                        trace!("Received video RTP from {}: {} bytes", addr, n);
                        if let Err(e) = sender.send(buf[..n].to_vec()) {
                            error!("Failed to forward video RTP: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Video RTP receive error: {}", e);
                        break;
                    }
                }
            }
        });
    }

    // Setup audio RTP listener
    if let Some(rtsp::TransportInfo::Udp {
        rtp_recv_port: Some(audio_port),
        ..
    }) = media_info.audio_transport
        && let Some(sender) = audio_sender
    {
        let audio_socket = UdpSocket::bind(format!("{}:{}", listen_host, audio_port)).await?;
        info!(
            "Audio UDP RTP listener started on {}",
            audio_socket.local_addr()?
        );

        tokio::spawn(async move {
            let mut buf = vec![0u8; 2000];
            loop {
                match audio_socket.recv_from(&mut buf).await {
                    Ok((n, addr)) => {
                        trace!("Received audio RTP from {}: {} bytes", addr, n);
                        if let Err(e) = sender.send(buf[..n].to_vec()) {
                            error!("Failed to forward audio RTP: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Audio RTP receive error: {}", e);
                        break;
                    }
                }
            }
        });
    }

    // Setup RTCP listeners
    if let Some(rtsp::TransportInfo::Udp {
        rtcp_recv_port: Some(port),
        ..
    }) = &media_info.video_transport
    {
        info!("Setting up video RTCP listener on port {}", port);
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
        info!("Setting up audio RTCP listener on port {}", port);
        tokio::spawn(utils::rtcp_listener(
            listen_host.to_string(),
            *port,
            peer.clone(),
        ));
    }

    // Setup RTCP senders
    let senders = peer.get_senders().await;

    if let Some(rtsp::TransportInfo::Udp {
        rtcp_send_port: Some(rtcp_port),
        ..
    }) = &media_info.video_transport
    {
        info!("Setting up video RTCP sender to port {}", rtcp_port);
        for sender in &senders {
            if let Some(track) = sender.track().await
                && track.kind() == RTPCodecType::Video
            {
                tokio::spawn(read_rtcp(
                    sender.clone(),
                    listen_host.to_string(),
                    host.to_string(),
                    *rtcp_port,
                ));
            }
        }
    }

    if let Some(rtsp::TransportInfo::Udp {
        rtcp_send_port: Some(rtcp_port),
        ..
    }) = &media_info.audio_transport
    {
        info!("Setting up audio RTCP sender to port {}", rtcp_port);
        for sender in &senders {
            if let Some(track) = sender.track().await
                && track.kind() == RTPCodecType::Audio
            {
                tokio::spawn(read_rtcp(
                    sender.clone(),
                    listen_host.to_string(),
                    host.to_string(),
                    *rtcp_port,
                ));
            }
        }
    }

    Ok(())
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
                    debug!("Received RTCP packet from WebRTC: {:?}", packet);

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
                warn!("Error reading RTCP packet from WebRTC: {}", err);
                break Ok(());
            }
        }
    }
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
    let (peer, video_sender, audio_sender) =
        webrtc_start(client, media_info, complete_tx, input.to_string())
            .await
            .map_err(|error| anyhow!(format!("[{}] {}", PREFIX_LIB, error)))?;

    Ok((peer, video_sender, audio_sender))
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
            if let Ok(mut child) = child_mutex.lock()
                && let Ok(wait) = child.try_wait()
                && wait.is_some()
            {
                let _ = complete_tx.send(());
                return Ok(());
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

async fn webrtc_start(
    client: &mut Client,
    media_info: &rtsp::MediaInfo,
    complete_tx: UnboundedSender<()>,
    input: String,
) -> Result<(
    Arc<RTCPeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
)> {
    let (peer, video_sender, audio_sender) =
        new_peer(media_info, complete_tx.clone(), input).await?;

    utils::setup_webrtc_connection(peer.clone(), client).await?;
    utils::setup_peer_connection_handlers(peer.clone(), complete_tx).await;

    Ok((peer, video_sender, audio_sender))
}

async fn new_peer(
    media_info: &rtsp::MediaInfo,
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

    let video_tx = if let Some(ref video_codec_params) = media_info.video_codec {
        let video_codec: RTCRtpCodecCapability = video_codec_params.clone().into();
        setup_video_track(peer.clone(), video_codec, input.clone(), video_codec_params).await?
    } else {
        None
    };

    // let audio_tx = if let Some(ref audio_codec_params) = media_info.audio_codec {
    //     let audio_codec: RTCRtpCodecCapability = audio_codec_params.clone().into();
    //     setup_audio_track(peer.clone(), audio_codec, input).await?
    // } else {
    //     None
    // };

    let audio_tx = if let Some(ref audio_codec_params) = media_info.audio_codec {
        // 检查是否为支持的编解码器
        if is_supported_audio_codec(&audio_codec_params.codec) {
            let audio_codec: RTCRtpCodecCapability = audio_codec_params.clone().into();
            setup_audio_track(peer.clone(), audio_codec, input).await?
        } else {
            warn!(
                "Audio codec '{}' is not supported by WebRTC. Supported: Opus, G722, PCMU, PCMA",
                audio_codec_params.codec
            );
            warn!("Skipping audio track, only video will be transmitted");
            None
        }
    } else {
        None
    };

    Ok((peer, video_tx, audio_tx))
}

fn is_supported_audio_codec(codec: &str) -> bool {
    matches!(
        codec.to_uppercase().as_str(),
        "OPUS" | "G722" | "PCMU" | "PCMA"
    )
}

async fn setup_video_track(
    peer: Arc<RTCPeerConnection>,
    video_codec: RTCRtpCodecCapability,
    input: String,
    video_codec_params: &rtsp::VideoCodecParams,
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

    let video_codec_params = video_codec_params.clone();

    tokio::spawn(async move {
        debug!("Video codec: {}", video_codec.mime_type);

        let mut handler: Box<dyn payload::RePayload + Send> = match video_codec.mime_type.as_str() {
            MIME_TYPE_VP8 => Box::new(payload::RePayloadCodec::new(video_codec.mime_type.clone())),
            MIME_TYPE_VP9 => Box::new(payload::RePayloadCodec::new(video_codec.mime_type.clone())),
            MIME_TYPE_H264 => {
                let mut repayloader = payload::RePayloadCodec::new(video_codec.mime_type.clone());

                if let rtsp::VideoCodecParams::H264 { sps, pps, .. } = &video_codec_params {
                    debug!(
                        "Setting H.264 params - SPS: {} bytes, PPS: {} bytes",
                        sps.len(),
                        pps.len()
                    );
                    repayloader.set_h264_params(sps.clone(), pps.clone());
                } else {
                    warn!("Video codec params mismatch: expected H264");
                }

                Box::new(repayloader)
            }
            MIME_TYPE_HEVC => {
                let mut repayloader = payload::RePayloadCodec::new(video_codec.mime_type.clone());

                if let rtsp::VideoCodecParams::H265 { vps, sps, pps, .. } = &video_codec_params {
                    info!(
                        "Setting H.265 params - VPS: {} bytes, SPS: {} bytes, PPS: {} bytes",
                        vps.len(),
                        sps.len(),
                        pps.len()
                    );
                    repayloader.set_h265_params(vps.clone(), sps.clone(), pps.clone());
                } else {
                    warn!("Video codec params mismatch: expected H265");
                }

                Box::new(repayloader)
            }
            _ => Box::new(payload::Forward::new()),
        };

        while let Some(data) = video_rx.recv().await {
            if let Ok(packet) = Packet::unmarshal(&mut data.as_slice()) {
                trace!(
                    "Received video packet: seq={}, ts={}, marker={}",
                    packet.header.sequence_number, packet.header.timestamp, packet.header.marker
                );

                for packet in handler.payload(packet) {
                    trace!(
                        "Sending video packet: seq={}, ts={}, marker={}",
                        packet.header.sequence_number,
                        packet.header.timestamp,
                        packet.header.marker
                    );

                    if let Err(e) = video_track.write_rtp(&packet).await {
                        error!("Failed to write RTP: {}", e);
                    }
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

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[tokio::test]
//     async fn test_new_peer_with_codecs() {
//         let video_codec = Some(RTCRtpCodecCapability {
//             mime_type: "video/H264".to_string(),
//             clock_rate: 90000,
//             channels: 0,
//             sdp_fmtp_line: "".to_string(),
//             rtcp_feedback: vec![],
//         });
//         let audio_codec = Some(RTCRtpCodecCapability {
//             mime_type: "audio/opus".to_string(),
//             clock_rate: 48000,
//             channels: 2,
//             sdp_fmtp_line: "".to_string(),
//             rtcp_feedback: vec![],
//         });
//         let (complete_tx, _complete_rx) = unbounded_channel();
//         let input = "test-input".to_string();

//         let result = new_peer(video_codec, audio_codec, complete_tx, input).await;
//         assert!(result.is_ok());

//         let (peer, video_tx, audio_tx) = result.unwrap();
//         assert!(video_tx.is_some(), "Video sender should be created");
//         assert!(audio_tx.is_some(), "Audio sender should be created");

//         let senders = peer.get_senders().await;
//         assert_eq!(senders.len(), 2, "Should have two tracks (video + audio)");
//     }
// }
