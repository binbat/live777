use async_trait::async_trait;
use crate::storage_redis::Node;
use std::sync::Arc;
use tokio::sync::Mutex;
use rand::prelude::*;


#[async_trait]
#[async_trait]
pub trait LoadBalancing: Send + Sync {
    async fn next(&self) -> Result<Arc<Node>, Box<dyn std::error::Error>>;
}

pub struct RandomLoadBalancing {
    nodes: Arc<Mutex<Vec<Arc<Node>>>>,
}

impl RandomLoadBalancing {
    pub fn new(nodes: Vec<Arc<Node>>) -> Self {
        RandomLoadBalancing {
            nodes: Arc::new(Mutex::new(nodes)),
        }
    }
}

#[async_trait]
impl LoadBalancing for RandomLoadBalancing {
    async fn next(&self) -> Result<Arc<Node>, Box<dyn std::error::Error>> {
        let nodes = self.nodes.lock().await;
        if nodes.is_empty() {
            return Err("No nodes available for load balancing".into());
        }
        let mut rng = rand::thread_rng();
        let index = rng.gen_range(0..nodes.len());
        Ok(nodes[index].clone())
    }
}


pub struct RoundRobinLoadBalancing {
    nodes: Arc<Mutex<Vec<Arc<Node>>>>,
    current: Arc<Mutex<usize>>,
}

impl RoundRobinLoadBalancing {
    pub fn new(nodes: Vec<Arc<Node>>) -> Self {
        RoundRobinLoadBalancing {
            nodes: Arc::new(Mutex::new(nodes)),
            current: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl LoadBalancing for RoundRobinLoadBalancing {
    async fn next(&self) -> Result<Arc<Node>, Box<dyn std::error::Error>> {
        let mut current = self.current.lock().await;
        let nodes = self.nodes.lock().await;
        if nodes.is_empty() {
            return Err("No nodes available for load balancing".into());
        }
        let node = nodes[*current].clone();
        *current = (*current + 1) % nodes.len();
        Ok(node)
    }
}