use anyhow::{Result, anyhow};
use libwish::Client;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfiguration, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
    RTCPeerConnectionState, RTCSessionDescription, Registry,
};

pub fn create_peer_connection_builder() -> Result<(
    PeerConnectionBuilder<std::net::SocketAddr>,
    RTCConfiguration,
)> {
    debug!("Creating WebRTC API");
    let m = MediaEngine::default();

    let registry = Registry::new();

    let builder = PeerConnectionBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry);

    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            username: "".to_string(),
            credential: "".to_string(),
        }])
        .build();

    debug!("Default ICE configuration created");
    Ok((builder, config))
}

pub async fn setup_connection(
    peer: Arc<dyn PeerConnection>,
    client: &mut Client,
    gather_complete: Arc<Notify>,
) -> Result<RTCSessionDescription> {
    let offer = peer.create_offer(None).await?;
    debug!("WebRTC offer created");

    peer.set_local_description(offer).await?;

    // Wait for ICE gathering to complete with a timeout.
    if tokio::time::timeout(
        std::time::Duration::from_secs(3),
        gather_complete.notified(),
    )
    .await
    .is_err()
    {
        warn!("ICE gathering timed out after 3s, using partial description");
    }

    let local_desc = peer.local_description().await.unwrap();

    let (answer, ice_servers) = client.wish(local_desc.sdp).await?;

    debug!("ICE servers from response: {:?}", ice_servers);

    let new_config = RTCConfigurationBuilder::new()
        .with_ice_servers(ice_servers)
        .build();
    peer.set_configuration(new_config).await?;
    debug!("ICE configuration updated");

    peer.set_remote_description(answer.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    debug!("Remote description set successfully");
    Ok(answer)
}

pub fn create_event_handler(
    ct: CancellationToken,
    gather_complete: Arc<Notify>,
) -> Arc<dyn PeerConnectionEventHandler> {
    Arc::new(Handler {
        ct,
        gather_complete,
    })
}

#[derive(Clone)]
struct Handler {
    ct: CancellationToken,
    gather_complete: Arc<Notify>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        warn!("Connection state changed: {}", state);
        match state {
            RTCPeerConnectionState::Failed => {
                self.ct.cancel();
                warn!("Connection closed due to failure");
            }
            RTCPeerConnectionState::Closed => {
                self.ct.cancel();
                info!("Connection closed normally");
            }
            _ => debug!("Connection state: {}", state),
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct NoopHandler;
    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for NoopHandler {}

    #[tokio::test]
    async fn test_create_peer_connection() {
        let (builder, config) = create_peer_connection_builder().unwrap();
        assert_eq!(config.ice_servers.len(), 1);
        assert_eq!(
            config.ice_servers[0].urls,
            vec!["stun:stun.l.google.com:19302"]
        );
        let peer = builder
            .with_configuration(config)
            .with_handler(Arc::new(NoopHandler))
            .with_udp_addrs(vec!["127.0.0.1:0".parse().unwrap()])
            .build()
            .await
            .unwrap();
        let _ = peer;
    }
}
