use crate::config::Config;

use std::net::SocketAddr;
use webrtc::ice_transport::ice_server::RTCIceServer;

#[derive(Clone)]
pub struct ManagerConfig {
    pub ice_servers: Vec<RTCIceServer>,
    pub reforward_close_sub: bool,
    pub publish_leave_timeout: u64,
    pub addr: SocketAddr,
    pub webhooks: Vec<String>,
}

impl ManagerConfig {
    pub fn from_config(cfg: Config) -> Self {
        let ice_servers = cfg
            .ice_servers
            .clone()
            .into_iter()
            .map(|i| i.into())
            .collect();
        Self {
            ice_servers,
            reforward_close_sub: cfg.stream_info.reforward_close_sub,
            publish_leave_timeout: cfg.stream_info.publish_leave_timeout.0,
            addr: cfg.node_addr.unwrap(),
            webhooks: cfg.webhooks.clone(),
        }
    }
}
