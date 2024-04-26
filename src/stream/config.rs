use crate::config::Config;
use live777_storage::{Auth, NodeMetaData, Storage, StreamInfo};
use std::sync::Arc;
use webrtc::ice_transport::ice_server::RTCIceServer;

#[derive(Clone)]
pub struct ManagerConfig {
    pub ice_servers: Vec<RTCIceServer>,
    pub reforward_close_sub: bool,
    pub publish_leave_timeout: u64,
    pub storage: Option<Arc<Box<dyn Storage + 'static + Send + Sync>>>,
    pub node_addr: String,
    pub metadata: NodeMetaData,
}

impl From<Config> for NodeMetaData {
    fn from(value: Config) -> Self {
        Self {
            auth: Auth {
                authorization: value.auth.to_authorizations().first().cloned(),
                admin_authorization: value.admin_auth.to_authorizations().first().cloned(),
            },
            stream_info: StreamInfo {
                pub_max: value.stream_info.pub_max.0,
                sub_max: value.stream_info.sub_max.0,
                reforward_maximum_idle_time: value
                    .node_info
                    .meta_data
                    .reforward_maximum_idle_time
                    .0,
                reforward_cascade: value.node_info.meta_data.reforward_cascade,
            },
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
                live777_storage::new(storage.clone().into()).await.unwrap(),
            ))
        } else {
            None
        };
        Self {
            ice_servers,
            reforward_close_sub: cfg.stream_info.reforward_close_sub,
            publish_leave_timeout: cfg.stream_info.publish_leave_timeout.0,
            storage,
            node_addr: cfg.node_info.ip_port.clone(),
            metadata: NodeMetaData::from(cfg.clone()),
        }
    }
}
