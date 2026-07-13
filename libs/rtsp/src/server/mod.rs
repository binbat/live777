pub mod handler;
pub mod server_session;
pub mod unified_session;

use std::net::SocketAddr;

use crate::constants::server;
pub use handler::Handler;
pub use server_session::ServerSession;
pub use unified_session::{
    PortUpdate, RtspServerSession, SessionEndpoint, SessionHandler, setup_rtsp_server_with_handler,
};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub listen_addr: SocketAddr,
    pub max_connections: usize,
    pub session_timeout: u64,
    pub enable_auth: bool,
    pub username: String,
    pub password: String,
    pub realm: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8554".parse().unwrap(),
            max_connections: server::DEFAULT_MAX_CONNECTIONS,
            session_timeout: server::DEFAULT_SESSION_TIMEOUT,
            enable_auth: false,
            username: String::new(),
            password: String::new(),
            realm: "live777".to_string(),
        }
    }
}
