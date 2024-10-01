use crate::config::Config;

use webrtc::ice_transport::ice_server::RTCIceServer;

#[derive(Clone)]
pub struct ManagerConfig {
    pub ice_servers: Vec<RTCIceServer>,
    pub reforward_close_sub: bool,
    pub webhooks: Vec<String>,

    pub auto_create_pub: bool,
    pub auto_create_sub: bool,
    pub auto_delete_pub: i64,
    pub auto_delete_sub: i64,
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
            reforward_close_sub: cfg.strategy.reforward_close_sub,
            webhooks: cfg.webhook.webhooks.clone(),
            auto_create_pub: cfg.strategy.auto_create_whip,
            auto_create_sub: cfg.strategy.auto_create_whep,
            auto_delete_pub: cfg.strategy.auto_delete_whip.0 * 1000,
            auto_delete_sub: cfg.strategy.auto_delete_whep.0 * 1000,
        }
    }
}
