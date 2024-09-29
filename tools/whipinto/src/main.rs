use std::fs;
use std::{sync::Arc, time::Duration, vec};

use anyhow::{anyhow, Result};
use clap::{ArgAction, Parser};
use core::net::{Ipv4Addr, Ipv6Addr};
use scopeguard::defer;
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use tracing::{debug, error, info, trace, warn, Level};
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
    rtp_transceiver::{rtp_codec::RTCRtpCodecCapability, rtp_sender::RTCRtpSender},
    track::track_local::{
        track_local_static_rtp::TrackLocalStaticRTP, TrackLocal, TrackLocalWriter,
    },
    util::Unmarshal,
};

use cli::{codec_from_str, create_child};
use libwish::Client;
use tokio::sync::mpsc::UnboundedReceiver;

use rtspclient::setup_rtsp_session;

mod payload;
mod rtspclient;
#[cfg(test)]
mod test;

const PREFIX_LIB: &str = "WEBRTC";
const SCHEME_RTSP_SERVER: &str = "rtsp-listen";
const SCHEME_RTSP_CLIENT: &str = "rtsp";
const SCHEME_RTP_SDP: &str = "sdp";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// rtsp://[username]:[password]@[ip]:[port]/[stream] Or <stream.sdp>
    #[arg(short, long, default_value_t = format!("{}://0.0.0.0:8554", SCHEME_RTSP_SERVER))]
    input: String,
    /// The WHIP server endpoint to POST SDP offer to. e.g.: https://example.com/whip/777
    #[arg(short, long)]
    whip: String,
    /// Set Listener address
    #[arg(long)]
    host: Option<String>,
    /// Run a command as childprocess
    #[arg(long)]
    command: Option<String>,
    /// Authentication basic to use, will be sent in the HTTP Header as 'Basic ' e.g.: admin:public
    #[arg(long)]
    auth_basic: Option<String>,
    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(long)]
    auth_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    utils::set_log(format!(
        "whipinto={},webrtc=error",
        match args.verbose {
            0 => Level::WARN,
            1 => Level::INFO,
            2 => Level::DEBUG,
            _ => Level::TRACE,
        }
    ));

    let input = Url::parse(&args.input).unwrap_or(
        Url::parse(&format!(
            "{}://{}:0/{}",
            SCHEME_RTP_SDP,
            Ipv4Addr::UNSPECIFIED,
            args.input
        ))
        .unwrap(),
    );
    info!("=== Received Input: {} ===", args.input);

    let mut host = match input.host().unwrap() {
        Host::Domain(_) | Host::Ipv4(_) => Ipv4Addr::UNSPECIFIED.to_string(),
        Host::Ipv6(_) => Ipv6Addr::UNSPECIFIED.to_string(),
    };

    if let Some(ref h) = args.host {
        debug!("=== Specified set host, using {} ===", h);
        host.clone_from(h)
    }

    let video_port = input.port().unwrap_or(0);

    let (complete_tx, mut complete_rx) = unbounded_channel();

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

        while let Some(media_info) = rx.recv().await {
            let _ = setup_webrtc_session(
                &args,
                &complete_tx,
                &mut complete_rx,
                &media_info,
                host.clone(),
            )
            .await;
        }
    } else if input.scheme() == SCHEME_RTSP_CLIENT {
        let (rtp_port, audio_port, video_codec, audio_codec) =
            setup_rtsp_session(&args.input).await?;
        let media_info = rtsp::MediaInfo {
            video_rtp_client: None,
            audio_rtp_client: None,
            video_codec: Some(video_codec),
            audio_codec: Some(audio_codec),
            video_rtp_server: Some(rtp_port),
            audio_rtp_server: Some(audio_port),
            video_rtcp_client: Some(rtp_port + 1),
        };
        let _ =
            setup_webrtc_session(&args, &complete_tx, &mut complete_rx, &media_info, host).await;
    } else {
        let sdp = sdp_types::Session::parse(&fs::read(&args.input).unwrap()).unwrap();
        let video_track = sdp.medias.iter().find(|md| md.media == "video");
        let audio_track = sdp.medias.iter().find(|md| md.media == "audio");

        let codec_vid = video_track
            .and_then(|md| {
                md.attributes.iter().find_map(|attr| {
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
            })
            .unwrap_or_else(|| "unknown".to_string());
        let codec_aud = audio_track
            .and_then(|md| {
                md.attributes.iter().find_map(|attr| {
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
            })
            .unwrap_or_else(|| "unknown".to_string());
        let media_info = rtsp::MediaInfo {
            video_rtp_client: None,
            audio_rtp_client: None,
            video_codec: Some(codec_from_str(&codec_vid)?),
            audio_codec: Some(codec_from_str(&codec_aud)?),
            video_rtp_server: video_track.map(|track| track.port),
            audio_rtp_server: audio_track.map(|track| track.port),
            video_rtcp_client: None,
        };

        let _ =
            setup_webrtc_session(&args, &complete_tx, &mut complete_rx, &media_info, host).await;
    }
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
                    info!("Received RTCP packet from {}: {:?}", addr, packet);
                    if let Err(err) = peer.write_rtcp(&[packet]).await {
                        warn!("Failed to send RTCP packet: {}", err);
                    }
                }
            }
        }
    }
}

