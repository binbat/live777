use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Error, Result, anyhow};
use http::header;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};

use api::response::Stream;

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
    pub sub_max: u16,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub token: String,
    pub kind: NodeKind,
    pub url: String,
    pub mode: UpdateMode,
    /// Liveman-side per-stream subscriber capacity from `[[nodes]] sub_max`
    /// (`None` = unlimited).
    pub sub_max: Option<u16>,
    /// Whether the node is currently reachable: poll-mode nodes are marked
    /// by their `/api/streams/` poll result, SSE nodes by their stream
    /// connection, and net4mqtt nodes by discovery presence.
    pub online: bool,

    streams: Vec<Stream>,
    /// Round-trip time of the last successful poll contact.
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
            sub_max: v.sub_max.unwrap_or(u16::MAX),
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Server {
            alias: String::default(),
            token: String::default(),
            url: String::default(),
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
    /// Desired cascade hops, keyed by stream. The supervisor
    /// (`route::cascade::cascade_supervisor`) converges the actual hop to
    /// each intent and the reaper (`tick::cascade_check`) tears intents
    /// down; inserts notify `cascade_notify` so the first establishment
    /// attempt is immediate instead of waiting for a tick.
    cascade_intents: Arc<RwLock<HashMap<String, CascadeIntent>>>,
    cascade_notify: Arc<tokio::sync::Notify>,
}

/// A desired inter-node cascade hop for one stream. Nodes are referenced by
/// alias and resolved fresh on every supervisor pass, so node re-registration
/// (changed URL/token) does not strand the intent.
#[derive(Debug, Clone)]
pub struct CascadeIntent {
    pub stream: String,
    pub src: String,
    pub dst: String,
    /// Whether the one-time `close_other_sub` cleanup already ran for this
    /// hop. Runs once per intent, right after the hop first establishes.
    pub subs_closed: bool,
    /// Consecutive issue attempts since the hop was last seen healthy; drives
    /// the retry backoff.
    pub attempts: u32,
    /// Earliest time the next (re)issue is allowed.
    pub next_attempt: Instant,
    /// Since when the destination has had no viewers, maintained by the idle
    /// reaper (`tick::cascade_check`). While set, the supervisor does not
    /// re-establish a dead hop — nobody is watching — which prevents a
    /// create/destroy thrash loop with the destination's auto_delete_whep.
    /// Once it exceeds `cascade.maximum_idle_time` the intent is torn down.
    pub idle_since: Option<Instant>,
    /// Whether the hop was ever seen healthy. Once healthy, a later death
    /// with no viewers watching must not be re-established (the hop dying
    /// dropped everyone); the gate keys off this rather than `attempts`,
    /// because a healthy observation resets the attempt counter.
    pub was_healthy: bool,
}

impl CascadeIntent {
    pub fn new(stream: String, src: String, dst: String) -> Self {
        Self {
            stream,
            src,
            dst,
            subs_closed: false,
            attempts: 0,
            next_attempt: Instant::now(),
            idle_since: None,
            was_healthy: false,
        }
    }
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
            cascade_intents: Arc::new(RwLock::new(HashMap::new())),
            cascade_notify: Arc::new(tokio::sync::Notify::new()),
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

