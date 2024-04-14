use std::time::Duration;
use std::vec;
use std::{collections::HashMap, sync::Arc};

use crate::forward::info::{ForwardInfo, ReforwardInfo};
use crate::result::Result;
use crate::storage::Storage;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::dto::req::ChangeResourceReq;
use crate::forward::info::Layer;
use crate::forward::PeerForward;
use crate::room::config::ManagerConfig;
use crate::{metrics, AppError};

use super::config::MetaData;

#[derive(Clone)]
pub struct Manager {
    room_map: Arc<RwLock<HashMap<String, PeerForward>>>,
    config: ManagerConfig,
}

pub type Response = (RTCSessionDescription, String);

impl Manager {
    pub async fn new(cfg: ManagerConfig) -> Self {
        let room_map: Arc<RwLock<HashMap<String, PeerForward>>> = Default::default();
        tokio::spawn(Self::heartbeat_and_check_tick(
            room_map.clone(),
            cfg.meta_data.clone(),
            cfg.publish_leave_timeout,
            cfg.storage.clone(),
        ));
        Manager {
            room_map,
            config: cfg,
        }
    }

    async fn heartbeat_and_check_tick(
        room_map: Arc<RwLock<HashMap<String, PeerForward>>>,
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
            let room_map_read = room_map.read().await;
            let mut remove_rooms = vec![];
            for (room, forward) in room_map_read.iter() {
                let forward_info = forward.info().await;
                if forward_info.publish_leave_time > 0
                    && Utc::now().timestamp_millis() - forward_info.publish_leave_time
                        > publish_leave_timeout_i64
                {
                    remove_rooms.push(room.clone());
                }
            }
            if remove_rooms.is_empty() {
                continue;
            }
            drop(room_map_read);
            let mut room_map = room_map.write().await;
            for room in remove_rooms.iter() {
                if let Some(forward) = room_map.get(room) {
                    let forward_info = forward.info().await;
                    if forward_info.publish_leave_time > 0
                        && Utc::now().timestamp_millis() - forward_info.publish_leave_time
                            > publish_leave_timeout_i64
                    {
                        let _ = forward.close().await;
                        room_map.remove(room);
                        metrics::ROOM.dec();
                        let publish_leave_time =
                            DateTime::from_timestamp_millis(forward_info.publish_leave_time)
                                .unwrap()
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string();
                        info!(
                            "room : {}, publish leave timeout, publish leave time : {}",
                            room, publish_leave_time
                        );
                        let _ = Self::unregister_room(&storage, room.clone()).await;
                    }
                }
            }
        }
    }

    pub async fn publish(&self, room: String, offer: RTCSessionDescription) -> Result<Response> {
        let room_map = self.room_map.read().await;
        let forward = room_map.get(&room).cloned();
        drop(room_map);
        if let Some(forward) = forward {
            forward.set_publish(offer).await
        } else {
            if metrics::ROOM.get() >= self.config.meta_data.pub_max as f64 {
                return Err(AppError::LackOfResources);
            }
            let forward = PeerForward::new(room.clone(), self.config.ice_servers.clone());
            let (sdp, session) = forward.set_publish(offer).await?;
            let mut room_map = self.room_map.write().await;
            if room_map.contains_key(&room) {
                let _ = forward.close().await;
                return Err(AppError::resource_already_exists("resource already exists"));
            }
            if room_map.len() >= self.config.meta_data.pub_max as usize {
                warn!("room {} set publish ok,but exceeded the limit", room);
                let _ = forward.close().await;
                return Err(AppError::LackOfResources);
            }
            info!("add room : {}", room);
            room_map.insert(room.clone(), forward);
            metrics::ROOM.inc();
            Self::registry_room(&self.config.storage, room.clone()).await?;
            Ok((sdp, session))
        }
    }

    pub async fn subscribe(&self, room: String, offer: RTCSessionDescription) -> Result<Response> {
        if metrics::SUBSCRIBE.get() >= self.config.meta_data.sub_max as f64 {
            return Err(AppError::LackOfResources);
        }
        let room_map = self.room_map.read().await;
        let forward = room_map.get(&room).cloned();
        drop(room_map);
        if let Some(forward) = forward {
            let (sdp, session) = forward.add_subscribe(offer).await?;
            if metrics::SUBSCRIBE.get() > self.config.meta_data.sub_max as f64 {
                warn!("room {} add subscribe ok,but exceeded the limit", room);
                let _ = forward.remove_peer(session).await;
                Err(AppError::LackOfResources)
            } else {
                Ok((sdp, session))
            }
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    async fn registry_room(
        storage: &Option<Arc<Box<dyn Storage + 'static + Send + Sync>>>,
        room: String,
    ) -> Result<()> {
        if let Some(storage) = storage {
            storage.registry_room(room.clone()).await?;
        }
        Ok(())
    }

    async fn unregister_room(
        storage: &Option<Arc<Box<dyn Storage + 'static + Send + Sync>>>,
        room: String,
    ) -> Result<()> {
        if let Some(storage) = storage {
            storage.unregister_room(room.clone()).await?;
        }
        Ok(())
    }

    pub async fn add_ice_candidate(
        &self,
        room: String,
        session: String,
        ice_candidates: String,
    ) -> Result<()> {
        let rooms = self.room_map.read().await;
        let forward = rooms.get(&room).cloned();
        drop(rooms);
        if let Some(forward) = forward {
            forward.add_ice_candidate(session, ice_candidates).await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn remove_room_session(&self, room: String, session: String) -> Result<()> {
        let rooms = self.room_map.read().await;
        let forward = rooms.get(&room).cloned();
        drop(rooms);
        if let Some(forward) = forward {
            let is_publish = forward.remove_peer(session.clone()).await?;
            if is_publish {
                let mut room_map = self.room_map.write().await;
                info!("remove room : {}", room);
                room_map.remove(&room);
                metrics::ROOM.dec();
                let _ = Self::unregister_room(&self.config.storage, room.clone()).await;
            }
        }
        Ok(())
    }

    pub async fn layers(&self, room: String) -> Result<Vec<Layer>> {
        let room_map = self.room_map.read().await;
        let forward = room_map.get(&room).cloned();
        drop(room_map);
        if let Some(forward) = forward {
            forward.layers().await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn select_layer(
        &self,
        room: String,
        session: String,
        layer: Option<Layer>,
    ) -> Result<()> {
        let room_map = self.room_map.read().await;
        let forward = room_map.get(&room).cloned();
        drop(room_map);
        if let Some(forward) = forward {
            forward.select_layer(session, layer).await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn change_resource(
        &self,
        room: String,
        session: String,
        change_resource: ChangeResourceReq,
    ) -> Result<()> {
        let room_map = self.room_map.read().await;
        let forward = room_map.get(&room).cloned();
        drop(room_map);
        if let Some(forward) = forward {
            forward.change_resource(session, change_resource).await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn info(&self, rooms: Vec<String>) -> Vec<ForwardInfo> {
        let mut rooms = rooms.clone();
        rooms.retain(|room| !room.trim().is_empty());
        let mut resp = vec![];
        let room_map = self.room_map.read().await;
        for (room, forward) in room_map.iter() {
            if rooms.is_empty() || rooms.contains(room) {
                resp.push(forward.info().await);
            }
        }
        resp
    }

    pub async fn reforward(&self, room: String, reforward_info: ReforwardInfo) -> Result<()> {
        let rooms = self.room_map.read().await;
        let forward = rooms.get(&room).cloned();
        drop(rooms);
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
