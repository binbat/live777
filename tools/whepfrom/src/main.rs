use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use clap::Parser;
use cli::{create_child, get_codec_type, Codec};

use libwish::Client;
use scopeguard::defer;
use tokio::{
    net::UdpSocket,
    signal,
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use webrtc::ice_transport::ice_credential_type::RTCIceCredentialType;
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        RTCPeerConnection,
    },
    rtp_transceiver::{
        rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters},
        rtp_transceiver_direction::RTCRtpTransceiverDirection,
        RTCRtpTransceiverInit,
    },
    util::MarshalSize,
};

const PREFIX_LIB: &str = "WEBRTC";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
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
    let payload_type = args.payload_type;
    assert!((96..=127).contains(&payload_type));
    let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;
    udp_socket.connect(&args.target).await?;
    let mut client = Client::new(
        args.url,
        Client::get_auth_header_map(args.auth_basic, args.auth_token),
    );
    let child = Arc::new(create_child(args.command)?);
    defer!({
        if let Some(child) = child.as_ref() {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }
    });
    let (complete_tx, mut complete_rx) = unbounded_channel();
    let (send, mut recv) = unbounded_channel::<Vec<u8>>();

    let peer = webrtc_start(
        &mut client,
        args.codec.into(),
        send,
        payload_type,
        complete_tx.clone(),
    )
    .await
    .map_err(|error| anyhow!(format!("[{}] {}", PREFIX_LIB, error)))?;

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
                let timeout = tokio::time::sleep(Duration::from_secs(1));
                tokio::pin!(timeout);
                let _ = timeout.as_mut().await;
            },
            None => println!("No child process"),
        }
    });
    tokio::select! {
        _= complete_rx.recv() => { }
        _= signal::ctrl_c() => {}
    }
    let _ = client.remove_resource().await;
    let _ = peer.close().await;
    Ok(())
}

async fn webrtc_start(
    client: &mut Client,
    codec: RTCRtpCodecCapability,
    send: UnboundedSender<Vec<u8>>,
    payload_type: u8,
    complete_tx: UnboundedSender<()>,
) -> Result<Arc<RTCPeerConnection>> {
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
    let (answer, _ice_servers) = client.wish(offer.sdp.clone()).await?;
    peer.set_local_description(offer.clone()).await?;
    peer.set_remote_description(answer).await?;
    Ok(peer)
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
            println!("connection state changed: {}", s);
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
                let size = rtp_packet.marshal_size();
                let data = b[0..size].to_vec();
                let _ = sender.send(data);
            }
        });
        Box::pin(async {})
    }));
    Ok(peer)
}
