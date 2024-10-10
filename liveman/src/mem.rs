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
use api::strategy::Strategy;

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

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub token: String,
    pub kind: NodeKind,
    pub url: String,

    streams: Vec<Stream>,
    strategy: Option<Strategy>,
}

impl Node {
    pub fn new(token: String, kind: NodeKind, url: String) -> Self {
        Self {
            token,
            kind,
            url,
            streams: Vec::new(),
            strategy: None,
        }
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    #[default]
    #[serde(rename = "build-in")]
    BuildIn,
    #[serde(rename = "static")]
    Static,
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "net4mqtt")]
    Net4mqtt,
}

impl From<Server> for (String, Node) {
    fn from(s: Server) -> Self {
        (
            s.alias,
            Node {
                token: s.token,
                url: s.url,
                ..Default::default()
            },
        )
    }
}

impl From<(String, Node)> for Server {
    fn from(r: (String, Node)) -> Self {
        let (k, v) = r;
        Self {
            alias: k,
            token: v.token,
            url: v.url,
            sub_max: match v.strategy {
                Some(x) => x.each_stream_max_sub.0,
                None => u16::MAX,
            },
            ..Default::default()
        }
    }
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
    list: Arc<RwLock<HashMap<String, Node>>>,
    time: SystemTime,
    client: reqwest::Client,

    stream: Arc<RwLock<HashMap<String, Vec<Server>>>>,
    session: Arc<RwLock<HashMap<String, Server>>>,
}

impl MemStorage {
    pub fn new(proxy: Option<(SocketAddr, String)>) -> Self {
        info!("Proxy is: {:?}", proxy);
        let mut client_builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_millis(1000));

        client_builder = if let Some((addr, domain)) = proxy {
            // References: https://github.com/seanmonstar/reqwest/issues/899
            let target = reqwest::Url::parse(format!("socks5h://{}", addr).as_str()).unwrap();
            client_builder.proxy(reqwest::Proxy::custom(move |url| match url.host_str() {
                Some(host) => {
                    if host.ends_with(domain.as_str()) {
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
            list: Arc::new(RwLock::new(HashMap::new())),
            time: SystemTime::now(),
            client: client_builder.build().unwrap(),

            stream: Arc::new(RwLock::new(HashMap::new())),
            session: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get_map_nodes_mut(&self) -> Arc<RwLock<HashMap<String, Node>>> {
        self.list.clone()
    }

    pub fn get_map_nodes(&self) -> HashMap<String, Node> {
        self.list.read().unwrap().clone()
    }

    pub fn get_cluster(&self) -> Vec<Server> {
        self.list
            .read()
            .unwrap()
            .clone()
            .into_iter()
            .map(|x| x.into())
            .collect()
    }

    pub fn get_map_server(&self) -> HashMap<String, Server> {
        self.list
            .read()
            .unwrap()
            .clone()
            .into_iter()
            .map(|(k, v)| (k.clone(), (k, v).into()))
            .collect()
    }

    pub async fn nodes(&mut self) -> Vec<Server> {
        self.update().await;
        self.get_cluster()
    }

    pub async fn info_put(&self, alias: String, target: Vec<Stream>) -> Result<(), Error> {
        match self.list.write().unwrap().get_mut(&alias) {
            Some(node) => node.streams = target,
            None => return Err(anyhow!("node not found")),
        };
        Ok(())
    }

    pub async fn info_get(&mut self, alias: String) -> Result<Vec<Stream>, Error> {
        self.update().await;
        match self.list.read().unwrap().get(&alias) {
            Some(node) => Ok(node.streams.clone()),
            None => Err(anyhow!("node not found")),
        }
    }

    pub async fn info_raw_all(&mut self) -> Result<HashMap<String, Vec<Stream>>, Error> {
        self.update().await;
        Ok(self
            .list
            .read()
            .unwrap()
            .clone()
            .into_iter()
            .map(|(k, v)| (k.clone(), v.streams.clone()))
            .collect())
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

    fn get_do_strategy_updata_list(&self) -> HashMap<String, Node> {
        self.get_map_nodes()
            .into_iter()
            .filter(|(_, v)| v.strategy.is_none())
            .collect()
    }

    async fn update_strategy_from(&mut self, nodes: HashMap<String, Node>) {
        let start = Instant::now();
        let mut requests = Vec::new();

        for (alias, server) in nodes {
            requests.push((
                alias,
                self.client
                    .get(format!("{}{}", server.url, &api::path::strategy()))
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

        self.stream.write().unwrap().clear();

        // Maybe Don't need clear "session"
        //self.session.write().unwrap().clear();

        for handle in handles {
            let result = tokio::join!(handle);
            match result {
                (Ok((alias, Ok(res))),) => {
                    debug!("{}: Response: {:?}", alias, res);

                    match serde_json::from_str::<Strategy>(&res.text().await.unwrap()) {
                        Ok(strategy) => {
                            if let Some(node) =
                                self.get_map_nodes_mut().write().unwrap().get_mut(&alias)
                            {
                                node.strategy = Some(strategy)
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

    async fn update(&mut self) {
        if self.time.elapsed().unwrap() < Duration::from_secs(3) {
            return;
        }
        self.time = SystemTime::now();

        self.update_strategy_from(self.get_do_strategy_updata_list())
            .await;

        let start = Instant::now();
        let servers = self.get_cluster();
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
                                let target = self.get_map_server().get(&alias).unwrap().clone();
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
