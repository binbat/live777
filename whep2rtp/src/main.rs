use std::{
    process::Child,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Result;
use clap::Parser;
use cli::{create_child, get_codec_type, Codec};

use tokio::{
    net::UdpSocket,
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
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
use whip_whep::{get_answer, get_ide_servers};
#[derive(Parser)]
#[command(author, version, about,long_about = None)]
struct Args {
    #[arg(short, long)]
    target: String,
    #[arg(short, long, value_enum)]
    codec: Codec,
    #[arg(short, long)]
    url: String,
    #[arg(long)]
    command: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;
    udp_socket.connect(&args.target).await?;
    let (send, mut recv) = unbounded_channel::<Vec<u8>>();
    let ide_servers = get_ide_servers(args.url.clone()).await?;
    let child = create_child(args.command)?;
    let peer = new_peer(args.codec.into(), ide_servers, child.clone(), send)
        .await
        .unwrap();
    let offser = peer.create_offer(None).await.unwrap();
    let _ = peer.set_local_description(offser.clone()).await.unwrap();
    let (answer, _etag) = get_answer(args.url.clone(), offser.sdp).await.unwrap();
    peer.set_remote_description(answer).await.unwrap();
    let rtp_sender = async move {
        while let Some(data) = recv.recv().await {
            let _ = udp_socket.send(&data).await;
        }
    };
    if let Some(child) = child.as_ref() {
        tokio::spawn(rtp_sender);
        loop {
            if let Ok(mut child) = child.write() {
                if let Ok(exit) = child.try_wait() {
                    if let Some(exit) = exit {
                        let _ = peer.close().await;
                        std::process::exit(exit.code().unwrap())
                    }
                }
            }
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
        }
    } else {
        rtp_sender.await;
    }
    Ok(())
}

async fn new_peer(
    codec: RTCRtpCodecCapability,
    ice_servers: Vec<RTCIceServer>,
    child: Arc<Option<RwLock<Child>>>,
    sender: UnboundedSender<Vec<u8>>,
) -> Result<Arc<RTCPeerConnection>> {
    let ct = get_codec_type(&codec);
    let mut m = MediaEngine::default();
    m.register_codec(
        RTCRtpCodecParameters {
            capability: codec,
            payload_type: 96,
            ..Default::default()
        },
        ct,
    )?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();
    let config = RTCConfiguration {
        ice_servers,
        ..Default::default()
    };
    let peer = Arc::new(api.new_peer_connection(config).await?);
    let _ = peer
        .add_transceiver_from_kind(
            ct,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                send_encodings: vec![],
            }),
        )
        .await?;
    let pc = peer.clone();
    peer.on_peer_connection_state_change(Box::new(move |s| {
        let pc = pc.clone();
        let child = child.clone();
        tokio::spawn(async move {
            println!("connection state changed: {}", s);
            match s {
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                    let _ = pc.close().await;
                }
                RTCPeerConnectionState::Closed => {
                    if let Some(child) = child.as_ref() {
                        if let Ok(mut child) = child.write() {
                            let _ = child.kill();
                        }
                    }
                    std::process::exit(1);
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
