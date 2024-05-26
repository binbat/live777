use anyhow::{anyhow, Error, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use live777_http::response::StreamInfo;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

const SYNC_API: &str = "/admin/infos";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Clone)]
pub struct EmbedStorage {
    time: SystemTime,
    server: Arc<RwLock<HashMap<String, Server>>>,
    client: reqwest::Client,
    info: Arc<RwLock<HashMap<String, Vec<StreamInfo>>>>,
    stream: Arc<RwLock<HashMap<String, Server>>>,
    resource: Arc<RwLock<HashMap<String, Server>>>,
    servers: Vec<Server>,
}

impl EmbedStorage {
    pub fn new(_addr: String, servers: Vec<Server>) -> Self {
        let server = Arc::new(RwLock::new(HashMap::new()));

        for s in servers.clone() {
            server.write().unwrap().insert(s.key.clone(), s.clone());
        }

        Self {
            server,
            time: SystemTime::now(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_millis(500))
                .timeout(Duration::from_millis(1000))
                .build().unwrap(),
            servers,
            info: Arc::new(RwLock::new(HashMap::new())),
            stream: Arc::new(RwLock::new(HashMap::new())),
            resource: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn nodes(&mut self) -> Vec<Server> {
        self.update().await;
        self.servers.clone()
    }

    pub async fn info_put(&self, key: String, target: Vec<StreamInfo>) -> Result<()> {
        self.info.write().unwrap().insert(key, target);
        Ok(())
    }

    pub async fn _info_get(&mut self, key: String) -> Result<Vec<StreamInfo>, Error> {
        self.update().await;
        match self.info.read().unwrap().get(&key) {
            Some(server) => Ok(server.clone()),
            None => Err(anyhow!("stream not found")),
        }
    }

    pub async fn info_all(&mut self) -> Result<Vec<StreamInfo>, Error> {
        self.update().await;
        let mut result = Vec::new();
        for mut v in self.info.read().unwrap().values().cloned() {
            result.append(&mut v);
        };
        Ok(result)
    }

    pub async fn stream_put(&self, stream: String, target: Server) -> Result<()> {
        self.stream.write().unwrap().insert(stream, target);
        Ok(())
    }

    pub async fn stream_get(&mut self, stream: String) -> Result<Server, Error> {
        self.update().await;
        match self.stream.read().unwrap().get(&stream) {
            Some(server) => Ok(server.clone()),
            None => Err(anyhow!("stream not found")),
        }
    }

    pub async fn resource_put(&self, resource: String, target: Server) -> Result<()> {
        self.resource.write().unwrap().insert(resource, target);
        Ok(())
    }

    pub async fn resource_get(&mut self, resource: String) -> Result<Server, Error> {
        self.update().await;
        match self.resource.read().unwrap().get(&resource) {
            Some(data) => Ok(data.clone()),
            None => Err(anyhow!("resource not found")),
        }
    }

    async fn update(&mut self) {
        if self.time.elapsed().unwrap() < Duration::from_secs(3) {
            return
        }
        self.time = SystemTime::now();

        let start = Instant::now();
        let servers = self.servers.clone();
        let mut requests = Vec::new();

        for server in servers {
            requests.push((server.key, self.client.get(format!("{}{}", server.url, SYNC_API)).send()));
        }

        let handles = requests.into_iter().map(|(key, value)| {
            tokio::spawn(async move {
                (key, value.await)
            })
        }).collect::<Vec<tokio::task::JoinHandle<(std::string::String, std::result::Result<reqwest::Response, reqwest::Error>)>>>();

        let duration = start.elapsed();

        if duration > Duration::from_secs(1) {
            warn!("update duration: {:?}", duration);
        } else {
            info!("update duration: {:?}", duration);
        }

        self.info.write().unwrap().clear();
        self.stream.write().unwrap().clear();
        //self.resource.write().unwrap().clear();

        for handle in handles {
            let result = tokio::join!(handle);
            match result {
                (Ok((key , Ok(res))),) => {
                    debug!("{}: Response: {:?}", key, res);

                    match serde_json::from_str::<Vec<StreamInfo>>(&res.text().await.unwrap()) {
                        Ok(streams) => {
                            info!("{:?}", streams.clone());
                            self.info_put(key.clone(), streams.clone()).await.unwrap();
                            for stream in streams {
                                let target = self.server.read().unwrap().get(&key).unwrap().clone();
                                self.stream_put(
                                    stream.id,
                                    target,
                                ).await.unwrap();

                                // TODO: resource
                            };
                        },
                        Err(e) => error!("Error: {:?}", e),
                    };
                },
                (Ok((name, Err(e))),) => {
                    error!("{}: Error: {:?}", name, e);
                },
                _ => {},
            }
        }
    }

}
