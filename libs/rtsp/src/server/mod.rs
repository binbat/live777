pub mod handler;
pub mod server_session;
pub mod unified_session;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

pub use handler::Handler;
pub use server_session::ServerSession;
pub use unified_session::{PortUpdate, RtspServerSession, setup_rtsp_server_session};

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

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

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

    pub async fn list_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }

    pub async fn is_full(&self) -> bool {
        let sessions = self.sessions.read().await;
        sessions.len() >= self.config.max_connections
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
    async fn test_server_config_access() {
        let config = ServerConfig::default();
        let server = RtspServer::new(config.clone());

        assert_eq!(server.config().max_connections, config.max_connections);
        assert_eq!(server.config().session_timeout, config.session_timeout);
    }

    #[tokio::test]
    async fn test_server_is_full() {
        let config = ServerConfig {
            max_connections: 0,
            ..Default::default()
        };
        let server = RtspServer::new(config);

        assert!(server.is_full().await);
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let config = ServerConfig::default();
        let server = RtspServer::new(config);

        let sessions = server.list_sessions().await;
        assert_eq!(sessions.len(), 0);
    }
}
