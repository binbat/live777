use crate::config::Config;
use crate::forward::message::ForwardInfo;

use crate::hook::{Event, Stream, StreamEvent, StreamEventType};

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

use crate::forward::PeerForward;
use crate::forward::message::Layer;
use crate::stream::config::ManagerConfig;
use crate::{AppError, metrics, new_broadcast_channel};

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
        let send = new_broadcast_channel!(4);

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
                        metrics::STREAM.dec();
                        let publish_leave_at =
                            DateTime::from_timestamp_millis(forward_info.publish_leave_at)
                                .unwrap()
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string();
                        info!(
                            "stream : {}, publish leave timeout, publish leave time : {}",
                            stream, publish_leave_at
                        );

                        let _ = event_sender.send(Event::Stream(StreamEvent {
                            r#type: StreamEventType::Down,
                            stream: Stream {
                                stream: stream.clone(),
                            },
                        }));
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
                // Closed-session cleanup runs in publish_check_tick only, so we
                // don't duplicate the work (and the resulting events) each tick.
                let timeout = forward.strategy().auto_delete_whep.0;
                if timeout < 0 {
                    continue;
                }
                let forward_info = forward.info().await;
                if forward_info.subscribe_leave_at > 0
                    && Utc::now().timestamp_millis() - forward_info.subscribe_leave_at > timeout
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
                    let timeout = forward.strategy().auto_delete_whep.0;
                    if timeout < 0 {
                        continue;
                    }
                    let forward_info = forward.info().await;
                    if forward_info.subscribe_leave_at > 0
                        && Utc::now().timestamp_millis() - forward_info.subscribe_leave_at > timeout
                    {
                        let _ = forward.close().await;
                        stream_map.remove(stream);
                        metrics::STREAM.dec();
                        let subscribe_leave_at =
                            DateTime::from_timestamp_millis(forward_info.subscribe_leave_at)
                                .unwrap()
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string();
                        info!(
                            "stream : {}, subscribe leave timeout, publish leave time : {}",
                            stream, subscribe_leave_at
                        );

                        let _ = event_sender.send(Event::Stream(StreamEvent {
                            r#type: StreamEventType::Down,
                            stream: Stream {
                                stream: stream.clone(),
                            },
                        }));
                    }
                }
            }
        }
    }

    pub async fn forward_event_handler(
        mut stream_event: broadcast::Receiver<crate::forward::message::ForwardEvent>,
        hook_event: broadcast::Sender<Event>,
    ) {
        while let Ok(event) = stream_event.recv().await {
            trace!("forward event for stream {}", event.stream_id);
            let _ = hook_event.send(Event::Forward(event));
        }
    }

    pub async fn stream_create(&self, stream: String) -> std::result::Result<(), anyhow::Error> {
        let mut stream_map = self.stream_map.write().await;
        let forward = stream_map.get(&stream).cloned();
        if forward.is_some() {
            return Err(anyhow::anyhow!("resource already exists"));
        }
        debug!("create stream: {}", stream.clone());
        let forward = self.do_stream_create(stream.clone()).await;
        stream_map.insert(stream.clone(), forward);
        drop(stream_map);
        Ok(())
    }

    async fn do_stream_create(&self, stream: String) -> PeerForward {
        let entry = self.config.stream.streams.get(&stream);
        let strategy = api::strategy::Strategy::effective(
            &self.config.strategy,
            entry.and_then(|e| e.strategy.as_ref()),
        );
        #[cfg(feature = "source")]
        let channel = entry.and_then(|entry| entry.channel.clone());
        #[cfg(feature = "source")]
        let forward = PeerForward::new(
            stream.clone(),
            self.config.ice_servers.clone(),
            self.config.ice_udp_addrs.clone(),
            channel,
            strategy,
        );
        #[cfg(not(feature = "source"))]
        let forward = PeerForward::new(
            stream.clone(),
            self.config.ice_servers.clone(),
            self.config.ice_udp_addrs.clone(),
            strategy,
        );
        let subscribe_event = forward.subscribe_event();
        tokio::spawn(Self::forward_event_handler(
            subscribe_event,
            self.event_sender.clone(),
        ));

        info!("add stream : {}", stream);
        metrics::STREAM.inc();
        let _ = self.event_sender.send(Event::Stream(StreamEvent {
            stream: Stream {
                stream: stream.clone(),
            },
            r#type: StreamEventType::Up,
        }));
        #[cfg(feature = "source")]
        if let Err(e) = forward.try_init_udp_channel().await {
            tracing::warn!("Failed to init UDP channel for stream {}: {:?}", stream, e);
        }
        forward
    }

    pub async fn stream_delete(&self, stream: String) -> std::result::Result<(), anyhow::Error> {
        let mut stream_map = self.stream_map.write().await;
        let forward = stream_map.get(&stream).cloned();
        let _ = match forward {
            Some(forward) => forward.close().await,
            None => return Err(anyhow::anyhow!("resource not exists")),
        };
        stream_map.remove(&stream);
        drop(stream_map);

        self.do_stream_delete(stream.clone()).await;
        info!("remove stream : {}", stream);
        Ok(())
    }

    async fn do_stream_delete(&self, stream: String) {
        metrics::STREAM.dec();
        let _ = self.event_sender.send(Event::Stream(StreamEvent {
            stream: Stream { stream },
            r#type: StreamEventType::Down,
        }));
    }

    pub async fn publish(&self, stream: String, offer: RTCSessionDescription) -> Result<Response> {
        trace!(
            "Publishing to stream: {}, offer type: {:?}",
            stream, offer.sdp_type
        );
        let mut stream_map = self.stream_map.write().await;
        let mut forward = stream_map.get(&stream).cloned();
        if forward.is_none() && self.config.effective_strategy(&stream).auto_create_whip {
            let raw_forward = self.do_stream_create(stream.clone()).await;
            stream_map.insert(stream.clone(), raw_forward.clone());
            forward = Some(raw_forward);
        }
        drop(stream_map);

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
        let mut stream_map = self.stream_map.write().await;
        let mut forward = stream_map.get(&stream).cloned();
        if forward.is_none() && self.config.effective_strategy(&stream).auto_create_whep {
            let raw_forward = self.do_stream_create(stream.clone()).await;
            stream_map.insert(stream.clone(), raw_forward.clone());
            forward = Some(raw_forward);
        }
        drop(stream_map);

        if let Some(forward) = forward {
            Ok(forward.add_subscribe(offer).await?)
        } else {
            Err(AppError::stream_not_found("stream not exists"))
        }
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
            let is_publish = forward.remove_peer(session.clone()).await?;
            if is_publish {
                self.stream_delete(stream).await?;
            }
            Ok(())
        } else {
            Err(AppError::session_not_found("session not exists"))
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
        let mut stream_map = self.stream_map.write().await;
        let mut forward = stream_map.get(&stream).cloned();
        if forward.is_none() && self.config.effective_strategy(&stream).auto_create_whip {
            let raw_forward = self.do_stream_create(stream.clone()).await;
            stream_map.insert(stream.clone(), raw_forward.clone());
            forward = Some(raw_forward);
        }
        drop(stream_map);

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
        for (_, forward) in stream_map.iter() {
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
                    Ok(event) = event_recv.recv() => {
                        let stream = match event {
                            Event::Stream(val) => val.stream.stream,
                            Event::Forward(val) => val.stream_id,
                        };
                        if (streams.is_empty() || streams.contains(&stream))
                            && !send_snapshot(&stream_map, &streams, &mut last_sent, &send).await
                        {
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
        let mut stream_map = self.stream_map.write().await;

        if let Some(forward) = stream_map.get(stream_id) {
            forward.clone()
        } else {
            let entry = self.config.stream.streams.get(stream_id);
            let channel = entry.and_then(|entry| entry.channel.clone());
            let strategy = api::strategy::Strategy::effective(
                &self.config.strategy,
                entry.and_then(|e| e.strategy.as_ref()),
            );
            let forward = crate::forward::PeerForward::new(
                stream_id.to_string(),
                self.config.ice_servers.clone(),
                self.config.ice_udp_addrs.clone(),
                channel,
                strategy,
            );

            let subscribe_event = forward.subscribe_event();
            let event_sender = self.event_sender.clone();
            tokio::spawn(Self::forward_event_handler(subscribe_event, event_sender));

            stream_map.insert(stream_id.to_string(), forward.clone());

            tracing::info!("Created PeerForward for source: {}", stream_id);
            #[cfg(feature = "source")]
            if let Err(e) = forward.try_init_udp_channel().await {
                tracing::warn!(
                    "Failed to init UDP channel for source {}: {:?}",
                    stream_id,
                    e
                );
            }
            forward
        }
    }

    #[cfg(any(feature = "net4mqtt", feature = "recorder"))]
    pub fn subscribe_event(&self) -> broadcast::Receiver<Event> {
        self.event_sender.subscribe()
    }

    pub(crate) async fn get_forward(&self, stream: &str) -> Option<crate::forward::PeerForward> {
        let map = self.stream_map.read().await;
        map.get(stream).cloned()
    }
}
