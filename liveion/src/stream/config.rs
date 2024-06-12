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

    pub create_whip: bool,
    pub create_whep: bool,
    pub delete_whip: i64,
    pub delete_whep: i64,
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
            create_whip: true,
            create_whep: true,
            delete_whip: 60000,
            delete_whep: 60000,
        }
    }
}
