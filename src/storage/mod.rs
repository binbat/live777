pub mod redis;
use crate::result::Result;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use self::redis::RedisStandaloneStorage;
#[async_trait]
pub trait ClusterStorage {
    async fn registry(&self, value: String) -> Result<()>;
    async fn room_ownership(&self, room: String) -> Result<RoomOwnership>;
    async fn registry_room(&self, room: String) -> Result<()>;
    async fn unregister_room(&self, room: String) -> Result<()>;
}

pub enum RoomOwnership {
    None,
    MY,
    Other(String),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "model")]
pub enum ClusterStorageModel {
    RedisStandalone { addr: String },
}

pub async fn new(
    node_ip_port: String,
    storage: ClusterStorageModel,
) -> Box<dyn ClusterStorage + 'static + Send + Sync> {
    match storage {
        ClusterStorageModel::RedisStandalone { addr } => {
            Box::new(RedisStandaloneStorage::new(node_ip_port, addr).await)
        }
    }
}
