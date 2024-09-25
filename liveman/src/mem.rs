use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Error, Result};
use http::header;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use tracing::{debug, error, info, trace, warn};

use api::response::Stream;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Server {
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "u16_max_value")]
    pub pub_max: u16,
    #[serde(default = "u16_max_value")]
    pub sub_max: u16,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            alias: String::default(),
            token: String::default(),
            url: String::default(),
            pub_max: u16::MAX,
            sub_max: u16::MAX,
        }
    }
}

impl Hash for Server {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.alias.hash(state);
    }
}

fn u16_max_value() -> u16 {
    u16::MAX
}

#[derive(Clone)]
pub struct MemStorage {
    time: SystemTime,
    server: Arc<RwLock<HashMap<String, Server>>>,
    client: reqwest::Client,
    info: Arc<RwLock<HashMap<String, Vec<Stream>>>>,
    stream: Arc<RwLock<HashMap<String, Vec<Server>>>>,
    session: Arc<RwLock<HashMap<String, Server>>>,
    servers: Vec<Server>,
}

impl MemStorage {
    pub fn new(servers: Vec<Server>, proxy: Option<SocketAddr>) -> Self {
        let server = Arc::new(RwLock::new(HashMap::new()));

        info!("MemStorage: {:?}", servers);

        for s in servers.clone() {
            server.write().unwrap().insert(s.alias.clone(), s.clone());
        }
        let mut client_builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_millis(1000));

        client_builder = if let Some(addr) = proxy {
            let target = reqwest::Url::parse(format!("socks5h://{}", addr).as_str()).unwrap();
            let suffix = "net4mqtt.local";
            client_builder.proxy(reqwest::Proxy::custom(move |url| match url.host_str() {
                Some(host) => {
                    if host.ends_with(suffix) {
                        Some(target.clone())
                    } else {
                        None
                    }
                }
                None => None,
            }))
        } else {
            client_builder
        };

        Self {
            server,
            time: SystemTime::now(),
            client: client_builder.build().unwrap(),
            servers,
            info: Arc::new(RwLock::new(HashMap::new())),
            stream: Arc::new(RwLock::new(HashMap::new())),
            session: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get_cluster(&self) -> Vec<Server> {
        self.servers.clone()
    }

    pub fn get_map_server(&self) -> HashMap<String, Server> {
        self.server.read().unwrap().clone()
    }

    pub async fn nodes(&mut self) -> Vec<Server> {
        self.update().await;
        self.servers.clone()
    }

    pub async fn info_put(&self, alias: String, target: Vec<Stream>) -> Result<()> {
        self.info.write().unwrap().insert(alias, target);
        Ok(())
    }

    pub async fn info_get(&mut self, alias: String) -> Result<Vec<Stream>, Error> {
        self.update().await;
        match self.info.read().unwrap().get(&alias) {
            Some(server) => Ok(server.clone()),
            None => Err(anyhow!("stream not found")),
        }
    }

    pub async fn info_raw_all(&mut self) -> Result<HashMap<String, Vec<Stream>>, Error> {
        self.update().await;
        Ok(self.info.read().unwrap().clone())
    }

    pub async fn stream_put(&self, stream: String, target: Server) -> Result<()> {
        {
            let mut ctx = self.stream.write().unwrap();
            let mut arr = ctx.get(&stream).unwrap_or(&Vec::new()).clone();
            arr.push(target);
            ctx.insert(stream, arr);
        }
        Ok(())
    }

    pub async fn stream_get(&mut self, stream: String) -> Result<Vec<Server>, Error> {
        self.update().await;
        match self.stream.read().unwrap().get(&stream) {
            Some(server) => Ok(server.clone()),
            None => Ok(Vec::new()),
        }
    }

    pub async fn stream_all(&mut self) -> HashMap<String, Vec<Server>> {
        self.update().await;
        self.stream.read().unwrap().clone()
    }

    pub async fn session_put(&self, session: String, target: Server) -> Result<()> {
        self.session.write().unwrap().insert(session, target);
        Ok(())
    }

    pub async fn session_get(&mut self, session: String) -> Result<Server, Error> {
        self.update().await;
        match self.session.read().unwrap().get(&session) {
            Some(data) => Ok(data.clone()),
            None => Err(anyhow!("session not found")),
        }
    }

    async fn update(&mut self) {
        if self.time.elapsed().unwrap() < Duration::from_secs(3) {
            return;
        }
        self.time = SystemTime::now();

        let start = Instant::now();
        let servers = self.servers.clone();
        let mut requests = Vec::new();

        for server in servers {
            requests.push((
                server.alias,
                self.client
                    .get(format!("{}{}", server.url, &api::path::streams("")))
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .send(),
            ));
        }

        let handles = requests
            .into_iter()
            .map(|(alias, value)| tokio::spawn(async move { (alias, value.await) }))
            .collect::<Vec<
                tokio::task::JoinHandle<(
                    std::string::String,
                    std::result::Result<reqwest::Response, reqwest::Error>,
                )>,
            >>();

        let duration = start.elapsed();

        if duration > Duration::from_secs(1) {
            warn!("update duration: {:?}", duration);
        } else {
            debug!("update duration: {:?}", duration);
        }

        self.info.write().unwrap().clear();
        self.stream.write().unwrap().clear();

        // Maybe Don't need clear "session"
        //self.session.write().unwrap().clear();

        for handle in handles {
            let result = tokio::join!(handle);
            match result {
                (Ok((alias, Ok(res))),) => {
                    debug!("{}: Response: {:?}", alias, res);

                    match serde_json::from_str::<Vec<Stream>>(&res.text().await.unwrap()) {
                        Ok(streams) => {
                            trace!("{:?}", streams.clone());
                            self.info_put(alias.clone(), streams.clone()).await.unwrap();
                            for stream in streams {
                                let target =
                                    self.server.read().unwrap().get(&alias).unwrap().clone();
                                self.stream_put(stream.id.clone(), target.clone())
                                    .await
                                    .unwrap();

                                for session in stream.subscribe.sessions {
                                    match self
                                        .session_put(
                                            api::path::session(&stream.id, &session.id),
                                            target.clone(),
                                        )
                                        .await
                                    {
                                        Ok(_) => {}
                                        Err(e) => error!("Put Session Error: {:?}", e),
                                    }
                                }
                            }
                        }
                        Err(e) => error!("Error: {:?}", e),
                    };
                }
                (Ok((name, Err(e))),) => {
                    error!("{}: Error: {:?}", name, e);
                }
                _ => {}
            }
        }
    }
}