    /// Drop offline nodes from a routing candidate list (`Node::online` is
    /// maintained by poll/SSE/net4mqtt contact health). Candidates built
    /// from snapshots keep stale entries after a node departs; picking one
    /// only produces proxy errors and permanently failing cascade intents.
    pub fn filter_online(&self, servers: &mut Vec<Server>) {
        let nodes = self.list.read().unwrap();
        servers.retain(|s| nodes.get(&s.alias).map(|n| n.online).unwrap_or(false));
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

        // Rebuild stream/session indexes for the new snapshot. Closed
        // sessions linger in the node listing for display (30 s TTL) but are
        // not routable, so they stay out of the session index.
        for stream in streams {
            stream_map
                .entry(stream.id.clone())
                .or_default()
                .push(alias.to_string());
            for session in stream.subscribe.sessions {
                if session.state == api::response::RTCPeerConnectionState::Closed {
                    continue;
                }
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
            // Offline nodes keep their last snapshot in the index; routing a
            // viewer there only produces a proxy error, so leave them out.
            if let Some(n) = nodes.get(&alias)
                && n.online
            {
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

    // Serialize with snapshot updates so proxy writes are not interleaved
    // with apply_snapshot_body rebuilding the stream/session indexes.
    pub async fn session_remove(&self, session: &str) -> Result<()> {
        let _guard = self.update_lock.lock().await;
        self.session
            .write()
            .map_err(|e| anyhow!("{:?}", e))?
            .remove(session);
        Ok(())
    }

    /// Insert or replace the cascade intent for a stream and wake the
    /// supervisor, so the first establishment attempt is immediate.
    pub fn cascade_intent_insert(&self, intent: CascadeIntent) {
        self.cascade_intents
            .write()
            .unwrap()
            .insert(intent.stream.clone(), intent);
        self.cascade_notify.notify_one();
    }

    /// Write back supervisor-side progress (backoff, subs_closed) without
    /// waking the supervisor. Compare-and-store: if the intent was removed
    /// after the supervisor snapshotted it (idle teardown, node gone), the
    /// write-back is dropped instead of resurrecting the intent.
    pub fn cascade_intent_update(&self, intent: CascadeIntent) {
        if let Some(slot) = self
            .cascade_intents
            .write()
            .unwrap()
            .get_mut(&intent.stream)
        {
            *slot = intent;
        }
    }

    pub fn cascade_intent_get(&self, stream: &str) -> Option<CascadeIntent> {
        self.cascade_intents.read().unwrap().get(stream).cloned()
    }

    pub fn cascade_intent_remove(&self, stream: &str) -> Option<CascadeIntent> {
        self.cascade_intents.write().unwrap().remove(stream)
    }

    pub fn cascade_intents(&self) -> Vec<CascadeIntent> {
        self.cascade_intents
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }

    pub fn cascade_notify_waiter(&self) -> Arc<tokio::sync::Notify> {
        self.cascade_notify.clone()
    }

    /// Mark a node's reachability, and on success remember the contact RTT
    /// for the dashboard.
    pub fn node_set_online(&self, alias: &str, online: bool, rtt: Option<Duration>) {
        if let Some(node) = self.list.write().unwrap().get_mut(alias) {
            node.online = online;
            if online && let Some(rtt) = rtt {
                node.duration = Some(rtt);
            }
        }
    }

    async fn update(&mut self) {
        // Throttle check: acquire lock only for the fast in-memory guard,
        // then release before making any HTTP requests so that concurrent
        // snapshot updates (SSE / xdata) are not blocked.
        {
            let update_lock = self.update_lock.clone();
            let _guard = update_lock.lock().await;
            if self.time.elapsed() < Duration::from_secs(3) {
                return;
            }
            self.time = Instant::now();
        }

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
                    .get(format!("{}{}", node.url, api::path::streams("")))
                    .header(header::AUTHORIZATION, format!("Bearer {}", node.token))
                    .send(),
            ));
        }

        let handles = requests
            .into_iter()
            .map(|(alias, value)| {
                // Measure the RTT after the response completes, not at
                // dispatch time.
                tokio::spawn(async move {
                    let res = value.await;
                    (alias, start.elapsed(), res)
                })
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

        // Re-acquire the lock only for the fast in-memory snapshot-apply
        // step so that snapshot writes are serialized.
        let update_lock = self.update_lock.clone();
        let _guard = update_lock.lock().await;

        for handle in handles {
            let result = tokio::join!(handle);
            match result {
                (Ok((alias, duration, Ok(res))),) => {
                    debug!(
                        "{}: spend time: [{:?}] Response: {:?}",
                        alias, duration, res
                    );
                    // Any HTTP response means the node is reachable, even an
                    // error status (auth mismatch, 500) — the snapshot simply
                    // stays stale then.
                    self.node_set_online(&alias, true, Some(duration));

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
                    self.node_set_online(&name, false, None);
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
    fn server_sub_max_comes_from_node_config() {
        let mut limited = Node::new(
            String::new(),
            NodeKind::Static,
            "http://127.0.0.1:7780".to_string(),
            UpdateMode::Poll,
        );
        limited.sub_max = Some(1);
        let server: Server = ("edge0".to_string(), limited).into();
        assert_eq!(server.sub_max, 1);

        let unlimited = Node::new(
            String::new(),
            NodeKind::Static,
            "http://127.0.0.1:7782".to_string(),
            UpdateMode::Poll,
        );
        let server: Server = ("cloud".to_string(), unlimited).into();
        assert_eq!(server.sub_max, u16::MAX);
    }
}
