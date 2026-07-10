use crate::config::Config;

use std::net::SocketAddr;
use webrtc::peer_connection::RTCIceServer;

#[derive(Clone)]
pub struct ManagerConfig {
    pub ice_servers: Vec<RTCIceServer>,
    pub ice_udp_addrs: Vec<SocketAddr>,
    pub stream: crate::config::StreamConfig,
    /// Global strategy (used directly and merged with per-stream overrides).
    pub strategy: api::strategy::Strategy,
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
            stream: cfg.stream,
            strategy: cfg.strategy,
        }
    }

    /// Return the effective strategy for a stream, merging the global strategy
    /// with any per-stream override configured under `[stream.<name>.strategy]`.
    pub fn effective_strategy(&self, stream: &str) -> api::strategy::Strategy {
        let override_strategy = self
            .stream
            .streams
            .get(stream)
            .and_then(|e| e.strategy.as_ref());
        api::strategy::Strategy::effective(&self.strategy, override_strategy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn strategy(
        each_stream_max_sub: u16,
        cascade_push_close_sub: bool,
        auto_create_whip: bool,
        auto_create_whep: bool,
        auto_delete_whip: i64,
        auto_delete_whep: i64,
    ) -> api::strategy::Strategy {
        api::strategy::Strategy {
            each_stream_max_sub: api::strategy::EachStreamMaxSub(each_stream_max_sub),
            cascade_push_close_sub,
            auto_create_whip,
            auto_create_whep,
            auto_delete_whip: api::strategy::AutoDestrayTime(auto_delete_whip),
            auto_delete_whep: api::strategy::AutoDestrayTime(auto_delete_whep),
        }
    }

    fn config_with_strategy(
        global: api::strategy::Strategy,
        streams: HashMap<String, crate::config::StreamEntry>,
    ) -> Config {
        Config {
            strategy: global,
            stream: crate::config::StreamConfig { streams },
            ..Default::default()
        }
    }

    #[test]
    fn test_effective_strategy_no_override() {
        let global = strategy(10, false, true, true, -1, -1);
        let cfg = config_with_strategy(global.clone(), HashMap::new());
        let manager_cfg = ManagerConfig::from_config(cfg);
        let effective = manager_cfg.effective_strategy("unknown");
        assert_eq!(effective, global);
    }

    #[test]
    fn test_effective_strategy_with_override() {
        let global = strategy(10, false, true, true, -1, -1);
        let mut streams = HashMap::new();
        streams.insert(
            "cam1".to_string(),
            crate::config::StreamEntry {
                strategy: Some(strategy(2, true, false, false, 0, 1000)),
                ..Default::default()
            },
        );
        let cfg = config_with_strategy(global, streams);
        let manager_cfg = ManagerConfig::from_config(cfg);
        let effective = manager_cfg.effective_strategy("cam1");
        assert_eq!(
            effective.each_stream_max_sub,
            api::strategy::EachStreamMaxSub(2)
        );
        assert!(effective.cascade_push_close_sub);
        assert!(!effective.auto_create_whip);
        assert!(!effective.auto_create_whep);
        assert_eq!(
            effective.auto_delete_whip,
            api::strategy::AutoDestrayTime(0)
        );
        assert_eq!(
            effective.auto_delete_whep,
            api::strategy::AutoDestrayTime(1000)
        );
    }

    #[test]
    fn test_effective_strategy_unknown_stream_uses_global() {
        let global = strategy(20, true, false, false, 500, 1500);
        let mut streams = HashMap::new();
        streams.insert(
            "cam1".to_string(),
            crate::config::StreamEntry {
                strategy: Some(strategy(2, false, true, true, 0, 1000)),
                ..Default::default()
            },
        );
        let cfg = config_with_strategy(global.clone(), streams);
        let manager_cfg = ManagerConfig::from_config(cfg);
        let effective = manager_cfg.effective_strategy("unknown");
        assert_eq!(effective, global);
    }
}
