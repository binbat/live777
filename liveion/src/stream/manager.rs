use crate::config::Config;
#[cfg(feature = "source")]
use crate::event::SessionStopReason;
use crate::event::{Event, StreamDeleteReason};

use crate::result::Result;

use chrono::{DateTime, Utc};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use std::vec;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::RwLock;
use tracing::{debug, info, trace};
use webrtc::peer_connection::RTCSessionDescription;

#[cfg(feature = "source")]
use crate::forward::VIRTUAL_SOURCE_SESSION;
use crate::forward::message::Layer;
use crate::forward::{PeerForward, RemovePeerOutcome};
use crate::stream::config::ManagerConfig;
use crate::{AppError, metrics, new_broadcast_channel};

/// Grace period before an orphaned stream (no publisher, no subscribers) is
/// reclaimed by `subscribe_check_tick`. Leaves room for an in-flight
/// `add_subscribe` handshake to attach and never touches freshly created
/// forwards (`subscribe_leave_at` starts at the creation time).
const ORPHAN_GRACE: Duration = Duration::from_secs(5);

/// Orphan teardown only applies to auto-created streams: when neither
/// auto-create is enabled, an empty stream was provisioned explicitly (e.g.
/// via the admin API) and reaping it would break a later publish.
fn orphan_reap_allowed(strategy: &api::strategy::Strategy) -> bool {
    strategy.auto_create_whip || strategy.auto_create_whep
}

/// Single funnel for stream-teardown bookkeeping: the metrics decrement and
/// the `StreamDeleted` lifecycle event always fire as a pair.
fn emit_stream_deleted(
    event_sender: &broadcast::Sender<Event>,
    stream: &str,
    reason: StreamDeleteReason,
) {
    metrics::STREAM.dec();
    let _ = event_sender.send(Event::StreamDeleted {
        stream: stream.to_string(),
        reason,
    });
}

/// Mirror of [`emit_stream_deleted`]: the metrics increment and the `StreamCreated`
/// lifecycle event always fire as a pair.
fn emit_stream_created(event_sender: &broadcast::Sender<Event>, stream: &str) {
    metrics::STREAM.inc();
    let _ = event_sender.send(Event::StreamCreated {
        stream: stream.to_string(),
    });
}

#[cfg(feature = "source")]
use crate::stream::source::*;

#[derive(Clone)]
pub struct Manager {
    stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
    config: ManagerConfig,
    event_sender: broadcast::Sender<Event>,
    cancel: CancellationToken,
    /// Streams declared in the config file (`[stream.<name>]`). Provisioned
    /// streams are pre-registered at startup, always listed (even when idle)
    /// and exempt from every automatic teardown path.
    provisioned: Arc<HashSet<String>>,
    /// Pending "stop on-demand sources" timers, keyed by stream. A new
    /// subscriber cancels the pending stop for its stream.
    #[cfg(feature = "source")]
    on_demand_stop_timers: Arc<tokio::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Per-stream locks serializing on-demand source starts and stops, so a
    /// slow camera start on one stream never blocks another stream's
    /// subscribers.
    #[cfg(feature = "source")]
    on_demand_locks: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// Active RTSP pull sessions per stream. RTSP pull clients tap tracks
    /// directly (no subscribe session), so they are counted separately to
    /// keep on-demand sources alive while they are attached.
    #[cfg(feature = "source")]
    rtsp_pull_counts: Arc<RwLock<HashMap<String, usize>>>,
    #[cfg(feature = "source")]
    pub source_manager: SourceManager,
}

/// Knobs bounding how long one stream's source startup may block.
#[cfg(feature = "source")]
#[derive(Clone, Copy)]
struct SourceStartBudget {
    /// How long to wait for the source's codec to become known before the
    /// start is considered failed.
    codec_timeout: Duration,
    /// Per-attempt codec re-wait inside bridge creation. Startup paths give
    /// the source an extra grace period here; on-demand passes zero because
    /// the subscriber's wait budget was already spent on `codec_timeout`.
    bridge_codec_wait: Duration,
    /// How long to wait for the source's RTCP sender before continuing
    /// without it (non-fatal: keyframe requests just won't work).
    rtcp_wait: Duration,
    /// Bridge creation attempts before giving up.
    max_bridge_retries: u32,
}

/// Budget for startup-triggered source starts (auto-start, provisioned
/// reset): no one is synchronously waiting, so retries are cheap.
#[cfg(feature = "source")]
const STARTUP_SOURCE_BUDGET: SourceStartBudget = SourceStartBudget {
    codec_timeout: Duration::from_secs(10),
    bridge_codec_wait: crate::stream::source::manager::DEFAULT_BRIDGE_CODEC_WAIT,
    rtcp_wait: crate::stream::source::manager::DEFAULT_BRIDGE_RTCP_WAIT,
    max_bridge_retries: 3,
};

#[cfg(feature = "source")]
impl SourceStartBudget {
    /// Budget for subscriber-triggered (on-demand) starts: a single bridge
    /// attempt with no inner re-wait, so the waiting subscriber gets the
    /// outcome within roughly `on_demand_start_timeout_ms`.
    fn on_demand(entry: &crate::config::StreamEntry) -> Self {
        Self {
            codec_timeout: Duration::from_millis(entry.on_demand_start_timeout_ms),
            bridge_codec_wait: Duration::ZERO,
            rtcp_wait: Duration::from_millis(500),
            max_bridge_retries: 1,
        }
    }
}

pub type Response = (RTCSessionDescription, String);

impl Manager {
    pub async fn new(config: Config, cancel: CancellationToken) -> Self {
        let cfg = ManagerConfig::from_config(config.clone());
        let stream_map: Arc<RwLock<HashMap<String, PeerForward>>> = Default::default();
        let send = new_broadcast_channel!(64);
        let provisioned: Arc<HashSet<String>> =
            Arc::new(cfg.stream.streams.keys().cloned().collect());

        tokio::spawn(Self::event_logger(send.subscribe(), cancel.clone()));

        tokio::spawn(Self::publish_check_tick(
            stream_map.clone(),
            send.clone(),
            provisioned.clone(),
            cancel.clone(),
        ));
        tokio::spawn(Self::subscribe_check_tick(
            stream_map.clone(),
            send.clone(),
            provisioned.clone(),
            cancel.clone(),
        ));

        let manager = Manager {
            stream_map,
            config: cfg,
            event_sender: send,
            cancel,
            provisioned,
            #[cfg(feature = "source")]
            on_demand_stop_timers: Default::default(),
            #[cfg(feature = "source")]
            on_demand_locks: Default::default(),
            #[cfg(feature = "source")]
            rtsp_pull_counts: Default::default(),
            #[cfg(feature = "source")]
            source_manager: SourceManager::new(),
        };

        #[cfg(feature = "source")]
        manager.spawn_on_demand_supervisor();

        manager
    }

