#[cfg(feature = "node_operate")]
pub mod node_operate;
pub mod redis;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use self::redis::RedisStandaloneStorage;

#[cfg(feature = "node_operate")]
use crate::node_operate::Node;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NodeMetaData {
    pub auth: Auth,
    #[serde(rename = "streamInfo")]
    pub stream_info: StreamInfo,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Auth {
    pub authorization: Option<String>,
    #[serde(rename = "adminAuthorization")]
    pub admin_authorization: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StreamInfo {
    #[serde(rename = "pubMax")]
    pub pub_max: u64,
    #[serde(rename = "subMax")]
    pub sub_max: u64,
    #[serde(rename = "reforwardMaximumIdleTime")]
    pub reforward_maximum_idle_time: u64,
    #[serde(rename = "reforwardCascade")]
    pub reforward_cascade: bool,
}

#[async_trait]
pub trait Storage {
    #[cfg(feature = "storage_operate")]
    async fn registry(&self, node_addr: String, metadata: NodeMetaData) -> Result<()>;
    #[cfg(feature = "storage_operate")]
    async fn registry_stream(&self, node_addr: String, stream: String) -> Result<()>;
    #[cfg(feature = "storage_operate")]
    async fn unregister_stream(&self, node_addr: String, stream: String) -> Result<()>;
    #[cfg(feature = "node_operate")]
    async fn nodes(&self) -> Result<Vec<Node>>;
    #[cfg(feature = "node_operate")]
    async fn stream_nodes(&self, stream: String) -> Result<Vec<Node>>;
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum StorageModel {
    RedisStandalone { addr: String },
}

pub async fn new(storage: StorageModel) -> Result<Box<dyn Storage + 'static + Send + Sync>> {
    match storage {
        StorageModel::RedisStandalone { addr } => {
            Ok(Box::new(RedisStandaloneStorage::new(addr).await?))
        }
    }
}
