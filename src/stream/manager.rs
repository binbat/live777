use std::time::Duration;
use std::vec;
use std::{collections::HashMap, sync::Arc};

use crate::forward::info::{ReforwardInfo, StreamInfo};
use crate::result::Result;
use crate::storage::Storage;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::forward::info::Layer;
use crate::forward::PeerForward;
use crate::stream::config::ManagerConfig;
use crate::{metrics, AppError};

use super::config::MetaData;

#[derive(Clone)]
pub struct Manager {
    stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
    config: ManagerConfig,
}

pub type Response = (RTCSessionDescription, String);

impl Manager {
    pub async fn new(cfg: ManagerConfig) -> Self {
        let stream_map: Arc<RwLock<HashMap<String, PeerForward>>> = Default::default();
        tokio::spawn(Self::heartbeat_and_check_tick(
            stream_map.clone(),
            cfg.meta_data.clone(),
            cfg.publish_leave_timeout,
            cfg.storage.clone(),
        ));
        Manager {
            stream_map,
            config: cfg,
        }
    }

    async fn heartbeat_and_check_tick(
        stream_map: Arc<RwLock<HashMap<String, PeerForward>>>,
        meta_data: MetaData,
        publish_leave_timeout: u64,
        storage: Option<Arc<Box<dyn Storage + 'static + Send + Sync>>>,
    ) {
        let publish_leave_timeout_i64: i64 = publish_leave_timeout.try_into().unwrap();
        loop {
            let timeout = tokio::time::sleep(Duration::from_millis(1000));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
            if let Some(storage) = &storage {
                let _ = storage
                    .registry(serde_json::to_string(&meta_data).unwrap())
                    .await;
            }
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
                        let _ = Self::unregister_stream(&storage, stream.clone()).await;
                    }
                }
            }
        }
    }

    pub async fn publish(&self, stream: String, offer: RTCSessionDescription) -> Result<Response> {
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            forward.set_publish(offer).await
        } else {
            if metrics::STREAM.get() >= self.config.meta_data.pub_max as f64 {
                return Err(AppError::LackOfResources);
            }
            let forward = PeerForward::new(stream.clone(), self.config.ice_servers.clone());
            let (sdp, session) = forward.set_publish(offer).await?;
            let mut stream_map = self.stream_map.write().await;
            if stream_map.contains_key(&stream) {
                let _ = forward.close().await;
                return Err(AppError::resource_already_exists("resource already exists"));
            }
            if stream_map.len() >= self.config.meta_data.pub_max as usize {
                warn!("stream {} set publish ok,but exceeded the limit", stream);
                let _ = forward.close().await;
                return Err(AppError::LackOfResources);
            }
            info!("add stream : {}", stream);
            stream_map.insert(stream.clone(), forward);
            metrics::STREAM.inc();
            Self::registry_stream(&self.config.storage, stream.clone()).await?;
            Ok((sdp, session))
        }
    }

    pub async fn subscribe(
        &self,
        stream: String,
        offer: RTCSessionDescription,
    ) -> Result<Response> {
        if metrics::SUBSCRIBE.get() >= self.config.meta_data.sub_max as f64 {
            return Err(AppError::LackOfResources);
        }
        let stream_map = self.stream_map.read().await;
        let forward = stream_map.get(&stream).cloned();
        drop(stream_map);
        if let Some(forward) = forward {
            let (sdp, session) = forward.add_subscribe(offer).await?;
            if metrics::SUBSCRIBE.get() > self.config.meta_data.sub_max as f64 {
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

    async fn registry_stream(
        storage: &Option<Arc<Box<dyn Storage + 'static + Send + Sync>>>,
        stream: String,
    ) -> Result<()> {
        if let Some(storage) = storage {
            storage.registry_stream(stream.clone()).await?;
        }
        Ok(())
    }

    async fn unregister_stream(
        storage: &Option<Arc<Box<dyn Storage + 'static + Send + Sync>>>,
        stream: String,
    ) -> Result<()> {
        if let Some(storage) = storage {
            storage.unregister_stream(stream.clone()).await?;
        }
        Ok(())
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
                let mut stream_map = self.stream_map.write().await;
                info!("remove stream : {}", stream);
                stream_map.remove(&stream);
                metrics::STREAM.dec();
                let _ = Self::unregister_stream(&self.config.storage, stream.clone()).await;
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

    pub async fn info(&self, streams: Vec<String>) -> Vec<StreamInfo> {
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
            if self.config.meta_data.reforward_close_sub {
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
}
