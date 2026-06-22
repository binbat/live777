#[cfg(feature = "source")]
use crate::config::Channel;
use crate::config::Config;

use std::net::SocketAddr;
use webrtc::peer_connection::RTCIceServer;

#[derive(Clone)]
pub struct ManagerConfig {
    pub ice_servers: Vec<RTCIceServer>,
    pub ice_udp_addrs: Vec<SocketAddr>,
    #[cfg(feature = "cascade")]
    pub cascade_push_close_sub: bool,
    pub webhooks: Vec<String>,
    pub auto_create_pub: bool,
    pub auto_create_sub: bool,
    pub auto_delete_pub: i64,
    pub auto_delete_sub: i64,
    #[cfg(feature = "source")]
    pub channel: Channel,
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
            ice_udp_addrs: api::webrtc::resolve_webrtc_ice_udp_addrs(Some(
                cfg.webrtc.ice_udp_addrs.clone(),
            )),
            #[cfg(feature = "cascade")]
            cascade_push_close_sub: cfg.strategy.cascade_push_close_sub,
            webhooks: cfg.webhook.webhooks.clone(),
            auto_create_pub: cfg.strategy.auto_create_whip,
            auto_create_sub: cfg.strategy.auto_create_whep,
            auto_delete_pub: cfg.strategy.auto_delete_whip.0,
            auto_delete_sub: cfg.strategy.auto_delete_whep.0,
            #[cfg(feature = "source")]
            channel: cfg.channel,
        }
    }
}
