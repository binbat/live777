use std::collections::HashSet;
use std::sync::Arc;

use super::Storage;
use crate::result::Result;
use async_trait::async_trait;
use chrono::Utc;
use redis::AsyncCommands;
use redis::Client;

use tokio::sync::RwLock;

const NODES_REGISTRY_KEY: &str = "live777:nodes";
const NODE_REGISTRY_KEY: &str = "live777:node";
const ROOM_REGISTRY_KEY: &str = "live777:room";

#[derive(Clone)]
pub struct RedisStandaloneStorage {
    ip_port: String,
    client: Client,
    rooms: Arc<RwLock<HashSet<String>>>,
}

impl RedisStandaloneStorage {
    pub async fn new(node_ip_port: String, addr: String) -> Self {
        let storage = RedisStandaloneStorage {
            ip_port: node_ip_port,
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
        // let storage_copy = storage.clone();
        // tokio::spawn(async move {
        //     storage_copy.room_heartbeat().await;
        // });
        storage
    }

    // async fn room_heartbeat(&self) {
    //     loop {
    //         let timeout = tokio::time::sleep(Duration::from_millis(1000));
    //         tokio::pin!(timeout);
    //         let _ = timeout.as_mut().await;
    //         if let Ok(mut conn) = self.client.get_multiplexed_async_connection().await {
    //             let rooms = self.rooms.read().await;
    //             for room in rooms.iter() {
    //                 let _ = conn
    //                     .set_options::<String, String, String>(
    //                         format!("{}:{}", ROOM_REGISTRY_KEY, room),
    //                         self.node_ip_port.clone(),
    //                         SetOptions::default()
    //                             .conditional_set(redis::ExistenceCheck::XX)
    //                             .with_expiration(redis::SetExpiry::EX(3)),
    //                     )
    //                     .await;
    //             }
    //         }
    //     }
    // }
}

#[async_trait]
impl Storage for RedisStandaloneStorage {
    async fn registry(&self, value: String) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.set_ex(format!("{}:{}", NODE_REGISTRY_KEY, self.ip_port), value, 3)
            .await?;
        conn.sadd(NODES_REGISTRY_KEY, self.ip_port.clone()).await?;
        Ok(())
    }

    async fn registry_room(&self, room: String) -> Result<()> {
        self.rooms.write().await.insert(room.clone());
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.zadd(
            format!("{}:{}", ROOM_REGISTRY_KEY, room),
            self.ip_port.clone(),
            Utc::now().timestamp_millis(),
        )
        .await?;
        self.rooms.write().await.insert(room.clone());
        Ok(())
    }
    async fn unregister_room(&self, room: String) -> Result<()> {
        self.rooms.write().await.remove(&room);
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.zrem(
            format!("{}:{}", ROOM_REGISTRY_KEY, room),
            self.ip_port.clone(),
        )
        .await?;
        Ok(())
    }
}
