use std::{process::Stdio, sync::Arc};

use anyhow::Result;
use clap::Parser;
use cli::Codec;

use tokio::{
    net::UdpSocket,
    process::Command,
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{configuration::RTCConfiguration, RTCPeerConnection},
    rtp,
    rtp_transceiver::rtp_codec::{RTCRtpCodecParameters, RTPCodecType},
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
    // TODO peer on_event

    let offser = peer.create_offer(None).await?;
    let _ = peer.set_local_description(offser.clone()).await?;
    let (answer, _etag) = get_answer(args.url.clone(), offser.sdp).await?;
    peer.set_remote_description(answer).await?;
    let listener = UdpSocket::bind(format!("0.0.0.0:{}", args.port)).await?;
    let port = listener.local_addr()?.port();
    println!("=== RTP listener started : {} ===", port);
    if let Some(command) = args.command {
        let command = command.replace("{port}", &port.to_string());
        let mut args = shellwords::split(&command)?;
        let mut child = Command::new(args.remove(0))
            .args(args)
            .stdout(Stdio::inherit())
            .spawn()?;
        let _ = rtp_listener(listener, sender).await;
        child.kill().await?;
    } else {
        let _ = rtp_listener(listener, sender).await;
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
    codec: RTCRtpCodecParameters,
    ice_servers: Vec<RTCIceServer>,
) -> Result<(RTCPeerConnection, UnboundedSender<Vec<u8>>)> {
    let mut m = MediaEngine::default();
    m.register_codec(codec.clone(), RTPCodecType::Video)?;
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
        codec.capability,
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
    Ok((peer, send))
}
