//! RTSP Server module
//!
//! Provides RTSP server functionality including:
//! - Request handling (OPTIONS, DESCRIBE, SETUP, PLAY, RECORD, etc.)
//! - Session management
//! - Unified session handler for WHIP and WHEP

pub mod handler;
pub mod server_session;
pub mod unified_session;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

pub use handler::Handler;
pub use server_session::ServerSession;
pub use unified_session::{RtspServerSession, setup_rtsp_server_session};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub listen_addr: SocketAddr,
    pub max_connections: usize,
    pub session_timeout: u64,
    pub enable_auth: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8554".parse().unwrap(),
            max_connections: 100,
            session_timeout: 60,
            enable_auth: false,
        }
    }
}

pub struct RtspServer {
    config: ServerConfig,
    sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
}

impl RtspServer {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // /// Start the RTSP server
    // ///
    // /// This will bind to the configured address and start accepting connections
    // pub async fn start(&self) -> Result<()> {
    //     use tokio::net::TcpListener;
    //     use tracing::info;

    //     let listener = TcpListener::bind(self.config.listen_addr).await?;
    //     info!("RTSP server listening on {}", self.config.listen_addr);

    //     loop {
    //         let (socket, addr) = listener.accept().await?;
    //         info!("New connection from {}", addr);

    //         let sessions = Arc::clone(&self.sessions);
    //         let config = self.config.clone();

    //         tokio::spawn(async move {
    //             // In real usage, this would create a RtspServerSession
    //             info!("Connection from {} accepted", addr);
    //         });
    //     }
    // }

    pub async fn active_sessions(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    pub async fn cleanup_expired_sessions(&self) {
        let mut sessions = self.sessions.write().await;
        let now = std::time::Instant::now();

        sessions.retain(|id, session| {
            if session.is_expired(now) {
                tracing::info!("Removing expired session: {}", id);
                false
            } else {
                true
            }
        });
    }

    pub async fn get_session(&self, id: &str) -> Option<ServerSession> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    pub async fn remove_session(&self, id: &str) -> bool {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.session_timeout, 60);
        assert!(!config.enable_auth);
    }

    #[tokio::test]
    async fn test_rtsp_server_creation() {
        let config = ServerConfig::default();
        let server = RtspServer::new(config);
        assert_eq!(server.active_sessions().await, 0);
    }

    #[tokio::test]
    async fn test_session_management() {
        let config = ServerConfig::default();
        let server = RtspServer::new(config);

        assert_eq!(server.active_sessions().await, 0);

        let addr: SocketAddr = "127.0.0.1:8554".parse().unwrap();
        let session = ServerSession::new("test-id".to_string(), addr, 60);
        {
            let mut sessions = server.sessions.write().await;
            sessions.insert("test-id".to_string(), session);
        }

        assert_eq!(server.active_sessions().await, 1);

        let retrieved = server.get_session("test-id").await;
        assert!(retrieved.is_some());

        assert!(server.remove_session("test-id").await);
        assert_eq!(server.active_sessions().await, 0);
    }
}
