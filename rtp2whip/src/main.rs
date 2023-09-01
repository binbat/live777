use std::{process::Stdio, sync::Arc, time::Duration};

use anyhow::Result;
use clap::Parser;
use cli::Codec;

use tokio::{
    net::UdpSocket,
    process::Command,
    sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        RwLock,
    },
};
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        RTCPeerConnection,
    },
    rtp,
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{
        track_local_static_rtp::TrackLocalStaticRTP, TrackLocal, TrackLocalWriter,
    },
    util::Unmarshal,
};
use whip_whep::{get_answer, get_ide_servers};
#[derive(Parser)]
#[command(author, version, about,long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 0)]
    port: u16,
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
    let ide_servers = get_ide_servers(args.url.clone()).await?;
    let (peer, sender) = new_peer(args.codec.into(), ide_servers).await?;
    let listener = UdpSocket::bind(format!("0.0.0.0:{}", args.port)).await?;
    let port = listener.local_addr()?.port();
    println!("=== RTP listener started : {} ===", port);
    let child = if let Some(command) = args.command {
        let command = command.replace("{port}", &port.to_string());
        let mut args = shellwords::split(&command)?;
        Arc::new(Some(RwLock::new(
            Command::new(args.remove(0))
                .args(args)
                .stdout(Stdio::inherit())
                .spawn()?,
        )))
    } else {
        Arc::new(Default::default())
    };
    let pc = peer.clone();
    let child_arc = child.clone();
    peer.on_peer_connection_state_change(Box::new(move |s| {
        let pc = pc.clone();
        let child = child_arc.clone();
        tokio::spawn(async move {
            println!("connection state changed: {}", s);
            match s {
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                    let _ = pc.close().await;
                }
                RTCPeerConnectionState::Closed => {
                    if let Some(child) = child.as_ref() {
                        let _ = child.write().await.kill().await;
                    }
                    std::process::exit(1);
                }
                _ => {}
            };
        });
        Box::pin(async {})
    }));
    let offser = peer.create_offer(None).await?;
    let _ = peer.set_local_description(offser.clone()).await?;
    let (answer, _etag) = get_answer(args.url.clone(), offser.sdp).await?;
    peer.set_remote_description(answer).await?;
    if let Some(child) = child.as_ref() {
        tokio::spawn(rtp_listener(listener, sender));
        loop {
            if let Ok(exit) = child.write().await.try_wait() {
                if let Some(exit) = exit {
                    let _ = peer.close().await;
                    std::process::exit(exit.code().unwrap())
                }
            }
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
        }
    } else {
        rtp_listener(listener, sender).await;
    }
    Ok(())
}

async fn rtp_listener(socker: UdpSocket, sender: UnboundedSender<Vec<u8>>) {
    let mut inbound_rtp_packet = vec![0u8; 1600];
    while let Ok((n, _)) = socker.recv_from(&mut inbound_rtp_packet).await {
        let data = inbound_rtp_packet[..n].to_vec();
        let _ = sender.send(data);
    }
}

async fn new_peer(
    codec: RTCRtpCodecCapability,
    ice_servers: Vec<RTCIceServer>,
) -> Result<(Arc<RTCPeerConnection>, UnboundedSender<Vec<u8>>)> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;
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
    let peer = api.new_peer_connection(config).await?;
    let track = Arc::new(TrackLocalStaticRTP::new(
        codec,
        "webrtc".to_owned(),
        "webrtc-rs".to_owned(),
    ));
    let _ = peer
        .add_track(track.clone() as Arc<dyn TrackLocal + Send + Sync>)
        .await?;
    let (send, mut recv) = unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        let mut sequence_number: u16 = 0;
        while let Some(data) = recv.recv().await {
            if let Ok(mut packet) = rtp::packet::Packet::unmarshal(&mut data.as_slice()) {
                packet.header.sequence_number = sequence_number;
                let _ = track.write_rtp(&packet).await;
                sequence_number = sequence_number.wrapping_add(1);
            }
        }
    });
    Ok((Arc::new(peer), send))
}
