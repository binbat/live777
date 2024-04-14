use crate::config::Config;
use crate::storage;
use crate::storage::Storage;
use serde::Serialize;
use std::sync::Arc;
use webrtc::ice_transport::ice_server::RTCIceServer;

#[derive(Clone)]
pub struct ManagerConfig {
    pub ice_servers: Vec<RTCIceServer>,
    pub publish_leave_timeout: u64,
    pub storage: Option<Arc<Box<dyn Storage + 'static + Send + Sync>>>,
    pub meta_data: MetaData,
}

#[derive(Serialize, Clone)]
pub struct MetaData {
    #[serde(rename = "pubMax")]
    pub pub_max: u64,
    #[serde(rename = "subMax")]
    pub sub_max: u64,
    #[serde(rename = "reforwardMaximumIdleTime")]
    pub reforward_maximum_idle_time: u64,
    pub authorization: Option<String>,
    #[serde(rename = "adminAuthorization")]
    pub admin_authorization: Option<String>,
}

impl From<Config> for MetaData {
    fn from(value: Config) -> Self {
        Self {
            pub_max: value.node_info.meta_data.pub_max.0,
            sub_max: value.node_info.meta_data.sub_max.0,
            reforward_maximum_idle_time: value.node_info.meta_data.reforward_maximum_idle_time.0,
            authorization: value.auth.to_authorizations().first().cloned(),
            admin_authorization: value.admin_auth.to_authorizations().first().cloned(),
        }
    }
}

impl ManagerConfig {
    pub async fn from_config(cfg: Config) -> Self {
        let ice_servers = cfg
            .ice_servers
            .clone()
            .into_iter()
            .map(|i| i.into())
            .collect();
        let storage = if let Some(storage) = &cfg.node_info.storage {
            Some(Arc::new(
                storage::new(cfg.node_info.ip_port.clone(), storage.clone()).await,
            ))
        } else {
            None
        };
        Self {
            ice_servers,
            publish_leave_timeout: cfg.publish_leave_timeout.0,
            storage,
            meta_data: MetaData::from(cfg.clone()),
        }
    }
}
