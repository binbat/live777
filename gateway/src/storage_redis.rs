use redis::{AsyncCommands, Client};
use serde::{Serialize, Deserialize};
use std::fmt;


#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Node {
    pub addr: String,
    pub metadata: String,
}


#[derive(Debug)]
struct StorageError(String);

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Storage error: {}", self.0)
    }
}

impl std::error::Error for StorageError {}

pub struct RedisStandaloneStorage {
    client: Client,
}

impl RedisStandaloneStorage {
    pub async fn new(addr: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let client = redis::Client::open(addr)?;
        let mut conn = client.get_multiplexed_async_connection().await?;
        let _: () = conn.set("test_connection_key", "connected").await?;
        println!("Successfully connected to Redis");
        Ok(RedisStandaloneStorage { client })
    }

    pub async fn get_all_node(&self) -> Result<Vec<Node>, Box<dyn std::error::Error>> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let nodes: Vec<String> = conn.sinter("live777:nodes").await?;
        let mut res_nodes = Vec::new();
        for node in nodes.iter() {
            let key = format!("live777:node:{}", node);
            match conn.get::<_, Option<String>>(&key).await? {
                Some(metadata) => res_nodes.push(Node { addr: node.clone(), metadata }),
                None => (),
            }
        }
        Ok(res_nodes)
    }

    pub async fn get_room_ownership(&self, room: &str) -> Result<Option<Node>, Box<dyn std::error::Error>> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let room_key = format!("live777:room:{}", room);
        if let Some(room_node) = conn.get::<_, Option<String>>(&room_key).await? {
            let node_key = format!("live777:node:{}", room_node);
            if let Some(metadata) = conn.get::<_, Option<String>>(&node_key).await? {
                return Ok(Some(Node { addr: room_node, metadata }));
            }
        }
        Ok(None)
    }
}
