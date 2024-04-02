use std::time::Duration;
use std::vec;
use std::{collections::HashMap, sync::Arc};

use crate::forward::info::{ForwardInfo, ReForwardInfo};
use crate::result::Result;
use crate::storage::{ClusterStorage, RoomOwnership};
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::info;
use webrtc::{
    ice_transport::ice_server::RTCIceServer,
    peer_connection::sdp::session_description::RTCSessionDescription,
};

use crate::dto::req::ChangeResourceReq;
use crate::forward::info::Layer;
use crate::forward::PeerForward;
use crate::AppError;

#[derive(Clone)]
pub struct Manager {
    ice_servers: Vec<RTCIceServer>,
    paths: Arc<RwLock<HashMap<String, PeerForward>>>,
    storage: Option<Arc<Box<dyn ClusterStorage + 'static + Send + Sync>>>,
}

pub type Response = (RTCSessionDescription, String);

impl Manager {
    pub async fn new(
        ice_servers: Vec<RTCIceServer>,
        publish_leave_timeout: u64,
        storage: Option<Arc<Box<dyn ClusterStorage + 'static + Send + Sync>>>,
    ) -> Self {
        let paths: Arc<RwLock<HashMap<String, PeerForward>>> = Default::default();
        tokio::spawn(Self::heartbeat_and_check_tick(
            paths.clone(),
            publish_leave_timeout,
            storage.clone(),
        ));
        Manager {
            ice_servers,
            paths,
            storage,
        }
    }

    async fn heartbeat_and_check_tick(
        paths: Arc<RwLock<HashMap<String, PeerForward>>>,
        publish_leave_timeout: u64,
        storage: Option<Arc<Box<dyn ClusterStorage + 'static + Send + Sync>>>,
    ) {
        let publish_leave_timeout_i64: i64 = publish_leave_timeout.try_into().unwrap();
        loop {
            let timeout = tokio::time::sleep(Duration::from_millis(1000));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
            if let Some(storage) = &storage {
                // TODO metadata
                let _ = storage.registry("".to_string()).await;
            }
            let paths_read = paths.read().await;
            let mut remove_paths = vec![];
            for (path, forward) in paths_read.iter() {
                let forward_info = forward.info().await;
                if forward_info.publish_leave_time > 0
                    && Utc::now().timestamp_millis() - forward_info.publish_leave_time
                        > publish_leave_timeout_i64
                {
                    remove_paths.push(path.clone());
                }
            }
            if remove_paths.is_empty() {
                continue;
            }
            drop(paths_read);
            let mut paths = paths.write().await;
            for path in remove_paths.iter() {
                if let Some(forward) = paths.get(path) {
                    let forward_info = forward.info().await;
                    if forward_info.publish_leave_time > 0
                        && Utc::now().timestamp_millis() - forward_info.publish_leave_time
                            > publish_leave_timeout_i64
                    {
                        let _ = forward.close().await;
                        paths.remove(path);
                        let publish_leave_time =
                            DateTime::from_timestamp_millis(forward_info.publish_leave_time)
                                .unwrap()
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string();
                        info!(
                            "path : {} publish leave timeout, publish leave time : {}",
                            path, publish_leave_time
                        );
                        let _ = Self::unregister_room(&storage, path.clone()).await;
                    }
                }
            }
        }
    }

    pub async fn publish(&self, path: String, offer: RTCSessionDescription) -> Result<Response> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            forward.set_publish(offer).await
        } else {
            Self::check_room_ownership(&self.storage, path.clone()).await?;
            let forward = PeerForward::new(path.clone(), self.ice_servers.clone());
            let (sdp, key) = forward.set_publish(offer).await?;
            let mut paths = self.paths.write().await;
            if paths.contains_key(&path) {
                return Err(AppError::resource_already_exists("resource already exists"));
            }
            info!("add path : {}", path);
            paths.insert(path.clone(), forward);
            Self::registry_room(&self.storage, path.clone()).await?;
            Ok((sdp, key))
        }
    }

    pub async fn subscribe(&self, path: String, offer: RTCSessionDescription) -> Result<Response> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            forward.add_subscribe(offer).await
        } else {
            Err(AppError::resource_not_fount(
                "The requested resource not exist,please check the path and try again.",
            ))
        }
    }

    async fn check_room_ownership(
        storage: &Option<Arc<Box<dyn ClusterStorage + 'static + Send + Sync>>>,
        path: String,
    ) -> Result<()> {
        if let Some(storage) = storage {
            if let RoomOwnership::Other(node_ip) = storage.room_ownership(path.clone()).await? {
                return Err(AppError::throw(format!(
                    "path {} at this node {}",
                    path, node_ip
                )));
            }
        }
        Ok(())
    }

    async fn registry_room(
        storage: &Option<Arc<Box<dyn ClusterStorage + 'static + Send + Sync>>>,
        path: String,
    ) -> Result<()> {
        if let Some(storage) = storage {
            if let RoomOwnership::Other(node_ip) = storage.room_ownership(path.clone()).await? {
                return Err(AppError::throw(format!(
                    "room {} at this node {}",
                    path, node_ip
                )));
            } else {
                storage.registry_room(path.clone()).await?;
            }
        }
        Ok(())
    }

    async fn unregister_room(
        storage: &Option<Arc<Box<dyn ClusterStorage + 'static + Send + Sync>>>,
        path: String,
    ) -> Result<()> {
        if let Some(storage) = storage {
            if let RoomOwnership::Other(node_ip) = storage.room_ownership(path.clone()).await? {
                return Err(AppError::throw(format!(
                    "room {} at this node {}",
                    path, node_ip
                )));
            } else {
                storage.unregister_room(path.clone()).await?;
            }
        }
        Ok(())
    }

    pub async fn add_ice_candidate(
        &self,
        path: String,
        key: String,
        ice_candidates: String,
    ) -> Result<()> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            forward.add_ice_candidate(key, ice_candidates).await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn remove_path_key(&self, path: String, key: String) -> Result<()> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            let is_publish = forward.remove_peer(key.clone()).await?;
            if is_publish {
                let mut paths = self.paths.write().await;
                info!("remove path : {}", path);
                paths.remove(&path);
                let _ = Self::unregister_room(&self.storage, path.clone()).await;
            }
        }
        Ok(())
    }

    pub async fn layers(&self, path: String) -> Result<Vec<Layer>> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            forward.layers().await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn select_layer(
        &self,
        path: String,
        key: String,
        layer: Option<Layer>,
    ) -> Result<()> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            forward.select_layer(key, layer).await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn change_resource(
        &self,
        path: String,
        key: String,
        change_resource: ChangeResourceReq,
    ) -> Result<()> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            forward.change_resource(key, change_resource).await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }

    pub async fn info(&self, paths: Vec<String>) -> Vec<ForwardInfo> {
        let mut paths = paths.clone();
        paths.retain(|path| !path.trim().is_empty());
        let mut resp = vec![];
        let path_forwards = self.paths.read().await;
        for (path, forward) in path_forwards.iter() {
            if paths.is_empty() || paths.contains(path) {
                resp.push(forward.info().await);
            }
        }
        resp
    }

    pub async fn re_forward(&self, path: String, re_forward_info: ReForwardInfo) -> Result<()> {
        let paths = self.paths.read().await;
        let forward = paths.get(&path).cloned();
        drop(paths);
        if let Some(forward) = forward {
            forward.re_forward(re_forward_info).await
        } else {
            Err(AppError::resource_not_fount("resource not exists"))
        }
    }
}
