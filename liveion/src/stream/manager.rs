use crate::config::Config;
use crate::event::{Event, StreamDownReason};
use crate::forward::message::ForwardInfo;

use crate::result::Result;

use chrono::{DateTime, Utc};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use std::vec;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{debug, info, trace};
use webrtc::peer_connection::RTCSessionDescription;

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
/// the `StreamDown` lifecycle event always fire as a pair.
fn emit_stream_down(
    event_sender: &broadcast::Sender<Event>,
    stream: &str,
    reason: StreamDownReason,
) {
    metrics::STREAM.dec();
    let _ = event_sender.send(Event::StreamDown {
        stream: stream.to_string(),
        reason,
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
    #[cfg(feature = "source")]
    pub source_manager: SourceManager,
}

pub type Response = (RTCSessionDescription, String);

impl Manager {
    pub async fn new(config: Config, cancel: CancellationToken) -> Self {
        let cfg = ManagerConfig::from_config(config.clone());
        let stream_map: Arc<RwLock<HashMap<String, PeerForward>>> = Default::default();
        let send = new_broadcast_channel!(64);

        tokio::spawn(Self::event_logger(send.subscribe(), cancel.clone()));

        tokio::spawn(Self::publish_check_tick(
            stream_map.clone(),
            send.clone(),
            cancel.clone(),
        ));
        tokio::spawn(Self::subscribe_check_tick(
            stream_map.clone(),
            send.clone(),
            cancel.clone(),
        ));

        Manager {
            stream_map,
            config: cfg,
            event_sender: send,
            cancel,
            #[cfg(feature = "source")]
            source_manager: SourceManager::new(),
        }
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
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            };
            match &event {
                Event::StreamUp { stream } => {
                    debug!("lifecycle: stream up, stream : {}", stream);
                }
                Event::StreamDown { stream, reason } => {
                    debug!(
                        "lifecycle: stream down, stream : {}, reason : {:?}",
                        stream, reason
                    );
                }
                Event::PublishUp { stream, session } => {
                    debug!(
                        "lifecycle: publish up, stream : {}, session : {}",
                        stream, session
                    );
                }
                Event::PublishDown {
                    stream,
                    session,
                    reason,
                } => {
                    debug!(
                        "lifecycle: publish down, stream : {}, session : {}, reason : {:?}",
                        stream, session, reason
                    );
                }
                Event::SubscribeUp { stream, session } => {
                    debug!(
                        "lifecycle: subscribe up, stream : {}, session : {}",
                        stream, session
                    );
                }
                Event::SubscribeDown {
                    stream,
                    session,
                    reason,
                } => {
                    debug!(
                        "lifecycle: subscribe down, stream : {}, session : {}, reason : {:?}",
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

                        emit_stream_down(
                            &event_sender,
                            stream,
                            StreamDownReason::PublishLeaveTimeout,
                        );
                    }
                }
            }
        }
    }

    async fn subscribe_check_tick(
        stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
        event_sender: broadcast::Sender<Event>,
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
                        StreamDownReason::Orphaned
                    } else {
                        StreamDownReason::SubscribeLeaveTimeout
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

                    emit_stream_down(&event_sender, stream, reason);
                }
            }
        }
    }

    pub async fn stream_create(&self, stream: String) -> std::result::Result<(), anyhow::Error> {
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
        metrics::STREAM.inc();
        let _ = self.event_sender.send(Event::StreamUp {
            stream: stream.to_string(),
        });
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

    pub async fn stream_delete(&self, stream: String) -> std::result::Result<(), anyhow::Error> {
        let forward = {
            let mut stream_map = self.stream_map.write().await;
            stream_map.remove(&stream)
        };
        let _ = match forward {
            Some(forward) => forward.close().await,
            None => return Err(anyhow::anyhow!("resource not exists")),
        };

        self.do_stream_delete(stream.clone()).await;
        info!("remove stream : {}", stream);
        Ok(())
    }

    async fn do_stream_delete(&self, stream: String) {
        emit_stream_down(&self.event_sender, &stream, StreamDownReason::ApiDeleted);
    }

    pub async fn publish(&self, stream: String, offer: RTCSessionDescription) -> Result<Response> {
        trace!(
            "Publishing to stream: {}, offer type: {:?}",
            stream, offer.sdp_type
        );
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
    /// success instead of propagating `stream_delete`'s "resource not
    /// exists" error (which would surface as a 500 on the DELETE API).
    async fn stream_delete_allow_missing(
        &self,
        stream: String,
    ) -> std::result::Result<(), anyhow::Error> {
        match self.stream_delete(stream.clone()).await {
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

    pub async fn info(&self, streams: Vec<String>) -> Vec<ForwardInfo> {
        let mut streams = streams.clone();
        streams.retain(|stream| !stream.trim().is_empty());
        let mut resp = vec![];
        let stream_map = self.stream_map.read().await;
        for (stream, forward) in stream_map.iter() {
            if streams.is_empty() || streams.contains(stream) {
                resp.push(forward.info().await);
            }
        }
        resp
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
        streams: &[String],
    ) -> Vec<api::response::Stream> {
        let stream_map = stream_map.read().await;
        let mut infos: Vec<api::response::Stream> = vec![];
        for forward in stream_map.values() {
            if !streams.is_empty() && !streams.contains(&forward.stream) {
                continue;
            }
            infos.push(forward.info().await.into());
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
        Self::do_snapshot(&self.stream_map, streams).await
    }

    pub async fn sse_handler(
        &self,
        streams: Vec<String>,
    ) -> Result<tokio::sync::mpsc::Receiver<Vec<api::response::Stream>>> {
        let (send, recv) = tokio::sync::mpsc::channel(64);
        let mut event_recv = self.event_sender.subscribe();
        let stream_map = self.stream_map.clone();
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut last_sent: Option<Vec<api::response::Stream>> = None;

            async fn send_snapshot(
                stream_map: &Arc<RwLock<HashMap<String, PeerForward>>>,
                streams: &[String],
                last_sent: &mut Option<Vec<api::response::Stream>>,
                send: &tokio::sync::mpsc::Sender<Vec<api::response::Stream>>,
            ) -> bool {
                let infos = Manager::do_snapshot(stream_map, streams).await;
                if last_sent.as_ref() == Some(&infos) {
                    return true;
                }
                trace!("sse send snapshot with {} streams", infos.len());
                *last_sent = Some(infos.clone());
                send.send(infos).await.is_ok()
            }

            // Send an initial snapshot so the consumer has current state immediately.
            if !send_snapshot(&stream_map, &streams, &mut last_sent, &send).await {
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
                        if !send_snapshot(&stream_map, &streams, &mut last_sent, &send).await {
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
    async fn wait_for_source_codec(&self, stream_id: &str, timeout_ms: u64) -> bool {
        let start = std::time::Instant::now();

        while start.elapsed().as_millis() < timeout_ms as u128 {
            if self.source_manager.is_codec_ready(stream_id).await {
                return true;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        false
    }

    #[cfg(feature = "source")]
    pub async fn auto_start_sources(
        &self,
        stream_config: &crate::config::StreamConfig,
    ) -> Result<()> {
        let count: usize = stream_config
            .streams
            .values()
            .map(|e| e.sources.len())
            .sum();
        if count == 0 {
            tracing::info!("No sources configured, skipping auto-start");
            return Ok(());
        }

        tracing::info!("Auto-starting {} sources", count);

        for (stream_id, entry) in &stream_config.streams {
            for source_cfg in &entry.sources {
                // Structured native sources: kind + capture + encoder
                #[cfg(feature = "native-source")]
                if let Some(spec) = source_cfg.to_spec(stream_id) {
                    tracing::info!(
                        "Auto-starting native source: {} (backend={})",
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
                    self.start_single_source(source, &spec.stream_id).await;
                    continue;
                }
                // URL-based sources (RTSP / SDP)
                if let Some(ref url) = source_cfg.url {
                    tracing::info!("Auto-starting URL-based source: {} from {}", stream_id, url);
                    let source = match create_source_from_url(stream_id, url, source_cfg).await {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("Failed to create source {}: {}", stream_id, e);
                            continue;
                        }
                    };
                    self.start_single_source(source, stream_id).await;
                }
            }
        }

        tracing::info!("Auto-start sources completed");
        Ok(())
    }

    #[cfg(feature = "source")]
    async fn start_single_source(
        &self,
        source: Box<dyn crate::stream::source::StreamSource>,
        stream_id: &str,
    ) {
        if let Err(e) = self.source_manager.add_source(source).await {
            tracing::error!("Failed to start source {}: {}", stream_id, e);
            return;
        }

        let codec_ready = self.wait_for_source_codec(stream_id, 10000).await;

        if !codec_ready {
            tracing::warn!(
                "Codec not ready for source: {} after 10s, continuing anyway",
                stream_id
            );
        }

        let forward = self.get_or_create_forward(stream_id).await;

        let mut retry_count = 0;
        let max_retries = 3;

        loop {
            match self
                .source_manager
                .create_bridge(stream_id, forward.clone())
                .await
            {
                Ok(_) => {
                    tracing::info!("Successfully started source: {}", stream_id);
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= max_retries {
                        tracing::error!(
                            "Failed to create bridge for {} after {} retries: {}",
                            stream_id,
                            max_retries,
                            e
                        );
                        break;
                    }

                    tracing::warn!(
                        "Failed to create bridge for {} (attempt {}/{}): {}, retrying...",
                        stream_id,
                        retry_count,
                        max_retries,
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

    #[cfg(any(feature = "net4mqtt", feature = "recorder"))]
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

    #[tokio::test]
    async fn concurrent_auto_create_emits_one_stream_up_event() {
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

        let mut up_events = 0;
        while let Ok(Ok(event)) = timeout(Duration::from_millis(50), events.recv()).await {
            if let Event::StreamUp { stream: s } = event
                && s == stream
            {
                up_events += 1;
            }
        }

        assert_eq!(up_events, 1);
    }
}
