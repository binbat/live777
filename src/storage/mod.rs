pub mod redis;
use crate::result::Result;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use self::redis::RedisStandaloneStorage;
#[async_trait]
pub trait Storage {
    async fn registry(&self, value: String) -> Result<()>;
    async fn registry_stream(&self, stream: String) -> Result<()>;
    async fn unregister_stream(&self, stream: String) -> Result<()>;
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "model")]
pub enum StorageModel {
    RedisStandalone { addr: String },
}

pub async fn new(
    node_ip_port: String,
    storage: StorageModel,
) -> Box<dyn Storage + 'static + Send + Sync> {
    match storage {
        StorageModel::RedisStandalone { addr } => {
            Box::new(RedisStandaloneStorage::new(node_ip_port, addr).await)
        }
    }
}
