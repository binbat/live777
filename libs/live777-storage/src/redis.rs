use crate::NodeMetaData;

use super::Storage;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use std::net::SocketAddr;

use redis::AsyncCommands;
use redis::Client;

#[cfg(feature = "node_operate")]
use crate::Node;
#[cfg(feature = "node_operate")]
use redis::aio::MultiplexedConnection;
#[cfg(feature = "node_operate")]
use redis::RedisError;
#[cfg(feature = "node_operate")]
use std::str::FromStr;
#[cfg(feature = "node_operate")]
use std::vec;

const NODES_REGISTRY_KEY: &str = "live777:nodes";
const NODE_REGISTRY_KEY: &str = "live777:node";
const STREAM_REGISTRY_KEY: &str = "live777:stream";

#[derive(Clone)]
pub struct RedisStandaloneStorage {
    client: Client,
}

impl RedisStandaloneStorage {
    pub async fn new(addr: String) -> Result<Self> {
        let storage = RedisStandaloneStorage {
            client: Client::open(addr.to_string())?,
        };
        // check conn
        let mut conn = storage.client.get_multiplexed_async_connection().await?;
        let _ = conn.get::<&str, Option<String>>("hello world").await?;
        Ok(storage)
    }
}

#[async_trait]
impl Storage for RedisStandaloneStorage {
    #[cfg(feature = "storage_operate")]
    async fn registry(&self, addr: SocketAddr, metadata: NodeMetaData) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.sadd(NODES_REGISTRY_KEY, addr.to_string()).await?;
        conn.set_ex(
            format!("{}:{}", NODE_REGISTRY_KEY, addr),
            serde_json::to_string(&metadata)?,
            3,
        )
        .await?;
        Ok(())
    }
    #[cfg(feature = "storage_operate")]
    async fn registry_stream(&self, addr: SocketAddr, stream: String) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.zadd(
            format!("{}:{}", STREAM_REGISTRY_KEY, stream),
            addr.to_string(),
            Utc::now().timestamp_millis(),
        )
        .await?;
        Ok(())
    }
    #[cfg(feature = "storage_operate")]
    async fn unregister_stream(&self, addr: SocketAddr, stream: String) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        conn.zrem(
            format!("{}:{}", STREAM_REGISTRY_KEY, stream),
            addr.to_string(),
        )
        .await?;
        Ok(())
    }
    #[cfg(feature = "node_operate")]
    async fn nodes(&self) -> Result<Vec<Node>> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let nodes_host: Vec<String> = conn.smembers(NODES_REGISTRY_KEY).await?;
        let (nodes, remove_nodes) = Self::final_nodes(nodes_host, &mut conn).await?;
        if !remove_nodes.is_empty() {
            let _ = conn
                .srem::<&str, Vec<std::string::String>, i64>(NODES_REGISTRY_KEY, remove_nodes)
                .await;
        }
        Ok(nodes)
    }
    #[cfg(feature = "node_operate")]
    async fn stream_nodes(&self, stream: String) -> Result<Vec<Node>> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let stream_nodes: Vec<String> = conn
            .zrange(format!("{}:{}", STREAM_REGISTRY_KEY, stream), 0, -1)
            .await?;
        let (nodes, mut remove_nodes) = Self::final_nodes(stream_nodes, &mut conn).await?;
        if !remove_nodes.is_empty() {
            let _ = conn
                .srem::<&str, Vec<std::string::String>, i64>(
                    NODES_REGISTRY_KEY,
                    remove_nodes.clone(),
                )
                .await;
        }
        let mut final_nodes = vec![];
        for node in nodes.into_iter() {
            let node_stream_info = node.stream_info(stream.clone()).await;
            let ok = node_stream_info.is_ok();
            let some = ok && node_stream_info.unwrap().is_some();
            if some {
                final_nodes.push(node);
            } else if ok {
                remove_nodes.push(node.addr.to_string());
            }
        }
        if !remove_nodes.is_empty() {
            let _: Result<u64, RedisError> = conn
                .zrem(format!("{}:{}", STREAM_REGISTRY_KEY, stream), remove_nodes)
                .await;
        }
        Ok(final_nodes)
    }
}

#[cfg(feature = "node_operate")]
impl RedisStandaloneStorage {
    async fn final_nodes(
        node_addrs: Vec<String>,
        conn: &mut MultiplexedConnection,
    ) -> Result<(Vec<Node>, Vec<String>)> {
        let mut nodes = vec![];
        let mut remove_nodes = vec![];
        if node_addrs.is_empty() {
            return Ok((nodes, remove_nodes));
        }
        let nodes_mget: Vec<Option<String>> = conn
            .mget(
                node_addrs
                    .iter()
                    .map(|addr| format!("{}:{}", NODE_REGISTRY_KEY, addr))
                    .collect::<Vec<String>>(),
            )
            .await?;
        for i in 0..node_addrs.len() {
            let addr = node_addrs.get(i).unwrap();
            let metadata = nodes_mget.get(i).unwrap();
            if metadata.is_none() {
                remove_nodes.push(addr.clone());
                continue;
            }
            if let Ok(addr) = SocketAddr::from_str(addr) {
                let metadata = metadata.clone().unwrap();
                if let Ok(metadata) = serde_json::from_str(&metadata) {
                    nodes.push(Node { addr, metadata })
                }
            } else {
                remove_nodes.push(addr.clone());
            }
        }
        Ok((nodes, remove_nodes))
    }
}
