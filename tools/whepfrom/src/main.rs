use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use clap::{ArgAction, Parser, ValueEnum};
use core::net::Ipv4Addr;
use scopeguard::defer;
use tokio::net::TcpListener;
use tokio::{
    net::UdpSocket,
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use tracing::{debug, error, info, trace, warn, Level};
use webrtc::ice_transport::ice_credential_type::RTCIceCredentialType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        RTCPeerConnection,
    },
    rtcp,
    rtp_transceiver::{
        rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters},
        rtp_transceiver_direction::RTCRtpTransceiverDirection,
        RTCRtpTransceiverInit,
    },
    util::MarshalSize,
};

use cli::{create_child, get_codec_type, Codec};
use libwish::Client;

const PREFIX_LIB: &str = "WEBRTC";

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Mode {
    Rtsp,
    Rtp,
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    #[arg(short, long, value_enum, default_value_t = Mode::Rtsp)]
    mode: Mode,
    /// Set Listener address
    #[arg(long, default_value_t = Ipv4Addr::UNSPECIFIED.to_string())]
    host: String,
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(short, long)]
    target: String,
    #[arg(short, long, value_enum)]
    codec: Codec,
    /// value: [96, 127]
    #[arg(short, long, default_value_t = 96)]
    payload_type: u8,
    /// The WHEP server endpoint to POST SDP offer to. e.g.: https://example.com/whep/777
    #[arg(short, long)]
    url: String,
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
        "whepfrom={},webrtc=error",
        match args.verbose {
            0 => Level::WARN,
            1 => Level::INFO,
            2 => Level::DEBUG,
            _ => Level::TRACE,
        }
    ));

    let host = args.host.clone();
    let mut _codec = args.codec;
    let mut rtp_port = args.port;

    let udp_socket = UdpSocket::bind(format!("{}:0", host)).await?;

    let (complete_tx, mut complete_rx) = unbounded_channel();

    let payload_type = args.payload_type;
    assert!((96..=127).contains(&payload_type));
    let mut client = Client::new(
        args.url,
        Client::get_auth_header_map(args.auth_basic, args.auth_token),
    );
    let (send, mut recv) = unbounded_channel::<Vec<u8>>();

    let (peer, answer) = webrtc_start(
        &mut client,
        args.codec.into(),
        send,
        payload_type,
        complete_tx.clone(),
    )
    .await
    .map_err(|error| anyhow!(format!("[{}] {}", PREFIX_LIB, error)))?;

    let mut rtcp_port = 0;
    if args.mode == Mode::Rtsp {
        let (tx, mut rx) = unbounded_channel::<String>();

        let mut handler = rtsp::Handler::new(tx, complete_tx.clone());
        handler.set_sdp(answer.sdp.as_bytes().to_vec());
        info!("sdp:{:?}", answer.sdp);

        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("{}:{}", host, args.port))
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

        let (rtp_listen_port, _rtcp_listen_port, rtp_server_port) =
            match (rx.recv().await, rx.recv().await, rx.recv().await) {
                (Some(rtp), Some(rtcp), Some(rtp_server)) => {
                    let rtp_port = rtp.parse::<u16>().unwrap_or(8002);
                    let rtcp_port = rtcp.parse::<u16>().unwrap_or(8003);
                    let rtp_server_port = rtp_server.parse::<u16>().unwrap_or(8004);
                    println! {"receive port: {},{},{}",rtp_port,rtcp_port,rtp_server_port};
                    (rtp_port, rtcp_port, rtp_server_port)
                }
                _ => {
                    println!("Error receiving ports, using default values.");
                    (8002, 8003, 8004)
                }
            };
        rtp_port = rtp_listen_port;
        println!("=== Received RTPMAP: {} ===", rtp_port);
        rtcp_port = rtp_server_port + 1;
    }
    udp_socket
        .connect(format!("127.0.0.1:{}", rtp_port))
        .await?;

    let child = Arc::new(create_child(args.command)?);
    defer!({
        if let Some(child) = child.as_ref() {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }
    });

    tokio::spawn(async move {
        while let Some(data) = recv.recv().await {
            let _ = udp_socket.send(&data).await;
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
    if args.mode == Mode::Rtsp {
        tokio::spawn(rtcp_listener(args.host, rtcp_port, peer.clone()));
    }

    tokio::select! {
        _= complete_rx.recv() => { }
        msg = signal::wait_for_stop_signal() => warn!("Received signal: {}", msg)
    }
    let _ = client.remove_resource().await;
    let _ = peer.close().await;
    Ok(())
}

async fn rtcp_listener(host: String, rtcp_port: u16, peer: Arc<RTCPeerConnection>) {
    let rtcp_listener = UdpSocket::bind(format!("{}:{}", host, rtcp_port))
        .await
        .unwrap();
    println!(
        "RTCP listener bound to: {}",
        rtcp_listener.local_addr().unwrap()
    );
    let mut rtcp_buf = vec![0u8; 1500];

    loop {
        match rtcp_listener.recv_from(&mut rtcp_buf).await {
            Ok((len, addr)) => {
                if len > 0 {
                    debug!("Received {} bytes of RTCP data from {}", len, addr);
                    let mut rtcp_data = &rtcp_buf[..len];

                    match rtcp::packet::unmarshal(&mut rtcp_data) {
                        Ok(rtcp_packets) => {
                            for packet in rtcp_packets {
                                debug!("Received RTCP packet from {}: {:?}", addr, packet);
                                match peer.write_rtcp(&[packet]).await {
                                    Ok(_) => {
                                        debug!("Successfully sent RTCP packet to remote peer");
                                    }
                                    Err(e) => {
                                        error!("Error sending RTCP data to remote peer: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to parse RTCP packet: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                error!("Error receiving RTCP data: {}", e);
            }
        }
    }
}

async fn webrtc_start(
    client: &mut Client,
    codec: RTCRtpCodecCapability,
    send: UnboundedSender<Vec<u8>>,
    payload_type: u8,
    complete_tx: UnboundedSender<()>,
) -> Result<(Arc<RTCPeerConnection>, RTCSessionDescription)> {
    let peer = new_peer(
        RTCRtpCodecParameters {
            capability: codec,
            payload_type,
            stats_id: Default::default(),
        },
        complete_tx.clone(),
        send,
    )
    .await?;
    let offer = peer.create_offer(None).await?;

    let mut gather_complete = peer.gathering_complete_promise().await;
    peer.set_local_description(offer).await?;
    let _ = gather_complete.recv().await;

    let (answer, ice_servers) = client
        .wish(peer.local_description().await.unwrap().sdp.clone())
        .await?;

    debug!("Get http header link ice servers: {:?}", ice_servers);
    let mut current_config = peer.get_configuration().await;
    current_config.ice_servers.clone_from(&ice_servers);
    peer.set_configuration(current_config.clone()).await?;

    peer.set_remote_description(answer.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    Ok((peer, answer))
}

async fn new_peer(
    codec: RTCRtpCodecParameters,
    complete_tx: UnboundedSender<()>,
    sender: UnboundedSender<Vec<u8>>,
) -> Result<Arc<RTCPeerConnection>> {
    let ct = get_codec_type(&codec.capability);
    let mut m = MediaEngine::default();
    m.register_codec(codec, ct)?;
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
    let _ = peer
        .add_transceiver_from_kind(
            ct,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                send_encodings: vec![],
            }),
        )
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;
    let pc = peer.clone();
    peer.on_peer_connection_state_change(Box::new(move |s| {
        let pc = pc.clone();
        let complete_tx = complete_tx.clone();
        tokio::spawn(async move {
            warn!("connection state changed: {}", s);
            match s {
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                    let _ = pc.close().await;
                }
                RTCPeerConnectionState::Closed => {
                    let _ = complete_tx.send(());
                }
                _ => {}
            };
        });
        Box::pin(async {})
    }));
    peer.on_track(Box::new(move |track, _, _| {
        let sender = sender.clone();
        tokio::spawn(async move {
            let mut b = [0u8; 1500];
            while let Ok((rtp_packet, _)) = track.read(&mut b).await {
                trace!("received packet: {}", rtp_packet);
                let size = rtp_packet.marshal_size();
                let data = b[0..size].to_vec();
                let _ = sender.send(data);
            }
        });
        Box::pin(async {})
    }));
    Ok(peer)
}
