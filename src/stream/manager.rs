use crate::config::Config;
use crate::forward::message::{ForwardInfo, ReforwardInfo};
use crate::hook::webhook::WebHook;
use crate::hook::{Event, EventHook, NodeEvent, Stream, StreamEvent, StreamEventType};
use crate::result::Result;
use chrono::{DateTime, Utc};
use live777_http::event::NodeMetaData;
use std::time::Duration;
use std::vec;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::forward::message::Layer;
use crate::forward::PeerForward;
use crate::stream::config::ManagerConfig;
use crate::{metrics, AppError};

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
        let (send, mut recv) = broadcast::channel(4);
        tokio::spawn(async move { while recv.recv().await.is_ok() {} });
        let metadata: NodeMetaData = config.into();
        for web_hook_url in cfg.webhooks.iter() {
            let webhook = WebHook::new(web_hook_url.clone(), cfg.addr, metadata.clone());
            let recv = send.subscribe();
            tokio::spawn(async move {
                webhook.hook(recv).await;
            });
        }
        let _ = send.send(Event::Node(NodeEvent::Up));
        tokio::spawn(Self::keep_alive_tick(send.clone()));
        tokio::spawn(Self::publish_check_tick(
            stream_map.clone(),
            cfg.publish_leave_timeout,
            send.clone(),
        ));
        Manager {
            stream_map,
            config: cfg,
            event_sender: send,
        }
    }

    async fn keep_alive_tick(event_sender: broadcast::Sender<Event>) {
        loop {
            let timeout = tokio::time::sleep(Duration::from_millis(5000));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
            let _ = event_sender.send(Event::Node(NodeEvent::KeepAlive));
        }
    }

    async fn publish_check_tick(
        stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
        publish_leave_timeout: u64,
        event_sender: broadcast::Sender<Event>,
    ) {
        let publish_leave_timeout_i64: i64 = publish_leave_timeout.try_into().unwrap();
        loop {
            let timeout = tokio::time::sleep(Duration::from_millis(1000));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
            let stream_map_read = stream_map.read().await;
            let mut remove_streams = vec![];
            for (stream, forward) in stream_map_read.iter() {
                let forward_info = forward.info().await;
                if forward_info.publish_leave_time > 0
                    && Utc::now().timestamp_millis() - forward_info.publish_leave_time
                        > publish_leave_timeout_i64
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
                    if forward_info.publish_leave_time > 0
                        && Utc::now().timestamp_millis() - forward_info.publish_leave_time
                            > publish_leave_timeout_i64
                    {
                        let _ = forward.close().await;
                        stream_map.remove(stream);
                        metrics::STREAM.dec();
                        let publish_leave_time =
                            DateTime::from_timestamp_millis(forward_info.publish_leave_time)
                                .unwrap()
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string();
                        info!(
                            "stream : {}, publish leave timeout, publish leave time : {}",
                            stream, publish_leave_time
                        );
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

    pub async fn publish(&self, stream: String, offer: RTCSessionDescription) -> Result<Response> {
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            forward.set_publish(offer).await
        } else {
            if metrics::STREAM.get() >= self.config.pub_max as f64 {
                return Err(AppError::LackOfResources);
            }
            let forward = PeerForward::new(stream.clone(), self.config.ice_servers.clone());
            let subscribe_event = forward.subscribe_event();
            tokio::spawn(Self::forward_event_handler(
                subscribe_event,
                self.event_sender.clone(),
            ));
            let (sdp, session) = forward.set_publish(offer).await?;
            let mut stream_map = self.stream_map.write().await;
            if stream_map.contains_key(&stream) {
                let _ = forward.close().await;
                return Err(AppError::resource_already_exists("resource already exists"));
            }
            if stream_map.len() >= self.config.pub_max as usize {
                warn!("stream {} set publish ok,but exceeded the limit", stream);
                let _ = forward.close().await;
                return Err(AppError::LackOfResources);
            }
            info!("add stream : {}", stream);
            stream_map.insert(stream.clone(), forward);
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
            Ok((sdp, session))
        }
    }

    pub async fn subscribe(
        &self,
        stream: String,
        offer: RTCSessionDescription,
    ) -> Result<Response> {
        if metrics::SUBSCRIBE.get() >= self.config.sub_max as f64 {
            return Err(AppError::LackOfResources);
        }
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            let (sdp, session) = forward.add_subscribe(offer).await?;
            if metrics::SUBSCRIBE.get() > self.config.sub_max as f64 {
                warn!("stream {} add subscribe ok,but exceeded the limit", stream);
                let _ = forward.remove_peer(session).await;
                Err(AppError::LackOfResources)
            } else {
                Ok((sdp, session))
            }
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
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
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn remove_stream_session(&self, stream: String, session: String) -> Result<()> {
        let streams = self.stream_map.read().await;
        let forward = streams.get(&stream).cloned();
        drop(streams);
        if let Some(forward) = forward {
            let is_publish = forward.remove_peer(session.clone()).await?;
            if is_publish {
                let _ = forward.close().await;
                let mut stream_map = self.stream_map.write().await;
                info!("remove stream : {}", stream);
                stream_map.remove(&stream);
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
        }
        Ok(())
    }

    pub async fn layers(&self, stream: String) -> Result<Vec<Layer>> {
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            forward.layers().await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
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
            Err(AppError::resource_not_fount("resource not exists"))
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
            Err(AppError::resource_not_fount("resource not exists"))
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

    pub async fn reforward(&self, stream: String, reforward_info: ReforwardInfo) -> Result<()> {
        let streams = self.stream_map.read().await;
        let forward = streams.get(&stream).cloned();
        drop(streams);
        if let Some(forward) = forward {
            forward.reforward(reforward_info).await?;
            if self.config.reforward_close_sub {
                for subscribe_session_info in forward.info().await.subscribe_session_infos {
                    if subscribe_session_info.reforward.is_none() {
                        let _ = forward.remove_peer(subscribe_session_info.id).await;
                    }
                }
            }
            Ok(())
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn shotdown(&self) -> Result<()> {
        let _ = self.event_sender.send(Event::Node(NodeEvent::Down));
        let timeout = tokio::time::sleep(Duration::from_millis(3000));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;
        Ok(())
    }
}
