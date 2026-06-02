use anyhow::{Result, anyhow};
use libwish::Client;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfiguration, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
    RTCPeerConnectionState, RTCSessionDescription, Registry,
};

const OFFER_ICE_CANDIDATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const OFFER_ICE_CANDIDATE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

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

pub fn ice_udp_addrs() -> Vec<SocketAddr> {
    api::webrtc::resolve_webrtc_ice_udp_addrs(None)
}

pub async fn setup_connection(
    peer: Arc<dyn PeerConnection>,
    client: &mut Client,
    gather_complete: Arc<Notify>,
) -> Result<RTCSessionDescription> {
    let offer = peer.create_offer(None).await?;
    debug!("WebRTC offer created");

    peer.set_local_description(offer).await?;

    let local_desc = wait_for_local_ice_candidate(peer.clone(), gather_complete).await?;
    info!(
        "WebRTC local SDP offer summary:\n{}",
        summarize_sdp(&local_desc.sdp)
    );

    let (answer, ice_servers) = client.wish(local_desc.sdp).await?;
    info!(
        "WebRTC remote SDP answer summary:\n{}",
        summarize_sdp(&answer.sdp)
    );

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

async fn wait_for_local_ice_candidate(
    peer: Arc<dyn PeerConnection>,
    gather_complete: Arc<Notify>,
) -> Result<RTCSessionDescription> {
    let deadline = tokio::time::sleep(OFFER_ICE_CANDIDATE_TIMEOUT);
    tokio::pin!(deadline);
    let mut poll = tokio::time::interval(OFFER_ICE_CANDIDATE_POLL_INTERVAL);

    loop {
        if let Some(desc) = peer.local_description().await
            && sdp_has_ice_candidate(&desc.sdp)
        {
            return Ok(desc);
        }

        tokio::select! {
            _ = gather_complete.notified() => {
                if let Some(desc) = peer.local_description().await {
                    if sdp_has_ice_candidate(&desc.sdp) {
                        return Ok(desc);
                    }
                    return Err(anyhow!(
                        "WHIP local SDP offer has no ICE candidates after ICE gathering completed:\n{}",
                        summarize_sdp(&desc.sdp)
                    ));
                }
            }
            _ = poll.tick() => {}
            _ = &mut deadline => {
                let summary = peer
                    .local_description()
                    .await
                    .map(|desc| summarize_sdp(&desc.sdp))
                    .unwrap_or_else(|| "<no local description>".to_string());
                return Err(anyhow!(
                    "WHIP local SDP offer has no ICE candidates within {}ms:\n{}",
                    OFFER_ICE_CANDIDATE_TIMEOUT.as_millis(),
                    summary
                ));
            }
        }
    }
}

fn sdp_has_ice_candidate(sdp: &str) -> bool {
    sdp.lines().any(|line| line.starts_with("a=candidate:"))
}

pub fn summarize_sdp(sdp: &str) -> String {
    let mut lines = Vec::new();
    for line in sdp.lines() {
        if line.starts_with("m=")
            || line.starts_with("a=rtpmap:")
            || line.starts_with("a=fmtp:")
            || line.starts_with("a=sendonly")
            || line.starts_with("a=recvonly")
            || line.starts_with("a=sendrecv")
            || line.starts_with("a=inactive")
            || line.starts_with("a=setup:")
            || line.starts_with("a=fingerprint:")
            || line.starts_with("a=ice-ufrag:")
            || line.starts_with("a=ice-pwd:")
            || line.starts_with("a=rtcp-mux")
            || line.starts_with("a=candidate:")
        {
            lines.push(line.to_string());
        }
    }

    if lines.is_empty() {
        "<no relevant SDP lines>".to_string()
    } else {
        lines.join("\n")
    }
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
        assert_eq!(config.ice_servers().len(), 1);
        assert_eq!(
            config.ice_servers()[0].urls,
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

    #[tokio::test]
    async fn offer_sdp_does_not_advertise_unspecified_candidate_addr() {
        let (builder, config) = create_peer_connection_builder().unwrap();
        let gather_complete = Arc::new(Notify::new());
        let peer = builder
            .with_configuration(config)
            .with_handler(create_event_handler(
                CancellationToken::new(),
                gather_complete.clone(),
            ))
            .with_udp_addrs(api::webrtc::resolve_webrtc_ice_udp_addrs(Some(vec![
                "0.0.0.0:0".to_string(),
            ])))
            .build()
            .await
            .unwrap();

        let peer: Arc<dyn PeerConnection> = Arc::new(peer);
        peer.create_data_channel("ice-candidate-test", None)
            .await
            .unwrap();

        let offer = peer.create_offer(None).await.unwrap();
        peer.set_local_description(offer).await.unwrap();
        let local_desc = wait_for_local_ice_candidate(peer, gather_complete)
            .await
            .unwrap();

        assert!(
            local_desc.sdp.contains(" 127.0.0.1 "),
            "expected loopback ICE candidate in SDP:\n{}",
            summarize_sdp(&local_desc.sdp)
        );
        assert!(
            !local_desc.sdp.contains(" 0.0.0.0 "),
            "unspecified ICE candidate leaked into SDP:\n{}",
            summarize_sdp(&local_desc.sdp)
        );
    }

    #[test]
    fn sdp_candidate_detection_checks_actual_candidate_lines() {
        assert!(sdp_has_ice_candidate(
            "v=0\na=candidate:1 1 udp 1 127.0.0.1 1 typ host\n"
        ));
        assert!(!sdp_has_ice_candidate("v=0\na=end-of-candidates\n"));
    }

    #[test]
    fn sdp_summary_keeps_connection_relevant_lines() {
        let summary = summarize_sdp(
            "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 9\r\na=rtpmap:9 G722/8000\r\na=sendonly\r\na=ice-ufrag:abc\r\na=candidate:1 1 udp 1 127.0.0.1 1 typ host\r\na=msid:ignored\r\n",
        );

        assert!(summary.contains("m=audio"));
        assert!(summary.contains("a=rtpmap:9 G722/8000"));
        assert!(summary.contains("a=ice-ufrag:abc"));
        assert!(summary.contains("a=candidate:1"));
        assert!(!summary.contains("msid"));
    }
}
