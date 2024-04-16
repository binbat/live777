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
const STREAM_REGISTRY_KEY: &str = "live777:stream";

#[derive(Clone)]
pub struct RedisStandaloneStorage {
    ip_port: String,
    client: Client,
    streams: Arc<RwLock<HashSet<String>>>,
}

impl RedisStandaloneStorage {
    pub async fn new(node_ip_port: String, addr: String) -> Self {
        let storage = RedisStandaloneStorage {
            ip_port: node_ip_port,
            client: Client::open(addr.clone()).unwrap(),
            streams: Default::default(),
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
        //     storage_copy.stream_heartbeat().await;
        // });
        storage
    }

    // async fn stream_heartbeat(&self) {
    //     loop {
    //         let timeout = tokio::time::sleep(Duration::from_millis(1000));
    //         tokio::pin!(timeout);
    //         let _ = timeout.as_mut().await;
    //         if let Ok(mut conn) = self.client.get_multiplexed_async_connection().await {
    //             let streams = self.streams.read().await;
    //             for stream in streams.iter() {
    //                 let _ = conn
    //                     .set_options::<String, String, String>(
    //                         format!("{}:{}", stream_REGISTRY_KEY, stream),
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

    async fn registry_stream(&self, stream: String) -> Result<()> {
        self.streams.write().await.insert(stream.clone());
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.zadd(
            format!("{}:{}", STREAM_REGISTRY_KEY, stream),
            self.ip_port.clone(),
            Utc::now().timestamp_millis(),
        )
        .await?;
        self.streams.write().await.insert(stream.clone());
        Ok(())
    }
    async fn unregister_stream(&self, stream: String) -> Result<()> {
        self.streams.write().await.remove(&stream);
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.zrem(
            format!("{}:{}", STREAM_REGISTRY_KEY, stream),
            self.ip_port.clone(),
        )
        .await?;
        Ok(())
    }
}
