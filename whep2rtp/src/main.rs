use std::{sync::Arc, time::Duration};

use anyhow::Result;
use clap::Parser;
use cli::{create_child, get_codec_type, Codec};

use tokio::{
    net::UdpSocket,
    signal,
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
use whip_whep::Client;
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
    let client = Client::new(args.url, None);
    let ide_servers = client.get_ide_servers().await?;
    let child = create_child(args.command)?;
    let (complete_tx, mut complete_rx) = unbounded_channel();
    let (send, mut recv) = unbounded_channel::<Vec<u8>>();
    let peer = new_peer(args.codec.into(), ide_servers, complete_tx.clone(), send)
        .await
        .unwrap();
    let offser = peer.create_offer(None).await.unwrap();
    let _ = peer.set_local_description(offser.clone()).await.unwrap();
    let (answer, etag) = client.get_answer(offser.sdp).await.unwrap();
    peer.set_remote_description(answer).await.unwrap();
    tokio::spawn(async move {
        while let Some(data) = recv.recv().await {
            let _ = udp_socket.send(&data).await;
        }
    });
    let wait_child = child.clone();
    tokio::spawn(async move {
        if let Some(child) = wait_child.as_ref() {
            loop {
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
            }
        }
    });
    tokio::select! {
        _= complete_rx.recv() => { }
        _= signal::ctrl_c() => {}
    }
    let _ = client.remove_resource(etag).await;
    let _ = peer.close().await;
    if let Some(child) = child.as_ref() {
        if let Ok(mut child) = child.lock() {
            let _ = child.kill();
        }
    }
    Ok(())
}

async fn new_peer(
    codec: RTCRtpCodecCapability,
    ice_servers: Vec<RTCIceServer>,
    complete_tx: UnboundedSender<()>,
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
