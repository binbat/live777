use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use super::ClusterStorage;
use super::RoomOwnership;
use crate::result::Result;
use async_trait::async_trait;
use redis::AsyncCommands;
use redis::Client;
use redis::SetOptions;
use tokio::sync::RwLock;

const NODES_REGISTRY_KEY: &str = "live777:nodes";
const NODE_REGISTRY_KEY: &str = "live777:node";
const ROOM_REGISTRY_KEY: &str = "live777:room";

#[derive(Clone)]
pub struct RedisStandaloneStorage {
    node_ip_port: String,
    client: Client,
    rooms: Arc<RwLock<HashSet<String>>>,
}

impl RedisStandaloneStorage {
    pub async fn new(node_ip_port: String, addr: String) -> Self {
        let storage = RedisStandaloneStorage {
            node_ip_port,
            client: Client::open(addr.clone()).unwrap(),
            rooms: Default::default(),
        };
        // check conn
        let mut conn = storage
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| format!("get conn error : {:?} , redis addr : {}", e, addr.clone()))
            .unwrap();
        let _ = conn
            .get::<&str, Option<String>>("hello world")
            .await
            .map_err(|e| {
                format!(
                    "conn command error : {:?} , redis addr : {}",
                    e,
                    addr.clone()
                )
            })
            .unwrap();
        let storage_copy = storage.clone();
        tokio::spawn(async move {
            storage_copy.room_heartbeat().await;
        });
        storage
    }

    async fn room_heartbeat(&self) {
        loop {
            let timeout = tokio::time::sleep(Duration::from_millis(1000));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
            if let Ok(mut conn) = self.client.get_multiplexed_async_connection().await {
                let rooms = self.rooms.read().await;
                for room in rooms.iter() {
                    let _ = conn
                        .set_options::<String, String, String>(
                            format!("{}:{}", ROOM_REGISTRY_KEY, room),
                            self.node_ip_port.clone(),
                            SetOptions::default()
                                .conditional_set(redis::ExistenceCheck::XX)
                                .with_expiration(redis::SetExpiry::EX(3)),
                        )
                        .await;
                }
            }
        }
    }
}

#[async_trait]
impl ClusterStorage for RedisStandaloneStorage {
    async fn registry(&self, value: String) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.set_ex(
            format!("{}:{}", NODE_REGISTRY_KEY, self.node_ip_port),
            value,
            3,
        )
        .await?;
        conn.sadd(NODES_REGISTRY_KEY, self.node_ip_port.clone())
            .await?;
        Ok(())
    }
    async fn room_ownership(&self, room: String) -> Result<RoomOwnership> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let result = conn
            .get::<String, Option<String>>(format!("{}:{}", ROOM_REGISTRY_KEY, room))
            .await?;
        Ok(match result {
            Some(value) => {
                if value == self.node_ip_port {
                    RoomOwnership::MY
                } else {
                    self.rooms.write().await.remove(&room);
                    RoomOwnership::Other(value)
                }
            }
            None => RoomOwnership::None,
        })
    }
    async fn registry_room(&self, room: String) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.set_options(
            format!("{}:{}", ROOM_REGISTRY_KEY, room),
            self.node_ip_port.clone(),
            SetOptions::default()
                .conditional_set(redis::ExistenceCheck::NX)
                .with_expiration(redis::SetExpiry::EX(3)),
        )
        .await?;
        self.rooms.write().await.insert(room.clone());
        Ok(())
    }
    async fn unregister_room(&self, room: String) -> Result<()> {
        self.rooms.write().await.remove(&room);
        // TODO lua script
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.del(format!("{}:{}", ROOM_REGISTRY_KEY, room)).await?;
        Ok(())
    }
}
