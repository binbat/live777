use anyhow::{Result, anyhow};
use libwish::Client;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};
use webrtc::{
    api::{APIBuilder, interceptor_registry::register_default_interceptors, media_engine::*},
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{
        RTCPeerConnection, configuration::RTCConfiguration,
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
};

pub async fn create_api() -> Result<(APIBuilder, RTCConfiguration)> {
    debug!("Creating WebRTC API");
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry);

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            username: "".to_string(),
            credential: "".to_string(),
        }],
        ..Default::default()
    };

    debug!("Default ICE configuration created");
    Ok((api, config))
}

pub async fn setup_connection(
    peer: Arc<RTCPeerConnection>,
    client: &mut Client,
) -> Result<RTCSessionDescription> {
    let offer = peer.create_offer(None).await?;
    debug!("WebRTC offer created");

    let mut gather_complete = peer.gathering_complete_promise().await;
    peer.set_local_description(offer).await?;
    let _ = gather_complete.recv().await;

    let (answer, ice_servers) = client
        .wish(peer.local_description().await.unwrap().sdp)
        .await?;

    debug!("ICE servers from response: {:?}", ice_servers);

    let mut current_config = peer.get_configuration().await;
    current_config.ice_servers = ice_servers;
    peer.set_configuration(current_config).await?;
    debug!("ICE configuration updated");

    peer.set_remote_description(answer.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    debug!("Remote description set successfully");
    Ok(answer)
}

pub async fn setup_handlers(peer: Arc<RTCPeerConnection>, complete_tx: UnboundedSender<()>) {
    let pc = peer.clone();
    peer.on_peer_connection_state_change(Box::new(move |s| {
        let pc = pc.clone();
        let complete_tx = complete_tx.clone();
        tokio::spawn(async move {
            warn!("Connection state changed: {}", s);
            match s {
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                    let _ = pc.close().await;
                    warn!("Connection closed due to failure or disconnection");
                }
                RTCPeerConnectionState::Closed => {
                    let _ = complete_tx.send(());
                    info!("Connection closed normally");
                }
                _ => debug!("Connection state: {}", s),
            }
        });
        Box::pin(async {})
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_api() {
        let (api_builder, config) = create_api().await.unwrap();
        assert_eq!(config.ice_servers.len(), 1);
        assert_eq!(
            config.ice_servers[0].urls,
            vec!["stun:stun.l.google.com:19302"]
        );
        let api = api_builder.build();
        let peer = api.new_peer_connection(config).await.unwrap();
        assert_eq!(peer.connection_state(), RTCPeerConnectionState::New);
    }
}