    /// Canonical lifecycle event log. Emits one debug line per bus event with
    /// its full payload (session, reason) — the single place that answers
    /// "why did this stream/session die". `ForwardChanged` is skipped: it is
    /// a high-frequency ping, not a lifecycle transition.
    async fn event_logger(mut event_recv: broadcast::Receiver<Event>, cancel: CancellationToken) {
        loop {
            let event = tokio::select! {
                _ = cancel.cancelled() => return,
                event = event_recv.recv() => match event {
                    Ok(event) => event,
                    // Surface the loss instead of skipping it silently: the
                    // dropped lines are exactly the ones this log exists for.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("lifecycle: dropped {} events due to lag", n);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            };
            match &event {
                Event::StreamCreated { stream } => {
                    debug!("lifecycle: stream created, stream : {}", stream);
                }
                Event::StreamDeleted { stream, reason } => {
                    debug!(
                        "lifecycle: stream deleted, stream : {}, reason : {:?}",
                        stream, reason
                    );
                }
                Event::PublishStarted { stream, session } => {
                    debug!(
                        "lifecycle: publish started, stream : {}, session : {}",
                        stream, session
                    );
                }
                Event::PublishStopped {
                    stream,
                    session,
                    reason,
                } => {
                    debug!(
                        "lifecycle: publish stopped, stream : {}, session : {}, reason : {:?}",
                        stream, session, reason
                    );
                }
                Event::SubscribeStarted { stream, session } => {
                    debug!(
                        "lifecycle: subscribe started, stream : {}, session : {}",
                        stream, session
                    );
                }
                Event::SubscribeStopped {
                    stream,
                    session,
                    reason,
                } => {
                    debug!(
                        "lifecycle: subscribe stopped, stream : {}, session : {}, reason : {:?}",
                        stream, session, reason
                    );
                }
                Event::ForwardChanged { .. } => {}
            }
        }
    }

    async fn publish_check_tick(
        stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
        event_sender: broadcast::Sender<Event>,
        provisioned: Arc<HashSet<String>>,
        cancel: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(1000)) => {}
                _ = cancel.cancelled() => return,
            }
            let stream_map_read = stream_map.read().await;
            let mut remove_streams = vec![];
            for (stream, forward) in stream_map_read.iter() {
                forward.cleanup_closed_sessions().await;

                // Provisioned streams are never auto-deleted; an idle
                // provisioned stream is a standby stream, not a leak.
                if provisioned.contains(stream) {
                    continue;
                }
                let timeout = forward.strategy().auto_delete_whip.0;
                if timeout < 0 {
                    continue;
                }
                let forward_info = forward.info().await;
                if forward_info.publish_leave_at > 0
                    && Utc::now().timestamp_millis() - forward_info.publish_leave_at > timeout
                {
                    remove_streams.push(stream.clone());
                }
            }
            if remove_streams.is_empty() {
                continue;
            }
            drop(stream_map_read);
            let mut stream_map = stream_map.write().await;
            for stream in remove_streams.iter() {
                if let Some(forward) = stream_map.get(stream) {
                    let timeout = forward.strategy().auto_delete_whip.0;
                    if timeout < 0 {
                        continue;
                    }
                    let forward_info = forward.info().await;
                    if forward_info.publish_leave_at > 0
                        && Utc::now().timestamp_millis() - forward_info.publish_leave_at > timeout
                    {
                        let _ = forward.close().await;
                        stream_map.remove(stream);
                        let publish_leave_at =
                            DateTime::from_timestamp_millis(forward_info.publish_leave_at)
                                .unwrap()
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string();
                        info!(
                            "stream : {}, publish leave timeout, publish leave time : {}",
                            stream, publish_leave_at
                        );

                        emit_stream_deleted(
                            &event_sender,
                            stream,
                            StreamDeleteReason::PublishLeaveTimeout,
                        );
                    }
                }
            }
        }
    }

    async fn subscribe_check_tick(
        stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
        event_sender: broadcast::Sender<Event>,
        provisioned: Arc<HashSet<String>>,
        cancel: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(1000)) => {}
                _ = cancel.cancelled() => return,
            }
            let stream_map_read = stream_map.read().await;
            let mut remove_streams = vec![];
            for (stream, forward) in stream_map_read.iter() {
                // Provisioned streams are never auto-deleted (orphan reaper
                // and subscribe-leave timeout alike).
                if provisioned.contains(stream) {
                    continue;
                }
                let now = Utc::now().timestamp_millis();
                let subscribe_leave_at = forward.subscribe_leave_at().await;
                if subscribe_leave_at <= 0 {
                    continue;
                }
                let strategy = forward.strategy();
                // Orphaned streams are reclaimed without any timeout gate: a
                // viewer that vanishes ungracefully never reaches remove_peer,
                // so this tick is the only path that frees the stream. The
                // exception: when neither auto-create is enabled, an empty
                // stream was provisioned explicitly (e.g. via the admin API)
                // and reaping it would make a later publish fail.
                let recreate = orphan_reap_allowed(strategy);
                if recreate
                    && now - subscribe_leave_at > ORPHAN_GRACE.as_millis() as i64
                    && forward.confirm_orphan_teardown().await
                {
                    remove_streams.push(stream.clone());
                    continue;
                }
                // Closed-session cleanup runs in publish_check_tick only, so we
                // don't duplicate the work (and the resulting events) each tick.
                let timeout = strategy.auto_delete_whep.0;
                if timeout < 0 {
                    continue;
                }
                if now - subscribe_leave_at > timeout {
                    remove_streams.push(stream.clone());
                }
            }
            if remove_streams.is_empty() {
                continue;
            }
            drop(stream_map_read);
            let mut stream_map = stream_map.write().await;
            for stream in remove_streams.iter() {
                if let Some(forward) = stream_map.get(stream) {
                    let now = Utc::now().timestamp_millis();
                    let subscribe_leave_at = forward.subscribe_leave_at().await;
                    if subscribe_leave_at <= 0 {
                        continue;
                    }
                    // Recheck under the write lock: a publisher or subscriber
                    // may have attached since the read pass.
                    let strategy = forward.strategy();
                    let recreate = orphan_reap_allowed(strategy);
                    let orphaned = recreate
                        && now - subscribe_leave_at > ORPHAN_GRACE.as_millis() as i64
                        && forward.confirm_orphan_teardown().await;
                    let timeout = strategy.auto_delete_whep.0;
                    let leave_timeout = timeout >= 0 && now - subscribe_leave_at > timeout;
                    if !orphaned && !leave_timeout {
                        continue;
                    }
                    let _ = forward.close().await;
                    stream_map.remove(stream);
                    let reason = if orphaned {
                        StreamDeleteReason::Orphaned
                    } else {
                        StreamDeleteReason::SubscribeLeaveTimeout
                    };
                    let subscribe_leave_at = DateTime::from_timestamp_millis(subscribe_leave_at)
                        .unwrap()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string();
                    if orphaned {
                        info!(
                            "stream : {}, orphaned, subscribe leave time : {}",
                            stream, subscribe_leave_at
                        );
                    } else {
                        info!(
                            "stream : {}, subscribe leave timeout, subscribe leave time : {}",
                            stream, subscribe_leave_at
                        );
                    }

                    emit_stream_deleted(&event_sender, stream, reason);
                }
            }
        }
    }

    /// Public stream creation (admin API). Provisioned streams are owned by
    /// the config file: creating one through the API is always a conflict,
    /// even in the brief window where an internal teardown has unregistered
    /// the forward but not yet reset it.
    pub async fn stream_create(&self, stream: String) -> std::result::Result<(), anyhow::Error> {
        if self.is_provisioned(&stream) {
            return Err(anyhow::anyhow!(
                "stream '{stream}' is declared in the config file and cannot be created through the API"
            ));
        }
        self.do_stream_create(stream).await
    }

    async fn do_stream_create(&self, stream: String) -> std::result::Result<(), anyhow::Error> {
        {
            let stream_map = self.stream_map.read().await;
            if stream_map.contains_key(&stream) {
                return Err(anyhow::anyhow!("resource already exists"));
            }
        }

        debug!("create stream: {}", stream.clone());
        let forward = self.build_forward(&stream);

        let mut stream_map = self.stream_map.write().await;
        if stream_map.contains_key(&stream) {
            let _ = forward.close().await;
            return Err(anyhow::anyhow!("resource already exists"));
        }
        stream_map.insert(stream.clone(), forward.clone());
        drop(stream_map);
        self.register_stream_created(&stream);
        self.init_stream_forward(&stream, &forward).await;
        Ok(())
    }

    /// Pre-register every stream declared in the config file so that it
    /// exists (and is listed) even while idle. Provisioned streams are exempt
    /// from automatic teardown; their `StreamCreated` events fire here, at
    /// startup, so hooks and the recorder see them immediately.
    ///
    /// Must be called after hook/recorder init so the bus consumers are
    /// already subscribed (the bus does not replay).
    pub async fn provision_streams(&self) {
        let mut names: Vec<&String> = self.config.stream.streams.keys().collect();
        names.sort();
        for stream in names {
            if let Err(e) = self.do_stream_create(stream.clone()).await {
                // A stream can already exist if e.g. the RTSP server created
                // it first; that is not an error worth aborting startup for.
                debug!("provision stream {} skipped: {}", stream, e);
            }
        }
    }

    /// Whether `stream` is declared in the config file (`[stream.<name>]`).
    pub fn is_provisioned(&self, stream: &str) -> bool {
        self.provisioned.contains(stream)
    }

    /// Whether `stream` is a provisioned stream with `on_demand = true`.
    /// Drives the recorder's publish-triggered recording for such streams.
    #[cfg(feature = "recorder")]
    pub fn is_on_demand_stream(&self, stream: &str) -> bool {
        self.config
            .stream
            .streams
            .get(stream)
            .is_some_and(|e| e.on_demand)
    }

    fn build_forward(&self, stream: &str) -> PeerForward {
        let entry = self.config.stream.streams.get(stream);
        let strategy = api::strategy::Strategy::effective(
            &self.config.strategy,
            entry.and_then(|e| e.strategy.as_ref()),
        );
        #[cfg(feature = "source")]
        let channel = entry.and_then(|entry| entry.channel.clone());
        PeerForward::new(
            stream.to_string(),
            self.config.ice_servers.clone(),
            self.config.ice_udp_addrs.clone(),
            #[cfg(feature = "source")]
            channel,
            strategy,
            self.event_sender.clone(),
        )
    }

    fn register_stream_created(&self, stream: &str) {
        info!("add stream : {}", stream);
        emit_stream_created(&self.event_sender, stream);
    }

    async fn init_stream_forward(&self, stream: &str, forward: &PeerForward) {
        #[cfg(feature = "source")]
        if let Err(e) = forward.try_init_udp_channel().await {
            tracing::warn!("Failed to init UDP channel for stream {}: {:?}", stream, e);
        }
        // When the `source` feature is disabled this async fn is empty.
        // The compiler eliminates the future allocation in release builds
        // via dead-code elimination.
        let _ = (stream, forward);
    }

    /// Public stream deletion (admin API). Provisioned streams are owned by
    /// the config file and cannot be deleted through the API.
    pub async fn stream_delete(&self, stream: String) -> std::result::Result<(), anyhow::Error> {
        if self.is_provisioned(&stream) {
            return Err(anyhow::anyhow!(
                "stream '{stream}' is declared in the config file and cannot be deleted"
            ));
        }
        self.teardown_stream(&stream).await
    }

    /// Internal teardown used by the RTSP server (re-ANNOUNCE) and by
    /// session-removal cascades. A dynamic stream is removed outright. A
    /// provisioned stream cannot be unregistered, so its forward is reset to
    /// standby instead: all sessions closed, sources stopped, an empty
    /// forward re-registered. The media-plane replacement is signalled with
    /// a `StreamDeleted` + `StreamCreated` pair so per-stream consumers
    /// (recorder, hooks) restart their state; the registration itself never
    /// lapses and no automatic path ever emits `StreamDeleted` for it.
    pub(crate) async fn teardown_stream(
        &self,
        stream: &str,
    ) -> std::result::Result<(), anyhow::Error> {
        let forward = {
            let mut stream_map = self.stream_map.write().await;
            stream_map.remove(stream)
        };
        let _ = match forward {
            Some(forward) => forward.close().await,
            None => return Err(anyhow::anyhow!("resource not exists")),
        };

        if self.is_provisioned(stream) {
            // A provisioned stream is reset, not deleted: the event pair
            // carries the dedicated Reset reason so hooks can tell a standby
            // reset apart from a real deletion.
            emit_stream_deleted(&self.event_sender, stream, StreamDeleteReason::Reset);
            #[cfg(feature = "source")]
            let _ = self
                .stop_stream_source(stream, SessionStopReason::PeerClosed)
                .await;
            self.reset_provisioned_stream(stream).await;
        } else {
            self.do_stream_delete(stream.to_string()).await;
            info!("remove stream : {}", stream);
        }
        Ok(())
    }

    /// Re-register a provisioned stream after a teardown: rebuild an empty
    /// forward and restore the post-startup state — always-on sources are
    /// restarted, on-demand sources stay stopped until the next subscriber.
    /// Emits the paired `StreamCreated` so snapshot consumers see the stream
    /// return in standby state.
    async fn reset_provisioned_stream(&self, stream: &str) {
        let forward = self.build_forward(stream);
        {
            let mut stream_map = self.stream_map.write().await;
            if stream_map.contains_key(stream) {
                // Someone recreated the stream concurrently; keep that one.
                let _ = forward.close().await;
                return;
            }
            stream_map.insert(stream.to_string(), forward.clone());
        }
        info!("reset provisioned stream to standby: {}", stream);
        self.register_stream_created(stream);
        self.init_stream_forward(stream, &forward).await;

        #[cfg(feature = "source")]
        if let Some(entry) = self.config.stream.streams.get(stream)
            && !entry.on_demand
            && !entry.sources.is_empty()
        {
            self.start_stream_sources(stream, entry, STARTUP_SOURCE_BUDGET)
                .await;
        }
    }

    async fn do_stream_delete(&self, stream: String) {
        emit_stream_deleted(&self.event_sender, &stream, StreamDeleteReason::ApiDeleted);
    }

    pub async fn publish(&self, stream: String, offer: RTCSessionDescription) -> Result<Response> {
        trace!(
            "Publishing to stream: {}, offer type: {:?}",
            stream, offer.sdp_type
        );
        // A stream whose configured source is actively feeding tracks cannot
        // also accept a WHIP publisher — every subscriber would receive both
        // publishers' tracks mixed. Reject like mediamtx's "already
        // publishing" instead. (The reverse direction — a subscriber
        // starting the on-demand source while a publisher is live — is
        // guarded in `ensure_on_demand_source`.)
        #[cfg(feature = "source")]
        if self.source_manager.has_bridge(&stream).await {
            return Err(AppError::stream_source_active(format!(
                "stream '{stream}' has an active configured source"
            )));
        }
        let forward = self
            .get_or_create_forward_for_operation(
                &stream,
                self.config.effective_strategy(&stream).auto_create_whip,
            )
            .await;

        match forward {
            Some(forward) => forward.set_publish(offer).await,
            None => Err(AppError::stream_not_found("stream not exists")),
        }
    }

    pub async fn subscribe(
        &self,
        stream: String,
        offer: RTCSessionDescription,
    ) -> Result<Response> {
        trace!(
            "Subscribing to stream: {}, offer SDP length: {}",
            stream,
            offer.sdp.len()
        );
        // An on-demand stream starts its configured sources here, before the
        // SDP answer is built, so the subscriber's answer already contains
        // the source tracks (WHIP/WHEP has no renegotiation). A source that
        // fails to come up fails the subscribe instead.
        #[cfg(feature = "source")]
        self.ensure_on_demand_source(&stream).await?;
        let forward = self
            .get_or_create_forward_for_operation(
                &stream,
                self.config.effective_strategy(&stream).auto_create_whep,
            )
            .await;

        if let Some(forward) = forward {
            Ok(forward.add_subscribe(offer).await?)
        } else {
            Err(AppError::stream_not_found("stream not exists"))
        }
    }

    /// Look up an existing forward under a read lock; if absent and
    /// `auto_create` is true, create it outside of any lock and insert it
    /// under a brief write lock. Closes a racily-created duplicate to avoid
    /// leaking PeerForward resources.
    async fn get_or_create_forward_for_operation(
        &self,
        stream: &str,
        auto_create: bool,
    ) -> Option<PeerForward> {
        {
            let stream_map = self.stream_map.read().await;
            if let Some(forward) = stream_map.get(stream) {
                return Some(forward.clone());
            }
        }

        if !auto_create {
            return None;
        }

        let raw_forward = self.build_forward(stream);
        let (forward, duplicate_to_close) = {
            let mut stream_map = self.stream_map.write().await;
            if let Some(existing) = stream_map.get(stream) {
                (existing.clone(), Some(raw_forward.clone()))
            } else {
                stream_map.insert(stream.to_string(), raw_forward.clone());
                (raw_forward.clone(), None)
            }
        };
        if let Some(duplicate) = duplicate_to_close {
            let _ = duplicate.close().await;
        } else {
            self.register_stream_created(stream);
            self.init_stream_forward(stream, &forward).await;
        }
        Some(forward)
    }

    pub async fn add_ice_candidate(
        &self,
        stream: String,
        session: String,
        ice_candidates: String,
    ) -> Result<()> {
        let streams = self.stream_map.read().await;
        let forward = streams.get(&stream).cloned();
        drop(streams);
        if let Some(forward) = forward {
            forward.add_ice_candidate(session, ice_candidates).await
        } else {
            Err(AppError::session_not_found("session not exists"))
        }
    }

    pub async fn remove_stream_session(&self, stream: String, session: String) -> Result<()> {
        let streams = self.stream_map.read().await;
        let forward = streams.get(&stream).cloned();
        drop(streams);
        if let Some(forward) = forward {
            match forward.remove_peer(session.clone()).await? {
                RemovePeerOutcome::PublisherRemoved => {
                    // A new publisher may already be mid-handshake on this
                    // forward — deleting the stream now would kill it, so
                    // skip the delete and let the new publisher take over.
                    if !forward.publish_setup_in_progress() {
                        self.stream_delete_allow_missing(stream).await?;
                    }
                }
                RemovePeerOutcome::Orphaned => {
                    // The orphan hint races with publishers/subscribers
                    // attaching — confirm under the forward's locks first.
                    // When neither auto-create is enabled the stream was
                    // provisioned explicitly; leave its teardown to the
                    // admin so a later publish can still find it.
                    if orphan_reap_allowed(forward.strategy())
                        && forward.confirm_orphan_teardown().await
                    {
                        self.stream_delete_allow_missing(stream).await?;
                    }
                }
                RemovePeerOutcome::None => {}
            }
            Ok(())
        } else {
            Err(AppError::session_not_found("session not exists"))
        }
    }

    /// Delete a stream whose teardown was decided by `remove_stream_session`.
    /// The reaper or a concurrent DELETE may have already removed it — that
    /// is the desired end state, so treat an already-missing stream as
    /// success instead of propagating `teardown_stream`'s "resource not
    /// exists" error (which would surface as a 500 on the DELETE API).
    async fn stream_delete_allow_missing(
        &self,
        stream: String,
    ) -> std::result::Result<(), anyhow::Error> {
        match self.teardown_stream(&stream).await {
            Err(_e) if !self.stream_map.read().await.contains_key(&stream) => {
                debug!(
                    "stream : {} already removed by a concurrent teardown",
                    stream
                );
                Ok(())
            }
            result => result,
        }
    }

    pub async fn layers(&self, stream: String) -> Result<Vec<Layer>> {
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            forward.layers().await
        } else {
            Err(AppError::stream_not_found("stream not exists"))
        }
    }

    pub async fn select_layer(
        &self,
        stream: String,
        session: String,
        layer: Option<Layer>,
    ) -> Result<()> {
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            forward.select_layer(session, layer).await
        } else {
            Err(AppError::stream_not_found("stream not exists"))
        }
    }

    pub async fn change_resource(
        &self,
        stream: String,
        session: String,
        change: (String, bool),
    ) -> Result<()> {
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            forward.change_resource(session, change).await
        } else {
            Err(AppError::stream_not_found("stream not exists"))
        }
    }

    pub async fn info(&self, streams: Vec<String>) -> Vec<api::response::Stream> {
        let mut streams = streams.clone();
        streams.retain(|stream| !stream.trim().is_empty());
        let mut resp = vec![];
        let stream_map = self.stream_map.read().await;
        for (stream, forward) in stream_map.iter() {
            if streams.is_empty() || streams.contains(stream) {
                let mut info: api::response::Stream = forward.info().await.into();
                Self::backfill_stream_flags(&self.config.stream, &mut info);
                resp.push(info);
            }
        }
        resp
    }

    /// Fill in the config-derived flags (`provisioned`, `on_demand`) on an
    /// API stream view; the forward itself doesn't know them.
    fn backfill_stream_flags(
        stream_cfg: &crate::config::StreamConfig,
        stream: &mut api::response::Stream,
    ) {
        if let Some(entry) = stream_cfg.streams.get(&stream.id) {
            stream.provisioned = true;
            stream.on_demand = entry.on_demand;
        }
    }

    #[cfg(feature = "cascade")]
    pub async fn cascade_pull(
        &self,
        stream: String,
        src: String,
        token: Option<String>,
    ) -> Result<()> {
        let forward = self
            .get_or_create_forward_for_operation(
                &stream,
                self.config.effective_strategy(&stream).auto_create_whip,
            )
            .await;

        match forward {
            Some(forward) => forward.publish_pull(src, token).await,
            None => Err(AppError::stream_not_found("stream not exists")),
        }
    }

    #[cfg(feature = "cascade")]
    pub async fn cascade_push(
        &self,
        stream: String,
        dst: String,
        token: Option<String>,
    ) -> Result<()> {
        // A cascade push is a subscriber of this stream: start on-demand
        // sources before the reforward session is set up.
        #[cfg(feature = "source")]
        self.ensure_on_demand_source(&stream).await?;
        let streams = self.stream_map.read().await;
        let forward = streams.get(&stream).cloned();
        drop(streams);
        if let Some(forward) = forward {
            forward.subscribe_push(dst, token).await?;
            if forward.strategy().cascade_push_close_sub {
                for subscribe_session_info in forward.info().await.subscribe_session_infos {
                    if subscribe_session_info.cascade.is_none() {
                        // Return value discarded: if this removal orphans the
                        // stream, the subscribe_check_tick reaper tears it
                        // down after the grace period.
                        let _ = forward.remove_peer(subscribe_session_info.id).await;
                    }
                }
            }
            Ok(())
        } else {
            Err(AppError::stream_not_found("stream not exists"))
        }
    }

    async fn do_snapshot(
        stream_map: &Arc<RwLock<HashMap<String, PeerForward>>>,
        stream_cfg: &crate::config::StreamConfig,
        streams: &[String],
    ) -> Vec<api::response::Stream> {
        let stream_map = stream_map.read().await;
        let mut infos: Vec<api::response::Stream> = vec![];
        for forward in stream_map.values() {
            if !streams.is_empty() && !streams.contains(&forward.stream) {
                continue;
            }
            let mut info: api::response::Stream = forward.info().await.into();
            Self::backfill_stream_flags(stream_cfg, &mut info);
            infos.push(info);
        }
        drop(stream_map);
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        for info in &mut infos {
            info.publish.sessions.sort_by(|a, b| a.id.cmp(&b.id));
            info.subscribe.sessions.sort_by(|a, b| a.id.cmp(&b.id));
        }
        infos
    }

    #[cfg(any(feature = "net4mqtt", feature = "recorder"))]
    pub async fn snapshot(&self, streams: &[String]) -> Vec<api::response::Stream> {
        Self::do_snapshot(&self.stream_map, &self.config.stream, streams).await
    }

    pub async fn sse_handler(
        &self,
        streams: Vec<String>,
    ) -> Result<tokio::sync::mpsc::Receiver<Vec<api::response::Stream>>> {
        let (send, recv) = tokio::sync::mpsc::channel(64);
        let mut event_recv = self.event_sender.subscribe();
        let stream_map = self.stream_map.clone();
        let stream_cfg = self.config.stream.clone();
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut last_sent: Option<Vec<api::response::Stream>> = None;

            async fn send_snapshot(
                stream_map: &Arc<RwLock<HashMap<String, PeerForward>>>,
                stream_cfg: &crate::config::StreamConfig,
                streams: &[String],
                last_sent: &mut Option<Vec<api::response::Stream>>,
                send: &tokio::sync::mpsc::Sender<Vec<api::response::Stream>>,
            ) -> bool {
                let infos = Manager::do_snapshot(stream_map, stream_cfg, streams).await;
                if last_sent.as_ref() == Some(&infos) {
                    return true;
                }
                trace!("sse send snapshot with {} streams", infos.len());
                *last_sent = Some(infos.clone());
                send.send(infos).await.is_ok()
            }

            // Send an initial snapshot so the consumer has current state immediately.
            if !send_snapshot(&stream_map, &stream_cfg, &streams, &mut last_sent, &send).await {
                return;
            }

            loop {
                tokio::select! {
                    event = event_recv.recv() => {
                        match event {
                            Ok(event) => {
                                if !(streams.is_empty() || streams.iter().any(|s| s == event.stream())) {
                                    continue;
                                }
                            }
                            // Re-sync unconditionally after dropping events.
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                        if !send_snapshot(&stream_map, &stream_cfg, &streams, &mut last_sent, &send).await {
                            break;
                        }
                    }
                    _ = cancel.cancelled() => {
                        break;
                    }
                }
            }
        });
        Ok(recv)
    }

    #[cfg(feature = "source")]
    async fn wait_for_source_codec(&self, stream_id: &str, timeout: Duration) -> bool {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            if self.source_manager.is_codec_ready(stream_id).await {
                return true;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        false
    }

    // ── On-demand sources ──────────────────────────────────────────────
    //
    // A stream with `on_demand = true` keeps its configured sources stopped
    // until the first subscriber (WHEP / cascade / RTSP pull) arrives, and
    // stops them again `on_demand_close_after_ms` after the last one leaves.
    // The stream itself is provisioned and stays listed the whole time.

    /// Start the stream's configured sources if it is an on-demand stream
    /// whose sources are not running. Blocks until the source bridge is up
    /// (up to `on_demand_start_timeout_ms`) so the caller's SDP answer can
    /// include the source tracks — WHIP/WHEP has no renegotiation, so a
    /// subscriber answered before the tracks exist would never receive
    /// media; instead the subscribe fails with an error and the client can
    /// retry.
    ///
    /// No-op (Ok) for non-on-demand streams, streams whose source bridge is
    /// already up, and streams with a live WHIP publisher (the publisher
    /// feeds the media, starting the configured source would mix two
    /// publishers' tracks).
    ///
    /// Readiness is judged by the *bridge*, not by source existence: a
    /// source that is merely present (a concurrent start still waiting for
    /// its codec, or a zombie from a failed start) has no tracks yet, so
    /// answering a subscriber now would break it silently. Such callers
    /// serialize behind the per-stream lock below instead.
    #[cfg(feature = "source")]
    pub async fn ensure_on_demand_source(&self, stream: &str) -> Result<()> {
        // A pending stop from the previous viewer epoch is obsolete the
        // moment a new consumer arrives — cancel it before any early return.
        self.cancel_on_demand_stop(stream).await;

        let Some(entry) = self.config.stream.streams.get(stream) else {
            return Ok(());
        };
        if !entry.on_demand || entry.sources.is_empty() {
            return Ok(());
        }
        if self.source_manager.has_bridge(stream).await {
            return Ok(());
        }

        // Serialize start vs. start and start vs. stop for THIS stream only.
        // A subscriber arriving while another one's start is in flight
        // blocks here and then finds the bridge up.
        let lock = self.on_demand_lock(stream).await;
        let _guard = lock.lock().await;
        if self.source_manager.has_bridge(stream).await {
            return Ok(());
        }

        // A live WHIP/cascade publisher feeds the media; starting the
        // configured source would mix two publishers' tracks.
        let forward = self.stream_map.read().await.get(stream).cloned();
        if let Some(forward) = forward
            && !forward.has_no_live_publisher().await
        {
            return Ok(());
        }

        // A source left over from a failed start (present, but no bridge)
        // would make `add_source` bail — remove the zombie before retrying.
        // No PublishStopped can be emitted here: the zombie never had a
        // bridge, so no PublishStarted was ever paired with it.
        if self.source_manager.has_source(stream).await {
            self.stop_stream_source_locked(stream, SessionStopReason::PeerClosed)
                .await?;
        }

        info!("on-demand: starting sources for stream {}", stream);
        self.start_stream_sources(stream, entry, SourceStartBudget::on_demand(entry))
            .await;

        if self.source_manager.has_bridge(stream).await {
            // Safety net: arm the close-after timer now. A subscriber that
            // actually attaches cancels it via SubscribeStarted; a caller
            // that never consumes (RTSP DESCRIBE without PLAY, a failed
            // SDP handshake) leaves the source running only for close_after
            // instead of forever. `close_after` must comfortably exceed a
            // subscribe handshake or the timer could stop the source while
            // the first subscriber is still connecting.
            self.maybe_arm_on_demand_stop(stream).await;
            return Ok(());
        }

        // Not ready: stop the half-started source inline so no zombie
        // lingers (and blocks later ensures) until the close-after timer
        // would get around to it.
        tracing::error!(
            "on-demand: source for stream {} not ready after {}ms",
            stream,
            entry.on_demand_start_timeout_ms
        );
        self.stop_stream_source_locked(stream, SessionStopReason::PeerClosed)
            .await?;
        Err(AppError::throw(format!(
            "on-demand source for stream '{stream}' not ready"
        )))
    }

    /// Get (or create) the per-stream on-demand start/stop lock.
    #[cfg(feature = "source")]
    async fn on_demand_lock(&self, stream: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.on_demand_locks
            .lock()
            .await
            .entry(stream.to_string())
            .or_default()
            .clone()
    }

    /// Start all configured sources of one stream entry (shared by
    /// `auto_start_sources` and `ensure_on_demand_source`). `budget` bounds
    /// each wait along the way; see [`SourceStartBudget`].
    #[cfg(feature = "source")]
    async fn start_stream_sources(
        &self,
        stream_id: &str,
        entry: &crate::config::StreamEntry,
        budget: SourceStartBudget,
    ) {
        for source_cfg in &entry.sources {
            // Structured native sources: kind + capture + encoder
            #[cfg(feature = "native-source")]
            if let Some(spec) = source_cfg.to_spec(stream_id) {
                tracing::info!(
                    "Starting native source: {} (backend={})",
                    spec.stream_id,
                    spec.capture.backend
                );
                let source = match create_source_from_spec(&spec).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to create source {}: {}", spec.stream_id, e);
                        continue;
                    }
                };
                self.start_single_source(source, &spec.stream_id, budget)
                    .await;
                continue;
            }
            // URL-based sources (RTSP / SDP)
            if let Some(ref url) = source_cfg.url {
                tracing::info!("Starting URL-based source: {} from {}", stream_id, url);
                let source = match create_source_from_url(stream_id, url, source_cfg).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to create source {}: {}", stream_id, e);
                        continue;
                    }
                };
                self.start_single_source(source, stream_id, budget).await;
            }
        }
    }

    /// Stop the stream's source bridge + source (if running) and remove the
    /// virtual tracks they installed, returning the stream to standby.
    /// Serialized with on-demand starts through the per-stream lock, so a
    /// stop can never interleave with an in-flight `ensure_on_demand_source`.
    #[cfg(feature = "source")]
    pub async fn stop_stream_source(&self, stream: &str, reason: SessionStopReason) -> Result<()> {
        let lock = self.on_demand_lock(stream).await;
        let _guard = lock.lock().await;
        self.stop_stream_source_locked(stream, reason).await
    }

    /// Lock-free inner half of [`Manager::stop_stream_source`], for callers
    /// already holding the stream's on-demand lock. `PublishStopped` (and
    /// the `metrics::PUBLISH` decrement) is emitted only when a bridge
    /// actually existed, pairing the `PublishStarted` from bridge creation.
    #[cfg(feature = "source")]
    async fn stop_stream_source_locked(
        &self,
        stream: &str,
        reason: SessionStopReason,
    ) -> Result<()> {
        if !self.source_manager.has_source(stream).await {
            return Ok(());
        }
        // Snapshot before removal: `remove_source` drops the bridge too.
        let had_bridge = self.source_manager.has_bridge(stream).await;
        self.source_manager.remove_source(stream).await?;
        if let Some(forward) = self.stream_map.read().await.get(stream) {
            forward.remove_virtual_tracks().await;
        }
        if had_bridge {
            metrics::PUBLISH.dec();
            let _ = self.event_sender.send(Event::PublishStopped {
                stream: stream.to_string(),
                session: VIRTUAL_SOURCE_SESSION.to_string(),
                reason,
            });
        }
        info!("stopped source for stream {}", stream);
        Ok(())
    }

    /// Emit the virtual publisher's `PublishStarted` for a source-backed
    /// stream whose bridge just came up.
    #[cfg(feature = "source")]
    pub(crate) fn emit_source_publish_started(&self, stream: &str) {
        metrics::PUBLISH.inc();
        let _ = self.event_sender.send(Event::PublishStarted {
            stream: stream.to_string(),
            session: VIRTUAL_SOURCE_SESSION.to_string(),
        });
    }

    /// Supervisor translating subscriber lifecycle into on-demand source
    /// stops: `SubscribeStarted` cancels a pending stop, `SubscribeStopped`
    /// (with an empty subscriber set) arms the close-after timer.
    #[cfg(feature = "source")]
    fn spawn_on_demand_supervisor(&self) {
        let manager = self.clone();
        let mut events = self.event_sender.subscribe();
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = cancel.cancelled() => return,
                    event = events.recv() => match event {
                        Ok(event) => event,
                        // Missed events can only delay a stop; the next
                        // subscribe transition re-evaluates the stream.
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                };
                match event {
                    Event::SubscribeStarted { stream, .. } => {
                        manager.cancel_on_demand_stop(&stream).await;
                    }
                    Event::SubscribeStopped { stream, .. } => {
                        manager.maybe_arm_on_demand_stop(&stream).await;
                    }
                    _ => {}
                }
            }
        });
    }

    /// Cancel a pending on-demand source stop for `stream`, if any.
    #[cfg(feature = "source")]
    async fn cancel_on_demand_stop(&self, stream: &str) {
        let handle = self.on_demand_stop_timers.lock().await.remove(stream);
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    /// Arm the close-after timer if `stream` is an on-demand stream whose
    /// subscriber set just became empty. The timer re-checks under the same
    /// conditions before stopping the sources.
    #[cfg(feature = "source")]
    async fn maybe_arm_on_demand_stop(&self, stream: &str) {
        let Some(entry) = self.config.stream.streams.get(stream) else {
            return;
        };
        if !entry.on_demand || !self.source_manager.has_source(stream).await {
            return;
        }
        if !self.on_demand_stream_idle(stream).await {
            return;
        }

        let close_after = Duration::from_millis(entry.on_demand_close_after_ms);
        let manager = self.clone();
        let stream = stream.to_string();
        let handle = {
            let stream = stream.clone();
            tokio::spawn(async move {
                tokio::time::sleep(close_after).await;
                // Unregister BEFORE stopping: from here on this task is the
                // stopper, and a cancel must become a no-op instead of
                // aborting the stop halfway through (which would leave
                // stale virtual tracks and an unpaired PublishStarted
                // behind). Cancels during the sleep above still abort.
                manager.on_demand_stop_timers.lock().await.remove(&stream);
                // Re-check under the start/stop lock: a subscriber or RTSP
                // pull may have arrived while the timer was sleeping
                // without cancelling it (e.g. events dropped under
                // broadcast lag), and the stop must not interleave with an
                // in-flight start.
                let lock = manager.on_demand_lock(&stream).await;
                let _guard = lock.lock().await;
                if manager.on_demand_stream_idle(&stream).await {
                    info!(
                        "on-demand: stopping sources for stream {} (no subscribers for {:?})",
                        stream, close_after
                    );
                    if let Err(e) = manager
                        .stop_stream_source_locked(&stream, SessionStopReason::IdleTimeout)
                        .await
                    {
                        tracing::error!("on-demand: failed to stop source for {}: {:?}", stream, e);
                    }
                }
            })
        };

        let mut timers = self.on_demand_stop_timers.lock().await;
        // A newer timer replaces an older pending one for the same stream.
        if let Some(old) = timers.insert(stream, handle) {
            old.abort();
        }
    }

    /// `true` when nothing consumes the stream's media right now: no
    /// subscriber sessions (WHEP/cascade) and no RTSP pull clients.
    /// The recorder is intentionally not counted: it taps tracks internally
    /// and must not keep an on-demand source alive by itself.
    #[cfg(feature = "source")]
    async fn on_demand_stream_idle(&self, stream: &str) -> bool {
        if self
            .rtsp_pull_counts
            .read()
            .await
            .get(stream)
            .is_some_and(|n| *n > 0)
        {
            return false;
        }
        let stream_map = self.stream_map.read().await;
        match stream_map.get(stream) {
            Some(forward) => forward.has_no_subscribers().await,
            None => true,
        }
    }

    /// Register an RTSP pull client attach/detach for on-demand accounting.
    /// The returned guard is unused by callers; counting is internal.
    #[cfg(feature = "source")]
    pub async fn rtsp_pull_attach(&self, stream: &str) {
        *self
            .rtsp_pull_counts
            .write()
            .await
            .entry(stream.to_string())
            .or_insert(0) += 1;
        self.cancel_on_demand_stop(stream).await;
    }

    #[cfg(feature = "source")]
    pub async fn rtsp_pull_detach(&self, stream: &str) {
        let remaining = {
            let mut counts = self.rtsp_pull_counts.write().await;
            match counts.get_mut(stream) {
                Some(n) => {
                    *n = n.saturating_sub(1);
                    let r = *n;
                    if r == 0 {
                        counts.remove(stream);
                    }
                    r
                }
                None => 0,
            }
        };
        if remaining == 0 {
            self.maybe_arm_on_demand_stop(stream).await;
        }
    }

    #[cfg(feature = "source")]
    pub async fn auto_start_sources(
        &self,
        stream_config: &crate::config::StreamConfig,
    ) -> Result<()> {
        let count: usize = stream_config
            .streams
            .values()
            .filter(|e| !e.on_demand)
            .map(|e| e.sources.len())
            .sum();
        if count == 0 {
            tracing::info!("No sources configured, skipping auto-start");
            return Ok(());
        }

        tracing::info!("Auto-starting {} sources", count);

        for (stream_id, entry) in &stream_config.streams {
            // On-demand sources start on the first subscriber instead.
            if entry.on_demand {
                continue;
            }
            self.start_stream_sources(stream_id, entry, STARTUP_SOURCE_BUDGET)
                .await;
        }

        tracing::info!("Auto-start sources completed");
        Ok(())
    }

    #[cfg(feature = "source")]
    async fn start_single_source(
        &self,
        source: Box<dyn crate::stream::source::StreamSource>,
        stream_id: &str,
        budget: SourceStartBudget,
    ) {
        if let Err(e) = self.source_manager.add_source(source).await {
            tracing::error!("Failed to start source {}: {}", stream_id, e);
            return;
        }

        let codec_ready = self
            .wait_for_source_codec(stream_id, budget.codec_timeout)
            .await;

        if !codec_ready {
            tracing::warn!(
                "Codec not ready for source: {} after {:?}, continuing anyway",
                stream_id,
                budget.codec_timeout
            );
        }

        let forward = self.get_or_create_forward(stream_id).await;

        let mut retry_count = 0;

        loop {
            match self
                .source_manager
                .create_bridge(
                    stream_id,
                    forward.clone(),
                    budget.bridge_codec_wait,
                    budget.rtcp_wait,
                )
                .await
            {
                Ok(_) => {
                    tracing::info!("Successfully started source: {}", stream_id);
                    self.emit_source_publish_started(stream_id);
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= budget.max_bridge_retries {
                        tracing::error!(
                            "Failed to create bridge for {} after {} retries: {}",
                            stream_id,
                            budget.max_bridge_retries,
                            e
                        );
                        break;
                    }

                    tracing::warn!(
                        "Failed to create bridge for {} (attempt {}/{}): {}, retrying...",
                        stream_id,
                        retry_count,
                        budget.max_bridge_retries,
                        e
                    );

                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            }
        }
    }

    #[cfg(feature = "source")]
    pub async fn get_or_create_forward_for_source(
        &self,
        stream_id: &str,
    ) -> crate::forward::PeerForward {
        self.get_or_create_forward(stream_id).await
    }

    #[cfg(feature = "source")]
    pub(crate) async fn get_or_create_forward(
        &self,
        stream_id: &str,
    ) -> crate::forward::PeerForward {
        // Fast path: forward already exists.
        {
            let stream_map = self.stream_map.read().await;
            if let Some(forward) = stream_map.get(stream_id) {
                return forward.clone();
            }
        }

        // Create the forward outside the write lock (via the shared
        // `build_forward` helper) so I/O in `try_init_udp_channel` does not
        // block unrelated stream operations.
        let forward = self.build_forward(stream_id);

        // Re-check under write lock in case a concurrent caller won the race.
        let existing = {
            let mut stream_map = self.stream_map.write().await;
            if let Some(existing) = stream_map.get(stream_id) {
                Some(existing.clone())
            } else {
                stream_map.insert(stream_id.to_string(), forward.clone());
                None
            }
        };
        if let Some(existing) = existing {
            let _ = forward.close().await;
            return existing;
        }

        self.register_stream_created(stream_id);
        tracing::info!("Created PeerForward for source: {}", stream_id);
        if let Err(e) = forward.try_init_udp_channel().await {
            tracing::warn!(
                "Failed to init UDP channel for source {}: {:?}",
                stream_id,
                e
            );
        }
        forward
    }

    pub fn subscribe_event(&self) -> broadcast::Receiver<Event> {
        self.event_sender.subscribe()
    }

    #[cfg(any(feature = "rtsp", feature = "recorder"))]
    pub(crate) async fn get_forward(&self, stream: &str) -> Option<crate::forward::PeerForward> {
        let map = self.stream_map.read().await;
        map.get(stream).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tokio::time::{Duration, timeout};

    fn config_with_streams(streams: HashMap<String, crate::config::StreamEntry>) -> Config {
        Config {
            stream: crate::config::StreamConfig { streams },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn concurrent_auto_create_emits_one_stream_created_event() {
        let cancel = CancellationToken::new();
        let manager = Manager::new(Config::default(), cancel).await;
        let mut events = manager.event_sender.subscribe();
        let stream = "race-auto-create";

        let (r1, r2, r3, r4, r5, r6, r7, r8) = tokio::join!(
            manager.get_or_create_forward_for_operation(stream, true),
            manager.get_or_create_forward_for_operation(stream, true),
            manager.get_or_create_forward_for_operation(stream, true),
            manager.get_or_create_forward_for_operation(stream, true),
            manager.get_or_create_forward_for_operation(stream, true),
            manager.get_or_create_forward_for_operation(stream, true),
            manager.get_or_create_forward_for_operation(stream, true),
            manager.get_or_create_forward_for_operation(stream, true),
        );
        let results = [r1, r2, r3, r4, r5, r6, r7, r8];

        assert!(results.iter().all(Option::is_some));

        let mut created_events = 0;
        while let Ok(Ok(event)) = timeout(Duration::from_millis(50), events.recv()).await {
            if let Event::StreamCreated { stream: s } = event
                && s == stream
            {
                created_events += 1;
            }
        }

        assert_eq!(created_events, 1);
    }

    #[tokio::test]
    async fn provisioned_streams_appear_in_info_at_startup() {
        let cancel = CancellationToken::new();
        let mut streams = HashMap::new();
        streams.insert(
            "static-cam".to_string(),
            crate::config::StreamEntry {
                on_demand: true,
                ..Default::default()
            },
        );
        streams.insert("plain".to_string(), crate::config::StreamEntry::default());
        let manager = Manager::new(config_with_streams(streams), cancel).await;

        manager.provision_streams().await;

        let infos = manager.info(vec![]).await;
        assert_eq!(infos.len(), 2);
        let static_cam = infos.iter().find(|s| s.id == "static-cam").unwrap();
        assert!(static_cam.provisioned);
        assert!(static_cam.on_demand);
        assert!(static_cam.publish.sessions.is_empty());
        assert!(static_cam.subscribe.sessions.is_empty());
        let plain = infos.iter().find(|s| s.id == "plain").unwrap();
        assert!(plain.provisioned);
        assert!(!plain.on_demand);

        // Dynamic streams are not flagged as provisioned.
        manager.stream_create("dynamic".to_string()).await.unwrap();
        let dynamic = manager.info(vec!["dynamic".to_string()]).await;
        assert_eq!(dynamic.len(), 1);
        assert!(!dynamic[0].provisioned);

        // Creating an existing stream is a conflict — provisioned ones too.
        assert!(
            manager
                .stream_create("static-cam".to_string())
                .await
                .is_err()
        );
        assert!(manager.stream_create("dynamic".to_string()).await.is_err());
    }

    /// Provisioned streams must survive both the orphan reaper and the
    /// subscribe-leave timeout; a dynamic stream with the same strategy is
    /// reclaimed as usual.
    #[tokio::test]
    async fn provisioned_stream_survives_reapers() {
        let cancel = CancellationToken::new();
        let mut streams = HashMap::new();
        streams.insert("prov".to_string(), crate::config::StreamEntry::default());
        let manager = Manager::new(config_with_streams(streams), cancel.clone()).await;
        manager.provision_streams().await;
        // A dynamic stream under the same default strategy (auto-create on)
        // becomes an orphan right away: no publisher, no subscribers.
        manager.stream_create("dyn".to_string()).await.unwrap();

        // ORPHAN_GRACE is 5s and the reaper ticks every 1s; wait long enough
        // for both paths to have had several chances to fire.
        tokio::time::sleep(Duration::from_millis(7000)).await;

        let ids: Vec<String> = manager
            .info(vec![])
            .await
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(
            ids.contains(&"prov".to_string()),
            "provisioned stream reaped"
        );
        assert!(!ids.contains(&"dyn".to_string()), "dynamic stream survived");

        cancel.cancel();
    }

    #[tokio::test]
    async fn provisioned_stream_cannot_be_deleted_but_teardown_resets_it() {
        let cancel = CancellationToken::new();
        let mut streams = HashMap::new();
        streams.insert("prov".to_string(), crate::config::StreamEntry::default());
        let manager = Manager::new(config_with_streams(streams), cancel).await;
        let mut events = manager.event_sender.subscribe();
        manager.provision_streams().await;

        // Drain the provisioning StreamCreated.
        let _ = timeout(Duration::from_millis(100), events.recv()).await;

        // The public delete path rejects provisioned streams outright.
        let err = manager.stream_delete("prov".to_string()).await;
        assert!(err.is_err(), "provisioned stream delete must be rejected");
        let infos = manager.info(vec!["prov".to_string()]).await;
        assert_eq!(infos.len(), 1, "provisioned stream disappeared");

        // The internal teardown path (RTSP re-ANNOUNCE, session cascades)
        // resets the stream to standby instead.
        manager.teardown_stream("prov").await.unwrap();
        let infos = manager.info(vec!["prov".to_string()]).await;
        assert_eq!(infos.len(), 1);
        assert!(infos[0].provisioned);
        assert!(infos[0].publish.sessions.is_empty());
        assert!(infos[0].subscribe.sessions.is_empty());

        // A StreamDeleted + StreamCreated pair was emitted (deleted first).
        let mut seen_deleted = false;
        let mut seen_created_after_delete = false;
        while let Ok(Ok(event)) = timeout(Duration::from_millis(100), events.recv()).await {
            match event {
                Event::StreamDeleted { stream, .. } if stream == "prov" => seen_deleted = true,
                Event::StreamCreated { stream } if stream == "prov" && seen_deleted => {
                    seen_created_after_delete = true;
                }
                _ => {}
            }
        }
        assert!(seen_deleted, "no StreamDeleted emitted");
        assert!(
            seen_created_after_delete,
            "no standby StreamCreated emitted"
        );
    }

    #[cfg(feature = "source-sdp")]
    mod on_demand {
        use super::*;

        /// Write a minimal H264-over-RTP SDP to a temp file and return its
        /// path. Port 0 lets the OS assign the UDP receive ports.
        async fn write_test_sdp(name: &str) -> String {
            let path = std::env::temp_dir().join(format!(
                "live777-on-demand-{}-{}.sdp",
                name,
                std::process::id()
            ));
            let sdp = "v=0\r\n\
                       o=- 0 0 IN IP4 127.0.0.1\r\n\
                       s=test\r\n\
                       c=IN IP4 127.0.0.1\r\n\
                       t=0 0\r\n\
                       m=video 0 RTP/AVP 96\r\n\
                       a=rtpmap:96 H264/90000\r\n";
            tokio::fs::write(&path, sdp).await.unwrap();
            path.to_string_lossy().to_string()
        }

        fn on_demand_config(sdp_path: &str) -> Config {
            let mut streams = HashMap::new();
            streams.insert(
                "od".to_string(),
                crate::config::StreamEntry {
                    sources: vec![crate::config::SourceConfig {
                        url: Some(format!("file://{}", sdp_path)),
                        #[cfg(feature = "native-source")]
                        capture: None,
                        #[cfg(feature = "native-source")]
                        encoder: None,
                        #[cfg(feature = "native-source")]
                        output: Default::default(),
                    }],
                    on_demand: true,
                    on_demand_close_after_ms: 200,
                    on_demand_start_timeout_ms: 3000,
                    ..Default::default()
                },
            );
            config_with_streams(streams)
        }

        #[tokio::test]
        async fn sources_start_and_stop_with_subscribers() {
            let sdp_path = write_test_sdp("cycle").await;
            let cancel = CancellationToken::new();
            let manager = Manager::new(on_demand_config(&sdp_path), cancel.clone()).await;
            let mut events = manager.event_sender.subscribe();
            manager.provision_streams().await;

            // Standby: listed, provisioned, but no source running.
            assert!(!manager.source_manager.has_source("od").await);
            let infos = manager.info(vec!["od".to_string()]).await;
            assert_eq!(infos.len(), 1);
            assert!(infos[0].on_demand);
            assert!(infos[0].publish.sessions.is_empty());

            // First subscriber arrives -> source starts, virtual publisher appears.
            manager.ensure_on_demand_source("od").await.unwrap();
            assert!(manager.source_manager.has_source("od").await);
            let infos = manager.info(vec!["od".to_string()]).await;
            assert!(
                infos[0]
                    .publish
                    .sessions
                    .iter()
                    .any(|s| s.id == "virtual-source"),
                "no virtual publisher after start: {:?}",
                infos[0].publish.sessions
            );

            let started = timeout(Duration::from_secs(1), async {
                loop {
                    match events.recv().await {
                        Ok(Event::PublishStarted { stream, session })
                            if stream == "od" && session == "virtual-source" =>
                        {
                            break;
                        }
                        Ok(_) => continue,
                        Err(e) => panic!("event bus error: {e}"),
                    }
                }
            })
            .await;
            assert!(started.is_ok(), "no virtual PublishStarted emitted");

            // Last subscriber leaves -> close_after (200ms) -> source stops.
            let _ = manager
                .event_sender
                .send(Event::SubscribeStopped {
                    stream: "od".to_string(),
                    session: "some-viewer".to_string(),
                    reason: crate::event::SessionStopReason::PeerClosed,
                })
                .unwrap();
            tokio::time::sleep(Duration::from_millis(700)).await;
            assert!(
                !manager.source_manager.has_source("od").await,
                "on-demand source did not stop after close_after"
            );
            let infos = manager.info(vec!["od".to_string()]).await;
            assert!(
                infos[0].publish.sessions.is_empty(),
                "stale virtual publisher after stop"
            );

            let stopped = timeout(Duration::from_secs(1), async {
                loop {
                    match events.recv().await {
                        Ok(Event::PublishStopped {
                            stream, session, ..
                        }) if stream == "od" && session == "virtual-source" => {
                            break;
                        }
                        Ok(_) => continue,
                        Err(e) => panic!("event bus error: {e}"),
                    }
                }
            })
            .await;
            assert!(stopped.is_ok(), "no virtual PublishStopped emitted");

            cancel.cancel();
            let _ = std::fs::remove_file(&sdp_path);
        }

        #[tokio::test]
        async fn new_subscriber_cancels_pending_stop() {
            let sdp_path = write_test_sdp("cancel").await;
            let cancel = CancellationToken::new();
            let manager = Manager::new(on_demand_config(&sdp_path), cancel.clone()).await;
            manager.provision_streams().await;
            manager.ensure_on_demand_source("od").await.unwrap();
            assert!(manager.source_manager.has_source("od").await);

            // Viewer leaves, then returns before close_after elapses.
            let _ = manager
                .event_sender
                .send(Event::SubscribeStopped {
                    stream: "od".to_string(),
                    session: "viewer-1".to_string(),
                    reason: crate::event::SessionStopReason::PeerClosed,
                })
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = manager
                .event_sender
                .send(Event::SubscribeStarted {
                    stream: "od".to_string(),
                    session: "viewer-2".to_string(),
                })
                .unwrap();

            // Well past close_after: the stop timer must have been cancelled.
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(
                manager.source_manager.has_source("od").await,
                "on-demand source stopped despite the new subscriber"
            );

            manager
                .stop_stream_source("od", crate::event::SessionStopReason::PeerClosed)
                .await
                .unwrap();
            cancel.cancel();
            let _ = std::fs::remove_file(&sdp_path);
        }

        #[tokio::test]
        async fn ensure_fails_and_cleans_up_when_source_never_ready() {
            // An SDP file that doesn't exist yet: source creation fails, so
            // the ensure must error out instead of letting the subscriber
            // hang — and clean up inline so a later ensure gets a fresh
            // retry instead of finding a zombie.
            let sdp_path = std::env::temp_dir().join(format!(
                "live777-on-demand-retry-{}.sdp",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&sdp_path);
            let cancel = CancellationToken::new();
            let mut streams = HashMap::new();
            streams.insert(
                "od".to_string(),
                crate::config::StreamEntry {
                    sources: vec![crate::config::SourceConfig {
                        url: Some(format!("file://{}", sdp_path.to_string_lossy())),
                        #[cfg(feature = "native-source")]
                        capture: None,
                        #[cfg(feature = "native-source")]
                        encoder: None,
                        #[cfg(feature = "native-source")]
                        output: Default::default(),
                    }],
                    on_demand: true,
                    on_demand_close_after_ms: 100,
                    on_demand_start_timeout_ms: 100,
                    ..Default::default()
                },
            );
            let manager = Manager::new(config_with_streams(streams), cancel.clone()).await;
            manager.provision_streams().await;

            let result = manager.ensure_on_demand_source("od").await;
            assert!(result.is_err(), "ensure must fail for a dead source");
            assert!(
                !manager.source_manager.has_source("od").await,
                "failed source must not linger"
            );

            // The file appears: the next ensure retries cleanly and brings
            // the bridge (and with it the answer-ready tracks) up.
            let sdp = "v=0\r\n\
                       o=- 0 0 IN IP4 127.0.0.1\r\n\
                       s=test\r\n\
                       c=IN IP4 127.0.0.1\r\n\
                       t=0 0\r\n\
                       m=video 0 RTP/AVP 96\r\n\
                       a=rtpmap:96 H264/90000\r\n";
            tokio::fs::write(&sdp_path, sdp).await.unwrap();
            manager.ensure_on_demand_source("od").await.unwrap();
            assert!(
                manager.source_manager.has_bridge("od").await,
                "retry after the file appeared must bring the bridge up"
            );

            manager
                .stop_stream_source("od", crate::event::SessionStopReason::PeerClosed)
                .await
                .unwrap();
            cancel.cancel();
            let _ = std::fs::remove_file(&sdp_path);
        }

        /// A source whose codec never becomes ready: stands in for the
        /// zombie a failed start would leave behind.
        struct NeverReadySource {
            id: String,
            stopped: Arc<std::sync::atomic::AtomicBool>,
            rtp_tx: broadcast::Sender<crate::stream::source::MediaPacket>,
            state_tx: broadcast::Sender<crate::stream::source::StateChangeEvent>,
        }

        #[async_trait::async_trait]
        impl crate::stream::source::StreamSource for NeverReadySource {
            fn stream_id(&self) -> &str {
                &self.id
            }

            fn state(&self) -> crate::stream::source::StreamSourceState {
                crate::stream::source::StreamSourceState::Connected
            }

            async fn start(&mut self) -> anyhow::Result<()> {
                Ok(())
            }

            async fn stop(&mut self) -> anyhow::Result<()> {
                self.stopped
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }

            fn subscribe_rtp(&self) -> broadcast::Receiver<crate::stream::source::MediaPacket> {
                self.rtp_tx.subscribe()
            }

            fn subscribe_state(
                &self,
            ) -> broadcast::Receiver<crate::stream::source::StateChangeEvent> {
                self.state_tx.subscribe()
            }
        }

        #[tokio::test]
        async fn ensure_replaces_zombie_source_and_retries() {
            let sdp_path = write_test_sdp("zombie").await;
            let cancel = CancellationToken::new();
            let manager = Manager::new(on_demand_config(&sdp_path), cancel.clone()).await;
            manager.provision_streams().await;

            // Simulate the leftover of a failed start: a source that exists
            // but has no bridge (its codec never became ready). An ensure
            // that mistook source existence for readiness would return a
            // track-less Ok here.
            let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (rtp_tx, _) = broadcast::channel(1);
            let (state_tx, _) = broadcast::channel(1);
            manager
                .source_manager
                .add_source(Box::new(NeverReadySource {
                    id: "od".to_string(),
                    stopped: stopped.clone(),
                    rtp_tx,
                    state_tx,
                }))
                .await
                .unwrap();
            assert!(manager.source_manager.has_source("od").await);
            assert!(!manager.source_manager.has_bridge("od").await);

            manager.ensure_on_demand_source("od").await.unwrap();
            assert!(
                stopped.load(std::sync::atomic::Ordering::SeqCst),
                "zombie source was not stopped"
            );
            assert!(
                manager.source_manager.has_bridge("od").await,
                "configured source was not started after zombie cleanup"
            );

            manager
                .stop_stream_source("od", crate::event::SessionStopReason::PeerClosed)
                .await
                .unwrap();
            cancel.cancel();
            let _ = std::fs::remove_file(&sdp_path);
        }

        #[tokio::test]
        async fn publish_rejected_while_source_is_running() {
            let sdp_path = write_test_sdp("publish-guard").await;
            let cancel = CancellationToken::new();
            let manager = Manager::new(on_demand_config(&sdp_path), cancel.clone()).await;
            manager.provision_streams().await;
            manager.ensure_on_demand_source("od").await.unwrap();
            assert!(manager.source_manager.has_bridge("od").await);

            // A WHIP publisher attaching to a stream whose configured source
            // is actively feeding tracks would mix both publishers' tracks
            // into every subscriber — reject instead.
            let offer = RTCSessionDescription::offer(
                "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=test\r\nt=0 0\r\n".to_string(),
            )
            .unwrap();
            let err = manager.publish("od".to_string(), offer).await;
            assert!(
                matches!(err, Err(AppError::StreamSourceActive(_))),
                "publish onto a source-active stream must be a source-active conflict: {err:?}"
            );

            manager
                .stop_stream_source("od", crate::event::SessionStopReason::PeerClosed)
                .await
                .unwrap();
            cancel.cancel();
            let _ = std::fs::remove_file(&sdp_path);
        }
    }
}
