use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use clap::Parser;
use cli::{create_child, Codec};

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
    rtp,
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{
        track_local_static_rtp::TrackLocalStaticRTP, TrackLocal, TrackLocalWriter,
    },
    util::Unmarshal,
};

const PREFIX_LIB: &str = "WEBRTC";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 0)]
    port: u16,
    #[arg(short, long, value_enum)]
    codec: Codec,
    /// The WHIP server endpoint to POST SDP offer to. e.g.: https://example.com/whip/777
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
    let listener = UdpSocket::bind(format!("0.0.0.0:{}", args.port)).await?;
    let port = listener.local_addr()?.port();
    println!("=== RTP listener started : {} ===", port);
    let mut client = Client::new(
        args.url,
        Client::get_auth_header_map(args.auth_basic, args.auth_token),
    );
    let child = if let Some(command) = args.command {
        let command = command.replace("{port}", &port.to_string());
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
    let (complete_tx, mut complete_rx) = unbounded_channel();
    let (peer, sender) = webrtc_start(&mut client, args.codec.into(), complete_tx.clone())
        .await
        .map_err(|error| anyhow!(format!("[{}] {}", PREFIX_LIB, error)))?;
    tokio::spawn(rtp_listener(listener, sender));
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
    println!("RTP listener closed");
    let _ = client.remove_resource().await;
    let _ = peer.close().await;
    Ok(())
}

async fn rtp_listener(socker: UdpSocket, sender: UnboundedSender<Vec<u8>>) {
    let mut inbound_rtp_packet = vec![0u8; 1600];
    while let Ok((n, _)) = socker.recv_from(&mut inbound_rtp_packet).await {
        let data = inbound_rtp_packet[..n].to_vec();
        let _ = sender.send(data);
    }
}

async fn webrtc_start(
    client: &mut Client,
    codec: RTCRtpCodecCapability,
    complete_tx: UnboundedSender<()>,
) -> Result<(Arc<RTCPeerConnection>, UnboundedSender<Vec<u8>>)> {
    let (peer, sender) = new_peer(codec, complete_tx.clone()).await?;
    let offer = peer.create_offer(None).await?;
    let (answer, _ice_servers) = client.wish(offer.sdp.clone()).await?;
    peer.set_local_description(offer.clone()).await?;
    peer.set_remote_description(answer).await?;
    Ok((peer, sender))
}

async fn new_peer(
    codec: RTCRtpCodecCapability,
    complete_tx: UnboundedSender<()>,
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
    let track = Arc::new(TrackLocalStaticRTP::new(
        codec,
        "webrtc".to_owned(),
        "webrtc-rs".to_owned(),
    ));
    let _ = peer
        .add_track(track.clone() as Arc<dyn TrackLocal + Send + Sync>)
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;
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
