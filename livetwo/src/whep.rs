use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::{Result, anyhow};
use cli::create_child;
use scopeguard::defer;
use sdp::description::common::{Address, ConnectionInformation};
use sdp::{SessionDescription, description::media::RangedPort};
use std::{
    fs::File,
    io::{Cursor, Write},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tracing::{debug, error, info, trace, warn};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::{
    peer_connection::RTCPeerConnection,
    rtp_transceiver::{
        RTCRtpTransceiverInit, rtp_codec::RTPCodecType,
        rtp_transceiver_direction::RTCRtpTransceiverDirection,
    },
    util::MarshalSize,
};

use libwish::Client;

use crate::utils;
use crate::{SCHEME_RTSP_CLIENT, SCHEME_RTSP_SERVER};

pub async fn from(
    target_url: String,
    whep_url: String,
    sdp_file: Option<String>,
    token: Option<String>,
    command: Option<String>,
) -> Result<()> {
    let input = utils::parse_input_url(&target_url)?;
    info!("Processing output URL: {}", target_url);

    let (target_host, listen_host) = utils::parse_host(&input);
    info!("Target host: {}, Listen host: {}", target_host, listen_host);

    let (complete_tx, mut complete_rx) = unbounded_channel();
    let (video_send, video_recv) = unbounded_channel::<Vec<u8>>();
    let (audio_send, audio_recv) = unbounded_channel::<Vec<u8>>();
    let codec_info = Arc::new(tokio::sync::Mutex::new(rtsp::CodecInfo::new()));
    debug!("Channels and codec info initialized");

    let mut client = Client::new(whep_url.clone(), Client::get_auth_header_map(token.clone()));
    debug!("WHEP client created");

    let (peer, answer) = webrtc_start(
        &mut client,
        video_send,
        audio_send,
        complete_tx.clone(),
        codec_info.clone(),
    )
    .await?;
    info!("WebRTC connection established");

    tokio::time::sleep(Duration::from_secs(1)).await;
    let codec_info = codec_info.lock().await;
    debug!("Codec info: {:?}", codec_info);
    let port = input.port().unwrap_or(0);

    let filtered_sdp = rtsp::filter_sdp(
        &answer.sdp,
        codec_info.video_codec.as_ref(),
        codec_info.audio_codec.as_ref(),
    )?;
    debug!("SDP filtered");

    let notify = Arc::new(Notify::new());
    let notify_clone = notify.clone();
    let complete_tx_for_child = complete_tx.clone();

    tokio::spawn(async move {
        notify_clone.notified().await;
        debug!("Received signal to start child process");

        let child = match create_child(command) {
            Ok(child) => Arc::new(child),
            Err(e) => {
                error!("Failed to create child process: {}", e);
                return;
            }
        };
        info!("Child process created");
        defer!({
            if let Some(child) = child.as_ref()
                && let Ok(mut child) = child.lock()
            {
                let _ = child.kill();
            }
        });
        let wait_child = child.clone();
        match wait_child.as_ref() {
            Some(child) => loop {
                if let Ok(mut child) = child.lock()
                    && let Ok(wait) = child.try_wait()
                    && wait.is_some()
                {
                    let _ = complete_tx_for_child.send(());
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            },
            None => info!("No child process"),
        }
    });

    let (media_info, tx, rx) = match input.scheme() {
        SCHEME_RTSP_SERVER => {
            let (media_info, interleaved_tx, interleaved_rx) = rtsp_server_mode(
                filtered_sdp,
                &listen_host,
                complete_tx.clone(),
                port,
                notify,
            )
            .await?;
            (media_info, interleaved_tx, interleaved_rx)
        }
        SCHEME_RTSP_CLIENT => {
            let (media_info, interleaved_tx, interleaved_rx) =
                rtsp_client_mode(filtered_sdp, &target_url, &target_host).await?;
            (media_info, interleaved_tx, interleaved_rx)
        }
        _ => {
            let media_info = rtp_mode(filtered_sdp, &input, sdp_file, notify.clone()).await?;
            (media_info, None, None)
        }
    };

    info!("Media info: {:?}", media_info);
    setup_rtp_handlers(
        video_recv,
        audio_recv,
        target_host.clone(),
        &media_info,
        peer.clone(),
        tx,
        rx,
    );

    tokio::select! {
        _ = complete_rx.recv() => { }
        msg = signal::wait_for_stop_signal() => warn!("Received signal: {}", msg)
    }

    let _ = peer.close().await;

    Ok(())
}

fn setup_rtp_handlers(
    video_recv: UnboundedReceiver<Vec<u8>>,
    audio_recv: UnboundedReceiver<Vec<u8>>,
    target_host: String,
    media_info: &rtsp::MediaInfo,
    peer: Arc<RTCPeerConnection>,
    interleaved_tx: Option<UnboundedSender<(u8, Vec<u8>)>>,
    interleaved_rx: Option<UnboundedReceiver<(u8, Vec<u8>)>>,
) {
    if let Some(tx) = interleaved_tx {
        setup_tcp_handlers(video_recv, audio_recv, media_info, peer, tx, interleaved_rx);
    } else {
        setup_udp_handlers(video_recv, audio_recv, target_host, media_info, peer);
    }
}

fn setup_tcp_handlers(
    mut video_recv: UnboundedReceiver<Vec<u8>>,
    mut audio_recv: UnboundedReceiver<Vec<u8>>,
    media_info: &rtsp::MediaInfo,
    peer: Arc<RTCPeerConnection>,
    tx: UnboundedSender<(u8, Vec<u8>)>,
    interleaved_rx: Option<UnboundedReceiver<(u8, Vec<u8>)>>,
) {
    // Video RTP sender
    if let Some(rtsp::TransportInfo::Tcp { rtp_channel, .. }) = &media_info.video_transport {
        let channel = *rtp_channel;
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            info!("Starting video RTP sender on channel {}", channel);

            while let Some(data) = video_recv.recv().await {
                trace!("Sending video RTP data ({} bytes)", data.len());

                if let Err(e) = tx_clone.send((channel, data)) {
                    error!("Failed to send video RTP data: {}", e);
                    break;
                }
            }

            warn!("Video RTP sender stopped");
        });
    }

    // Audio RTP sender
    if let Some(rtsp::TransportInfo::Tcp { rtp_channel, .. }) = &media_info.audio_transport {
        let channel = *rtp_channel;
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            info!("Starting audio RTP sender on channel {}", channel);

            while let Some(data) = audio_recv.recv().await {
                trace!("Sending audio RTP data ({} bytes)", data.len());

                if let Err(e) = tx_clone.send((channel, data)) {
                    error!("Failed to send audio RTP data: {}", e);
                    break;
                }
            }

            warn!("Audio RTP sender stopped");
        });
    }

    // Video RTCP sender
    if let Some(rtsp::TransportInfo::Tcp { rtcp_channel, .. }) = &media_info.video_transport {
        let channel = *rtcp_channel;
        let tx_clone = tx.clone();
        let peer_clone = peer.clone();

        tokio::spawn(async move {
            info!("Starting video RTCP sender on channel {}", channel);

            let senders = peer_clone.get_senders().await;
            for sender in senders {
                if let Some(track) = sender.track().await
                    && track.kind() == RTPCodecType::Video
                {
                    let tx_clone = tx_clone.clone();

                    tokio::spawn(async move {
                        loop {
                            match sender.read_rtcp().await {
                                Ok((packets, _)) => {
                                    for packet in packets {
                                        if let Ok(data) = packet.marshal() {
                                            trace!(
                                                "Sending video RTCP data ({} bytes)",
                                                data.len()
                                            );
                                            if let Err(e) = tx_clone.send((channel, data.to_vec()))
                                            {
                                                error!("Failed to send video RTCP: {}", e);
                                                return;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Error reading video RTCP: {}", e);
                                    break;
                                }
                            }
                        }
                    });
                }
            }
        });
    }

    // Audio RTCP sender
    if let Some(rtsp::TransportInfo::Tcp { rtcp_channel, .. }) = &media_info.audio_transport {
        let channel = *rtcp_channel;
        let tx_clone = tx.clone();
        let peer_clone = peer.clone();

        tokio::spawn(async move {
            info!("Starting audio RTCP sender on channel {}", channel);

            let senders = peer_clone.get_senders().await;
            for sender in senders {
                if let Some(track) = sender.track().await
                    && track.kind() == RTPCodecType::Audio
                {
                    let tx_clone = tx_clone.clone();

                    tokio::spawn(async move {
                        loop {
                            match sender.read_rtcp().await {
                                Ok((packets, _)) => {
                                    for packet in packets {
                                        if let Ok(data) = packet.marshal() {
                                            trace!(
                                                "Sending audio RTCP data ({} bytes)",
                                                data.len()
                                            );
                                            if let Err(e) = tx_clone.send((channel, data.to_vec()))
                                            {
                                                error!("Failed to send audio RTCP: {}", e);
                                                return;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Error reading audio RTCP: {}", e);
                                    break;
                                }
                            }
                        }
                    });
                }
            }
        });
    }

    // RTCP receiver from RTSP client
    if let Some(mut rx) = interleaved_rx {
        let peer_clone = peer.clone();
        tokio::spawn(async move {
            info!("Starting RTCP receiver from RTSP client");

            while let Some((channel, data)) = rx.recv().await {
                debug!(
                    "Received RTCP data from RTSP client on channel {}, {} bytes",
                    channel,
                    data.len()
                );

                let mut cursor = Cursor::new(data.clone());
                match webrtc::rtcp::packet::unmarshal(&mut cursor) {
                    Ok(packets) => {
                        trace!("Successfully parsed {} RTCP packets", packets.len());

                        if let Err(e) = peer_clone.write_rtcp(&packets).await {
                            error!("Failed to write RTCP packets to WebRTC: {}", e);
                        } else {
                            trace!(
                                "Successfully forwarded {} RTCP packets to WebRTC",
                                packets.len()
                            );
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse RTCP packet: {}", e);
                    }
                }
            }

            warn!("RTCP receiver from RTSP client stopped");
        });
    }
}

fn setup_udp_handlers(
    video_recv: UnboundedReceiver<Vec<u8>>,
    audio_recv: UnboundedReceiver<Vec<u8>>,
    target_host: String,
    media_info: &rtsp::MediaInfo,
    peer: Arc<RTCPeerConnection>,
) {
    let listen_host = if target_host.parse::<Ipv6Addr>().is_ok() {
        Ipv6Addr::UNSPECIFIED.to_string()
    } else {
        Ipv4Addr::UNSPECIFIED.to_string()
    };

    // Video RTP sender
    tokio::spawn(utils::rtp_send(
        video_recv,
        listen_host.clone(),
        target_host.clone(),
        media_info.video_transport.as_ref().and_then(|t| match t {
            rtsp::TransportInfo::Udp { rtp_send_port, .. } => *rtp_send_port,
            _ => None,
        }),
        media_info.video_transport.as_ref().and_then(|t| match t {
            rtsp::TransportInfo::Udp { rtp_recv_port, .. } => *rtp_recv_port,
            _ => None,
        }),
    ));
    info!("Video RTP sender started");

    // Audio RTP sender
    tokio::spawn(utils::rtp_send(
        audio_recv,
        listen_host.clone(),
        target_host.clone(),
        media_info.audio_transport.as_ref().and_then(|t| match t {
            rtsp::TransportInfo::Udp { rtp_send_port, .. } => *rtp_send_port,
            _ => None,
        }),
        media_info.audio_transport.as_ref().and_then(|t| match t {
            rtsp::TransportInfo::Udp { rtp_recv_port, .. } => *rtp_recv_port,
            _ => None,
        }),
    ));
    info!("Audio RTP sender started");

    // Video RTCP listener
    let target_host_clone = target_host.clone();
    if let Some(rtsp::TransportInfo::Udp {
        rtcp_recv_port: Some(port),
        ..
    }) = &media_info.video_transport
    {
        info!("Starting up video RTCP on port {}", port);
        tokio::spawn(utils::rtcp_listener(target_host_clone, *port, peer.clone()));
    }

    // Audio RTCP listener
    if let Some(rtsp::TransportInfo::Udp {
        rtcp_recv_port: Some(port),
        ..
    }) = &media_info.audio_transport
    {
        info!("Starting up audio RTCP on port {}", port);
        tokio::spawn(utils::rtcp_listener(target_host, *port, peer.clone()));
    }
}

async fn rtsp_server_mode(
    filtered_sdp: String,
    listen_host: &str,
    _complete_tx: UnboundedSender<()>,
    tcp_port: u16,
    notify: Arc<Notify>,
) -> Result<(
    rtsp::MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    info!("Starting RTSP server mode for WHEP");

    let listen_addr = format!("{}:{}", listen_host, tcp_port);
    let sdp_bytes = filtered_sdp.into_bytes();

    // Notify child process to start
    notify.notify_one();
    info!("Sent signal to start child process");

    // Use unified RTSP server session
    rtsp::setup_rtsp_server_session(
        &listen_addr,
        sdp_bytes,
        rtsp::SessionMode::Pull,
        true, // Use TCP for now
    )
    .await
}

async fn rtsp_client_mode(
    filtered_sdp: String,
    target_url: &str,
    target_host: &str,
) -> Result<(
    rtsp::MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    info!("Starting RTSP client mode for WHEP");

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

    // Push mode: pull from WebRTC and push to RTSP
    rtsp::setup_rtsp_session(
        &clean_url_str,
        Some(filtered_sdp),
        target_host,
        rtsp::RtspMode::Push,
        use_tcp,
    )
    .await
}

async fn rtp_mode(
    filtered_sdp: String,
    input: &url::Url,
    sdp_filename: Option<String>,
    notify: Arc<Notify>,
) -> Result<rtsp::MediaInfo> {
    let mut reader = Cursor::new(filtered_sdp.as_bytes());
    let session = SessionDescription::unmarshal(&mut reader).unwrap();
    let (target_host, _listen_host) = utils::parse_host(input);

    let mut video_port: Option<u16> = None;
    let mut audio_port: Option<u16> = None;

    for (key, value) in input.query_pairs() {
        match key.as_ref() {
            "video" => {
                video_port = value.parse::<u16>().ok();
            }
            "audio" => {
                audio_port = value.parse::<u16>().ok();
            }
            _ => {}
        }
    }

    let mut video_codec = None;
    let mut audio_codec = None;

    for media in &session.media_descriptions {
        if media.media_name.media == "video" {
            video_codec = media
                .attributes
                .iter()
                .find(|attr| attr.key == "rtpmap")
                .and_then(|attr| attr.value.as_ref())
                .and_then(|value| {
                    value
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .split('/')
                        .next()
                        .map(|codec_str| cli::codec_from_str(codec_str).ok())
                })
                .unwrap_or(None);
        } else if media.media_name.media == "audio" {
            audio_codec = media
                .attributes
                .iter()
                .find(|attr| attr.key == "rtpmap")
                .and_then(|attr| attr.value.as_ref())
                .and_then(|value| {
                    value
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .split('/')
                        .next()
                        .map(|codec_str| cli::codec_from_str(codec_str).ok())
                })
                .unwrap_or(None);
        }
    }

    let video_port = if video_codec.is_some() {
        match video_port {
            Some(port) => {
                debug!("Video port set from URL: {}", port);
                Some(port)
            }
            None => {
                info!("No video port specified in URL, using default port: 5004");
                Some(5004)
            }
        }
    } else {
        None
    };

    let audio_port = if audio_codec.is_some() {
        match audio_port {
            Some(port) => {
                debug!("Audio port set from URL: {}", port);
                Some(port)
            }
            None => {
                info!("No audio port specified in URL, using default port: 5006");
                Some(5006)
            }
        }
    } else {
        None
    };

    let media_info = rtsp::MediaInfo {
        video_transport: video_port.map(|port| rtsp::TransportInfo::Udp {
            rtp_send_port: Some(port),
            rtp_recv_port: None,
            rtcp_send_port: Some(port + 1),
            rtcp_recv_port: None,
            server_addr: None,
        }),
        audio_transport: audio_port.map(|port| rtsp::TransportInfo::Udp {
            rtp_send_port: Some(port),
            rtp_recv_port: None,
            rtcp_send_port: Some(port + 1),
            rtcp_recv_port: None,
            server_addr: None,
        }),
        video_codec: video_codec.map(|c| c.into()),
        audio_codec: audio_codec.map(|c| c.into()),
    };

    let connection_info = ConnectionInformation {
        network_type: "IN".to_string(),
        address_type: if target_host.parse::<Ipv6Addr>().is_ok() {
            "IP6"
        } else {
            "IP4"
        }
        .to_string(),
        address: Some(Address {
            address: target_host.to_string(),
            ttl: None,
            range: None,
        }),
    };

    let mut session = session;
    session.connection_information = Some(connection_info.clone());

    for media in &mut session.media_descriptions {
        media.connection_information = Some(connection_info.clone());

        if media.media_name.media == "video" {
            if let Some(rtsp::TransportInfo::Udp {
                rtp_send_port: Some(port),
                ..
            }) = &media_info.video_transport
            {
                media.media_name.port = RangedPort {
                    value: *port as isize,
                    range: None,
                };
            }
        } else if media.media_name.media == "audio"
            && let Some(rtsp::TransportInfo::Udp {
                rtp_send_port: Some(port),
                ..
            }) = &media_info.audio_transport
        {
            media.media_name.port = RangedPort {
                value: *port as isize,
                range: None,
            };
        }
    }

    let sdp = session.marshal();

    let file_path = sdp_filename.unwrap_or_else(|| "output.sdp".to_string());
    debug!("SDP written to {:?}", file_path);
    let mut file = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(file_path)?;
    file.write_all(sdp.as_bytes())?;
    notify.notify_one();
    info!("Sent signal to start child process after RTP mode SDP write");

    Ok(media_info)
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
}
