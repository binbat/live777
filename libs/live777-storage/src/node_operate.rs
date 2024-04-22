use std::{collections::HashMap, str::FromStr, sync::Arc};

use anyhow::Result;
use live777_http::{
    path,
    request::Reforward,
    response::{Metrics, StreamInfo},
};
use reqwest::{header::HeaderMap, Body, Method};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::NodeMetaData;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Node {
    pub addr: String,
    pub metadata: NodeMetaData,
}

trait NodeUrl {
    fn path_url(&self, path: &str) -> String;
}

impl NodeUrl for Node {
    fn path_url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Node {
    pub async fn stream_info(&self, stream: String) -> Result<Option<StreamInfo>> {
        let streams_info = self.stream_infos(vec![stream]).await?;
        Ok(streams_info.first().cloned())
    }

    pub async fn stream_infos(&self, streams: Vec<String>) -> Result<Vec<StreamInfo>> {
        let data = request(
            self.path_url(&path::infos(streams)),
            "GET",
            self.metadata.admin_authorization.clone(),
            "",
        )
        .await?;
        serde_json::from_str::<Vec<StreamInfo>>(&data).map_err(|e| e.into())
    }

    pub async fn metrics(&self) -> Result<Metrics> {
        let data = request(self.path_url(path::METRICS_JSON), "GET", None, "").await?;
        serde_json::from_str::<Metrics>(&data).map_err(|e| e.into())
    }

    pub async fn reforward(
        &self,
        target_node: &Node,
        node_stream: String,
        target_stream: String,
    ) -> Result<()> {
        let _ = request(
            self.path_url(&path::reforward(&node_stream)),
            "POST",
            self.metadata.admin_authorization.clone(),
            serde_json::to_string(&Reforward {
                target_url: target_node.path_url(&path::whip(&target_stream)),
                admin_authorization: target_node.metadata.admin_authorization.clone(),
            })?,
        )
        .await;
        Ok(())
    }

    pub async fn resource_delete(&self, stream: String, session: String) -> Result<()> {
        let _ = request(
            self.path_url(&path::resource(&stream, &session)),
            "DELETE",
            self.metadata.admin_authorization.clone(),
            "",
        )
        .await?;
        Ok(())
    }
}

// nodes not empty
pub async fn maximum_idle_node(mut nodes: Vec<Node>, check_pub: bool) -> Result<Option<Node>> {
    let node_metrics_map = Arc::new(node_metrics_map(&nodes).await);
    let node_metrics_map_temp = node_metrics_map.clone();
    nodes.retain(move |node| match node_metrics_map_temp.get(&node.addr) {
        Some(node_metrics) => {
            (!check_pub || node_metrics.stream < node.metadata.pub_max)
                && node_metrics.subscribe < node.metadata.sub_max
        }
        None => false,
    });
    nodes.sort_by(move |a, b| {
        let a_node_available_sub =
            a.metadata.sub_max - node_metrics_map.get(&a.addr).unwrap().subscribe;
        let b_node_available_sub =
            b.metadata.sub_max - node_metrics_map.get(&b.addr).unwrap().subscribe;
        b_node_available_sub.cmp(&a_node_available_sub)
    });
    Ok(nodes.first().cloned())
}

async fn node_metrics_map(nodes: &Vec<Node>) -> HashMap<String, Metrics> {
    let mut node_metrics_map: HashMap<String, Metrics> = HashMap::new();
    for node in nodes {
        match node.metrics().await {
            Ok(metrics) => {
                node_metrics_map.insert(node.addr.clone(), metrics);
            }
            Err(err) => {
                info!("node : {} ,metrics request error : {}", node.addr, err);
            }
        }

        if let Ok(metrics) = node.metrics().await {
            node_metrics_map.insert(node.addr.clone(), metrics);
        };
    }
    node_metrics_map
}

async fn request<T: Into<Body>>(
    url: String,
    method: &str,
    authorization: Option<String>,
    body: T,
) -> Result<String> {
    let mut headers = HeaderMap::new();
    headers.append("Content-Type", "application/json".parse().unwrap());
    if let Some(authorization) = authorization {
        headers.append("Authorization", authorization.parse().unwrap());
    }
    let client = reqwest::Client::new();
    let response = client
        .request(Method::from_str(method)?, url)
        .headers(headers)
        .body(body)
        .send()
        .await?;
    let success = response.status().is_success();
    let body = response.text().await?;
    if !success {
        return Err(anyhow::anyhow!(body));
    }
    Ok(body)
}
