use std::time::Duration;
use std::vec;
use std::{collections::HashMap, sync::Arc};

use crate::forward::info::ForwardInfo;
use crate::result::Result;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::info;
use webrtc::{
    ice_transport::ice_server::RTCIceServer,
    peer_connection::sdp::session_description::RTCSessionDescription,
};

use crate::dto::req::ChangeResource;
use crate::forward::info::Layer;
use crate::forward::PeerForward;
use crate::AppError;

#[derive(Clone)]
pub struct Manager {
    ice_servers: Vec<RTCIceServer>,
    paths: Arc<RwLock<HashMap<String, PeerForward>>>,
}

pub type Response = (RTCSessionDescription, String);

impl Manager {
    pub async fn new(ice_servers: Vec<RTCIceServer>, publish_leave_timeout: u64) -> Self {
        let paths: Arc<RwLock<HashMap<String, PeerForward>>> = Default::default();
        tokio::spawn(Self::publish_leave_timeout_tick(
            paths.clone(),
            publish_leave_timeout,
        ));
        Manager { ice_servers, paths }
    }

    async fn publish_leave_timeout_tick(
        paths: Arc<RwLock<HashMap<String, PeerForward>>>,
        publish_leave_timeout: u64,
    ) {
        let publish_leave_timeout_i64: i64 = publish_leave_timeout.try_into().unwrap();
        loop {
            let timeout = tokio::time::sleep(Duration::from_millis(1000));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
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
            let forward = PeerForward::new(path.clone(), self.ice_servers.clone());
            let (sdp, key) = forward.set_publish(offer).await?;
            let mut paths = self.paths.write().await;
            if paths.contains_key(&path) {
                return Err(AppError::resource_already_exists("resource already exists"));
            }
            info!("add path : {}", path);
            paths.insert(path, forward);
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
        change_resource: ChangeResource,
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
}
