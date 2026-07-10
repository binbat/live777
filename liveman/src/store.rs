use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Error, Result, anyhow};
use http::header;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};

use api::response::Stream;
use api::strategy::Strategy;

use crate::config::UpdateMode;

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

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub token: String,
    pub kind: NodeKind,
    pub url: String,
    pub mode: UpdateMode,

    streams: Vec<Stream>,
    pub strategy: Option<Strategy>,
    pub duration: Option<Duration>,
}

impl Node {
    pub fn new(token: String, kind: NodeKind, url: String, mode: UpdateMode) -> Self {
        Self {
            token,
            kind,
            url,
            mode,
            ..Default::default()
        }
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    #[default]
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
pub struct Storage {
    list: Arc<RwLock<HashMap<String, Node>>>,
    time: Instant,
    client: reqwest::Client,
    stream: Arc<RwLock<HashMap<String, Vec<String>>>>,
    session: Arc<RwLock<HashMap<String, String>>>,
    update_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Storage {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            list: Arc::new(RwLock::new(HashMap::new())),
            time: Instant::now(),
            client,
            stream: Arc::new(RwLock::new(HashMap::new())),
            session: Arc::new(RwLock::new(HashMap::new())),
            update_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn get_map_nodes_mut(&self) -> Arc<RwLock<HashMap<String, Node>>> {
        self.list.clone()
    }

    pub fn get_map_nodes(&self) -> HashMap<String, Node> {
        //self.list.read().unwrap_or_default().clone()
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

    pub async fn update_snapshot(&self, alias: &str, streams: Vec<Stream>) -> Result<()> {
        let _guard = self.update_lock.lock().await;
        Self::apply_snapshot_body(&self.list, &self.session, &self.stream, alias, streams)
    }

    fn apply_snapshot_body(
        list: &Arc<RwLock<HashMap<String, Node>>>,
        session: &Arc<RwLock<HashMap<String, String>>>,
        stream: &Arc<RwLock<HashMap<String, Vec<String>>>>,
        alias: &str,
        streams: Vec<Stream>,
    ) -> Result<()> {
        // Hold all three indexes at once with a consistent lock order
        // (list -> session -> stream) so the snapshot is applied atomically
        // relative to other snapshot updates. These are std::sync::RwLock guards
        // held for short in-memory operations only; they never cross an await.
        let mut list = list.write().map_err(|e| anyhow!("{:?}", e))?;
        let mut session_map = session.write().map_err(|e| anyhow!("{:?}", e))?;
        let mut stream_map = stream.write().map_err(|e| anyhow!("{:?}", e))?;

        // Remove stale alias references contributed by this node.
        for aliases in stream_map.values_mut() {
            aliases.retain(|a| a != alias);
        }
        stream_map.retain(|_, aliases| !aliases.is_empty());

        // Remove stale session entries contributed by this node.
        let old_streams = list
            .get(alias)
            .map(|node| node.streams.clone())
            .unwrap_or_default();
        for stream in old_streams {
            for session in stream.subscribe.sessions {
                let key = api::path::session(&stream.id, &session.id);
                if let Some(existing_alias) = session_map.get(&key)
                    && existing_alias == alias
                {
                    session_map.remove(&key);
                }
            }
        }

        // Update the node's stream list.
        let node = list
            .get_mut(alias)
            .ok_or_else(|| anyhow!("node not found"))?;
        node.streams = streams.clone();

        // Rebuild stream/session indexes for the new snapshot.
        for stream in streams {
            stream_map
                .entry(stream.id.clone())
                .or_default()
                .push(alias.to_string());
            for session in stream.subscribe.sessions {
                session_map.insert(
                    api::path::session(&stream.id, &session.id),
                    alias.to_string(),
                );
            }
        }

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

    // Serialize with snapshot updates so proxy writes are not interleaved
    // with apply_snapshot_body rebuilding the stream/session indexes.
    pub async fn stream_put(&self, stream: String, alias: String) -> Result<()> {
        let _guard = self.update_lock.lock().await;
        let mut ctx = self.stream.write().map_err(|e| anyhow!("{:?}", e))?;
        let mut arr = ctx.get(&stream).cloned().unwrap_or(Vec::new());
        if !arr.contains(&alias) {
            arr.push(alias);
        }
        ctx.insert(stream, arr);
        Ok(())
    }

    pub async fn stream_get(&mut self, stream: String) -> Result<Vec<Server>, Error> {
        self.update().await;

        let streams = self
            .stream
            .read()
            .map_err(|e| anyhow!("{:?}", e))?
            .get(&stream)
            .cloned()
            .unwrap_or(vec![]);

        let nodes = self.get_map_nodes();

        let mut result: Vec<Server> = vec![];
        for alias in streams {
            if let Some(n) = nodes.get(&alias) {
                result.push((alias, n.clone()).into());
            }
        }
        Ok(result)
    }

    pub async fn stream_all(&mut self) -> HashMap<String, Vec<String>> {
        self.update().await;
        self.stream.read().unwrap().clone()
    }

    // Serialize with snapshot updates so proxy writes are not interleaved
    // with apply_snapshot_body rebuilding the stream/session indexes.
    pub async fn session_put(&self, session: String, alias: String) -> Result<()> {
        let _guard = self.update_lock.lock().await;
        self.session
            .write()
            .map_err(|e| anyhow!("{:?}", e))?
            .insert(session, alias);
        Ok(())
    }

    pub async fn session_get(&mut self, session: String) -> Result<Server> {
        self.update().await;
        let alias = self
            .session
            .read()
            .map_err(|e| anyhow!("{:?}", e))?
            .get(&session)
            .ok_or(anyhow!("session not found"))?
            .clone();

        let node = self
            .list
            .read()
            .map_err(|e| anyhow!("{:?}", e))?
            .get(&alias)
            .ok_or(anyhow!("node not found"))?
            .clone();

        Ok((alias, node).into())
    }

    fn get_do_strategy_update_list(&self) -> HashMap<String, Node> {
        self.get_map_nodes()
            .into_iter()
            .filter(|(_, v)| v.kind != NodeKind::Net4mqtt && v.strategy.is_none())
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
            .map(|(alias, value)| {
                tokio::spawn(async move { (alias, value.await, start.elapsed()) })
            })
            .collect::<Vec<
                tokio::task::JoinHandle<(
                    std::string::String,
                    std::result::Result<reqwest::Response, reqwest::Error>,
                    std::time::Duration,
                )>,
            >>();

        let duration = start.elapsed();

        if duration > Duration::from_secs(1) {
            warn!("update duration: {:?}", duration);
        } else {
            debug!("update duration: {:?}", duration);
        }

        for handle in handles {
            let result = tokio::join!(handle);
            match result {
                (Ok((alias, Ok(res), duration)),) => {
                    debug!(
                        "{}: spend time: [{:?}] Response: {:?}",
                        alias, duration, res
                    );

                    match serde_json::from_str::<Strategy>(&res.text().await.unwrap()) {
                        Ok(strategy) => {
                            if let Some(node) =
                                self.get_map_nodes_mut().write().unwrap().get_mut(&alias)
                            {
                                node.duration = Some(duration);
                                node.strategy = Some(strategy);
                            }
                        }
                        Err(e) => error!("Error: {:?}", e),
                    };
                }
                (Ok((name, Err(e), duration)),) => {
                    error!("{}: spend time: [{:?}] Error: {:?}", name, duration, e);
                }
                _ => {}
            }
        }
    }

    async fn update(&mut self) {
        let update_lock = self.update_lock.clone();
        let _guard = update_lock.lock().await;
        if self.time.elapsed() < Duration::from_secs(3) {
            return;
        }
        self.time = Instant::now();

        self.update_strategy_from(self.get_do_strategy_update_list())
            .await;

        let start = Instant::now();
        let poll_nodes: Vec<(String, Node)> = self
            .get_map_nodes()
            .into_iter()
            .filter(|(_, node)| node.kind != NodeKind::Net4mqtt && node.mode == UpdateMode::Poll)
            .collect();
        let mut requests = Vec::new();

        for (alias, node) in poll_nodes {
            requests.push((
                alias,
                self.client
                    .get(format!("{}{}", node.url, &api::path::streams("")))
                    .header(header::AUTHORIZATION, format!("Bearer {}", node.token))
                    .send(),
            ));
        }

        let handles = requests
            .into_iter()
            .map(|(alias, value)| {
                tokio::spawn(async move { (alias, start.elapsed(), value.await) })
            })
            .collect::<Vec<
                tokio::task::JoinHandle<(
                    std::string::String,
                    std::time::Duration,
                    std::result::Result<reqwest::Response, reqwest::Error>,
                )>,
            >>();

        let duration = start.elapsed();

        if duration > Duration::from_secs(1) {
            warn!("update duration: {:?}", duration);
        } else {
            debug!("update duration: {:?}", duration);
        }

        for handle in handles {
            let result = tokio::join!(handle);
            match result {
                (Ok((alias, duration, Ok(res))),) => {
                    debug!(
                        "{}: spend time: [{:?}] Response: {:?}",
                        alias, duration, res
                    );

                    match serde_json::from_str::<Vec<Stream>>(&res.text().await.unwrap()) {
                        Ok(streams) => {
                            trace!("{:?}", streams.clone());
                            if let Err(e) = Self::apply_snapshot_body(
                                &self.list,
                                &self.session,
                                &self.stream,
                                &alias,
                                streams,
                            ) {
                                error!("{}: apply snapshot error: {:?}", alias, e);
                            }
                        }
                        Err(e) => error!("Error: {:?}", e),
                    };
                }
                (Ok((name, duration, Err(e))),) => {
                    error!("{}: spend time: [{:?}] Error: {:?}", name, duration, e);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_update_list_skips_net4mqtt_nodes() {
        let storage = Storage::new(reqwest::Client::new());
        {
            let mut nodes = storage.list.write().unwrap();
            nodes.insert(
                "static-0".to_string(),
                Node::new(
                    "token".to_string(),
                    NodeKind::Static,
                    "http://127.0.0.1:7777".to_string(),
                    UpdateMode::Poll,
                ),
            );
            nodes.insert(
                "mqtt-0".to_string(),
                Node::new(
                    "".to_string(),
                    NodeKind::Net4mqtt,
                    "http://mqtt-0.net4mqtt.local:7777".to_string(),
                    UpdateMode::default(),
                ),
            );
        }

        let list = storage.get_do_strategy_update_list();
        assert!(list.contains_key("static-0"));
        assert!(!list.contains_key("mqtt-0"));
    }

    #[test]
    fn strategy_update_list_skips_nodes_with_strategy() {
        let storage = Storage::new(reqwest::Client::new());
        {
            let mut nodes = storage.list.write().unwrap();
            let mut node = Node::new(
                "token".to_string(),
                NodeKind::Static,
                "http://127.0.0.1:7777".to_string(),
                UpdateMode::Poll,
            );
            node.strategy = Some(Strategy::default());
            nodes.insert("static-0".to_string(), node);
        }

        let list = storage.get_do_strategy_update_list();
        assert!(!list.contains_key("static-0"));
    }
}