async fn read_rtcp(sender: Arc<RTCRtpSender>, host: String, port: u16) -> Result<()> {
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
                        if let Err(err) =
                            udp_socket.send_to(&buf, format!("{}:{}", host, port)).await
                        {
                            warn!("Failed to forward RTCP packet: {}", err);
                        } else {
                            debug!("Forwarded RTCP packet to {}:{}", host, port);
                        }
                    }
                }
            }
            Err(err) => {
                warn!("Error reading RTCP packet from remote peer: {}", err);
            }
        }
    }
}

async fn setup_webrtc_session(
    args: &Args,
    complete_tx: &UnboundedSender<()>,
    complete_rx: &mut UnboundedReceiver<()>,
    media_info: &rtsp::MediaInfo,
    host: String,
) -> Result<()> {
    let mut client = Client::new(
        args.whip.clone(),
        Client::get_auth_header_map(args.auth_basic.clone(), args.auth_token.clone()),
    );

    let child = if let Some(command) = &args.command {
        let command = command.replace("{port}", "0");
        Arc::new(create_child(Some(command))?)
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

    let (peer, video_sender, audio_sender) = webrtc_start(
        &mut client,
        media_info.video_codec.map(|c| c.into()),
        media_info.audio_codec.map(|c| c.into()),
        complete_tx.clone(),
        args,
    )
    .await
    .map_err(|error| anyhow!(format!("[{}] {}", PREFIX_LIB, error)))?;

    if let Some(video_port) = media_info.video_rtp_server {
        let video_listener = UdpSocket::bind(format!("{}:{}", host, video_port)).await?;
        info!(
            "=== video listener started : {} ===",
            video_listener.local_addr().unwrap()
        );
        tokio::spawn(rtp_listener(video_listener, video_sender));
    } else {
        info!("Video stream is None");
    }

    if let Some(audio_port) = media_info.audio_rtp_server {
        let audio_listener = UdpSocket::bind(format!("{}:{}", host, audio_port)).await?;
        info!(
            "=== audio listener started : {} ===",
            audio_listener.local_addr().unwrap()
        );
        tokio::spawn(rtp_listener(audio_listener, audio_sender));
    } else {
        info!("Audio stream is None.");
    }

    let rtcp_listener_port = media_info.video_rtp_server.unwrap_or(0) + 1;
    tokio::spawn(rtcp_listener(
        host.clone(),
        rtcp_listener_port,
        peer.clone(),
    ));
    let senders = peer.get_senders().await;
    if let Some(rtcp_sender_port) = media_info.video_rtcp_client {
        for sender in senders {
            tokio::spawn(read_rtcp(sender, host.clone(), rtcp_sender_port));
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

async fn webrtc_start(
    client: &mut Client,
    video_codec: Option<RTCRtpCodecCapability>,
    audio_codec: Option<RTCRtpCodecCapability>,
    complete_tx: UnboundedSender<()>,
    args: &Args,
) -> Result<(
    Arc<RTCPeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
)> {
    let (peer, video_sender, audio_sender) = new_peer(
        video_codec,
        audio_codec,
        complete_tx.clone(),
        args.input.clone(),
    )
    .await?;
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
            .add_track(video_track.clone() as Arc<dyn TrackLocal + Send + Sync>)
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
            .add_track(audio_track.clone() as Arc<dyn TrackLocal + Send + Sync>)
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
