use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
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
    pub fn new(ice_servers: Vec<RTCIceServer>) -> Self {
        Manager {
            ice_servers,
            paths: Default::default(),
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
                return Err(anyhow::anyhow!("resource already exists"));
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
            Err(AppError::ResourceNotFound(
                ("The requested resource not exist,please check the path and try again.")
                    .to_string(),
            )
            .into())
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
            Err(anyhow::anyhow!("resource not exists"))
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
            Err(anyhow::anyhow!("resource not exists"))
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
            Err(anyhow::anyhow!("resource not exists"))
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
            Err(anyhow::anyhow!("resource not exists"))
        }
    }
}
