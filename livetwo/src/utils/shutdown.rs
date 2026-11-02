use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use libwish::Client;
use webrtc::peer_connection::PeerConnection;

pub async fn graceful_shutdown(name: &str, client: &mut Client, peer: Arc<dyn PeerConnection>) {
    info!("Starting {} graceful shutdown", name);

    let shutdown_timeout = Duration::from_secs(5);
    let remove = async {
        if client.session_url.is_some() {
            match tokio::time::timeout(shutdown_timeout, client.remove_resource()).await {
                Ok(Ok(_)) => info!("{} resource removed successfully", name),
                Ok(Err(e)) => warn!("Failed to remove {} resource: {}", name, e),
                Err(_) => warn!(
                    "{} resource removal timed out after {:?}",
                    name, shutdown_timeout
                ),
            }
        }
    };
    let close = async {
        match tokio::time::timeout(shutdown_timeout, peer.close()).await {
            Ok(Ok(_)) => info!("PeerConnection closed successfully"),
            Ok(Err(e)) => warn!("Failed to close peer connection: {}", e),
            Err(_) => warn!("{} peer close timed out after {:?}", name, shutdown_timeout),
        }
    };

    tokio::join!(remove, close);
    info!("{} graceful shutdown completed", name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc::peer_connection::{
        MediaEngine, PeerConnectionBuilder, RTCConfigurationBuilder, Registry,
    };

    #[tokio::test]
    async fn unresponsive_delete_does_not_skip_peer_close() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let mut client = Client::new(format!("http://{addr}/whep/live"), None);
        client.session_url = Some(format!("http://{addr}/session/test"));
        let peer: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_media_engine(MediaEngine::default())
                .with_interceptor_registry(Registry::new())
                .with_handler(crate::utils::webrtc::create_event_handler(
                    tokio_util::sync::CancellationToken::new(),
                    Arc::new(tokio::sync::Notify::new()),
                ))
                .with_udp_addrs(api::webrtc::resolve_webrtc_ice_udp_addrs(Some(vec![
                    "127.0.0.1:0".to_string(),
                ])))
                .with_configuration(RTCConfigurationBuilder::new().build())
                .build()
                .await
                .unwrap(),
        );

        let started = std::time::Instant::now();
        tokio::time::timeout(
            Duration::from_secs(6),
            graceful_shutdown("test", &mut client, peer.clone()),
        )
        .await
        .expect("shutdown exceeded its independent cleanup deadlines");
        server.abort();

        assert!(peer.create_offer(None).await.is_err());
        assert!(started.elapsed() >= Duration::from_secs(5));
    }
}
