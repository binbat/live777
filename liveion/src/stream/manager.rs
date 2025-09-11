use crate::config::Config;
use crate::forward::message::ForwardInfo;

use crate::hook::webhook::WebHook;
use crate::hook::{Event, EventHook, Stream, StreamEvent, StreamEventType};

use crate::result::Result;

use chrono::{DateTime, Utc};
use std::time::Duration;

use tokio::sync::broadcast;

use std::vec;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{debug, info};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::forward::PeerForward;
use crate::forward::message::Layer;
use crate::stream::config::ManagerConfig;
use crate::{AppError, metrics, new_broadcast_channel};

#[derive(Clone)]
pub struct Manager {
    stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
    config: ManagerConfig,
    event_sender: broadcast::Sender<Event>,
}

pub type Response = (RTCSessionDescription, String);

impl Manager {
    pub async fn new(config: Config) -> Self {
        let cfg = ManagerConfig::from_config(config.clone());
        let stream_map: Arc<RwLock<HashMap<String, PeerForward>>> = Default::default();
        let send = new_broadcast_channel!(4);
        for web_hook_url in cfg.webhooks.iter() {
            let webhook = WebHook::new(web_hook_url.clone());
            let recv = send.subscribe();
            tokio::spawn(async move {
                webhook.hook(recv).await;
            });
        }

        if cfg.auto_delete_pub >= 0 {
            tokio::spawn(Self::publish_check_tick(
                stream_map.clone(),
                cfg.auto_delete_pub,
                send.clone(),
            ));
        }

        if cfg.auto_delete_sub >= 0 {
            tokio::spawn(Self::subscribe_check_tick(
                stream_map.clone(),
                cfg.auto_delete_sub,
                send.clone(),
            ));
        }

        Manager {
            stream_map,
            config: cfg,
            event_sender: send,
        }
    }

    async fn publish_check_tick(
        stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
        publish_leave_atout: i64,
        event_sender: broadcast::Sender<Event>,
    ) {
        loop {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            let stream_map_read = stream_map.read().await;
            let mut remove_streams = vec![];
            for (stream, forward) in stream_map_read.iter() {
                let forward_info = forward.info().await;
                if forward_info.publish_leave_at > 0
                    && Utc::now().timestamp_millis() - forward_info.publish_leave_at
                        > publish_leave_atout
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
                    let forward_info = forward.info().await;
                    if forward_info.publish_leave_at > 0
                        && Utc::now().timestamp_millis() - forward_info.publish_leave_at
                            > publish_leave_atout
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

                        metrics::STREAM.dec();
                        let _ = event_sender.send(Event::Stream(StreamEvent {
                            r#type: StreamEventType::Down,
                            stream: Stream {
                                stream: stream.clone(),
                                session: None,
                                publish: 0,
                                subscribe: 0,
                                reforward: 0,
                            },
                        }));
                    }
                }
            }
        }
    }

    async fn subscribe_check_tick(
        stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
        subscribe_leave_atout: i64,
        event_sender: broadcast::Sender<Event>,
    ) {
        loop {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            let stream_map_read = stream_map.read().await;
            let mut remove_streams = vec![];
            for (stream, forward) in stream_map_read.iter() {
                let forward_info = forward.info().await;
                if forward_info.subscribe_leave_at > 0
                    && Utc::now().timestamp_millis() - forward_info.publish_leave_at
                        > subscribe_leave_atout
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
                    let forward_info = forward.info().await;
                    if forward_info.subscribe_leave_at > 0
                        && Utc::now().timestamp_millis() - forward_info.subscribe_leave_at
                            > subscribe_leave_atout
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

                        metrics::STREAM.dec();
                        let _ = event_sender.send(Event::Stream(StreamEvent {
                            r#type: StreamEventType::Down,
                            stream: Stream {
                                stream: stream.clone(),
                                session: None,
                                publish: 0,
                                subscribe: 0,
                                reforward: 0,
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
        let forward = PeerForward::new(stream.clone(), self.config.ice_servers.clone());
        let subscribe_event = forward.subscribe_event();
        tokio::spawn(Self::forward_event_handler(
            subscribe_event,
            self.event_sender.clone(),
        ));

        info!("add stream : {}", stream);
        metrics::STREAM.inc();
        let _ = self.event_sender.send(Event::Stream(StreamEvent {
            stream: Stream {
                stream,
                session: None,
                publish: 1,
                subscribe: 0,
                reforward: 0,
            },
            r#type: StreamEventType::Up,
        }));
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
            stream: Stream {
                stream,
                publish: 0,
                subscribe: 0,
                reforward: 0,
                session: None,
            },
            r#type: StreamEventType::Down,
        }));
    }

    pub async fn publish(&self, stream: String, offer: RTCSessionDescription) -> Result<Response> {
        let mut stream_map = self.stream_map.write().await;
        let mut forward = stream_map.get(&stream).cloned();
        if forward.is_none() && self.config.auto_create_pub {
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
        let mut stream_map = self.stream_map.write().await;
        let mut forward = stream_map.get(&stream).cloned();
        if forward.is_none() && self.config.auto_create_sub {
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

    pub async fn cascade_pull(
        &self,
        stream: String,
        src: String,
        token: Option<String>,
    ) -> Result<()> {
        let mut stream_map = self.stream_map.write().await;
        let mut forward = stream_map.get(&stream).cloned();
        if forward.is_none() && self.config.auto_create_pub {
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
            if self.config.cascade_push_close_sub {
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

    pub async fn sse_handler(
        &self,
        streams: Vec<String>,
    ) -> Result<tokio::sync::mpsc::Receiver<Vec<ForwardInfo>>> {
        let (send, recv) = tokio::sync::mpsc::channel(64);
        let mut evnet_recv = self.event_sender.subscribe();
        let stream_map = self.stream_map.clone();
        tokio::spawn(async move {
            while let Ok(event) = evnet_recv.recv().await {
                let stream = match event {
                    Event::Stream(val) => val.stream.stream,
                    Event::Forward(val) => val.stream_info.id,
                };
                if streams.is_empty() || streams.contains(&stream) {
                    let stream_map = stream_map.read().await;
                    let mut infos = vec![];
                    for (_, forward) in stream_map.iter() {
                        if !streams.is_empty() && !streams.contains(&forward.stream) {
                            continue;
                        }
                        infos.push(forward.info().await);
                    }
                    let _ = send.send(infos).await;
                }
            }
        });
        Ok(recv)
    }
}
