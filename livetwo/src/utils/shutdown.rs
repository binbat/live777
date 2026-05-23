use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use libwish::Client;
use webrtc::peer_connection::PeerConnection;

pub async fn graceful_shutdown(
    name: &str,
    client: &mut Client,
    peer: Arc<dyn PeerConnection>,
) {
    info!("Starting {} graceful shutdown", name);

    let shutdown_timeout = Duration::from_secs(5);

    tokio::select! {
        _ = async {
            match client.remove_resource().await {
                Ok(_) => info!("{} resource removed successfully", name),
                Err(e) => warn!("Failed to remove {} resource: {}", name, e),
            }

            match peer.close().await {
                Ok(_) => info!("PeerConnection closed successfully"),
                Err(e) => warn!("Failed to close peer connection: {}", e),
            }

            info!("WebRTC resources cleaned up");
        } => {
            info!("{} graceful shutdown completed", name);
        }
        _ = tokio::time::sleep(shutdown_timeout) => {
            warn!("{} graceful shutdown timed out after {:?}", name, shutdown_timeout);
        }
    }
}
