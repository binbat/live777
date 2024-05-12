use std::{cmp::Ordering, str::FromStr, time::Duration};

use crate::{error::AppError, result::Result};
use anyhow::anyhow;
use chrono::{serde::ts_milliseconds, DateTime, Utc};
use live777_http::{
    path,
    request::{QueryInfo, Reforward},
    response::StreamInfo,
};
use reqwest::{header::HeaderMap, Body, Method};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

#[derive(Serialize, Deserialize, Clone, Debug, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: u64,
    pub addr: String,
    pub authorization: Option<String>,
    pub admin_authorization: Option<String>,
    pub pub_max: u64,
    pub sub_max: u64,
    pub reforward_maximum_idle_time: u64,
    pub reforward_cascade: bool,
    pub stream: u64,
    pub publish: u64,
    pub subscribe: u64,
    pub reforward: u64,
    #[serde(with = "ts_milliseconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts_milliseconds")]
    pub updated_at: DateTime<Utc>,
}

impl Node {
    pub fn active_time_point() -> DateTime<Utc> {
        Utc::now() - Duration::from_millis(10000)
    }

    pub fn deactivate_time() -> DateTime<Utc> {
        DateTime::from_timestamp_millis(0).unwrap()
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.addr == other.addr
    }
}

impl PartialOrd<Node> for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for Node {}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.sub_max - self.subscribe).cmp(&(other.sub_max - other.subscribe))
    }
}

impl Default for Node {
    fn default() -> Self {
        Self {
            id: Default::default(),
            addr: "0.0.0.0:0".to_string(),
            authorization: Default::default(),
            admin_authorization: Default::default(),
            pub_max: Default::default(),
            sub_max: Default::default(),
            reforward_maximum_idle_time: Default::default(),
            reforward_cascade: Default::default(),
            stream: Default::default(),
            publish: Default::default(),
            subscribe: Default::default(),
            reforward: Default::default(),
            created_at: Default::default(),
            updated_at: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub id: u64,
    pub stream: String,
    pub addr: String,
    pub publish: u64,
    pub subscribe: u64,
    pub reforward: u64,
    #[serde(with = "ts_milliseconds")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts_milliseconds")]
    pub updated_at: DateTime<Utc>,
}

impl Default for Stream {
    fn default() -> Self {
        Self {
            id: Default::default(),
            stream: Default::default(),
            addr: "0.0.0.0:0".to_string(),
            publish: Default::default(),
            subscribe: Default::default(),
            reforward: Default::default(),
            created_at: Default::default(),
            updated_at: Default::default(),
        }
    }
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
    pub fn available(&self, pub_check: bool) -> bool {
        (!pub_check || self.stream < self.pub_max) && self.subscribe < self.sub_max
    }

    pub async fn stream_info(&self, stream: String) -> Result<Option<StreamInfo>> {
        let streams_info = self.stream_infos(vec![stream]).await?;
        Ok(streams_info.first().cloned())
    }

    pub async fn stream_infos(&self, streams: Vec<String>) -> Result<Vec<StreamInfo>> {
        let data = request(
            self.path_url(&path::infos(QueryInfo { streams })),
            "GET",
            self.admin_authorization.clone(),
            "",
        )
        .await?;
        serde_json::from_str::<Vec<StreamInfo>>(&data).map_err(|e| e.into())
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
            self.admin_authorization.clone(),
            serde_json::to_string(&Reforward {
                target_url: target_node.path_url(&path::whip(&target_stream)),
                admin_authorization: target_node.admin_authorization.clone(),
            })?,
        )
        .await;
        Ok(())
    }

    pub async fn resource_delete(&self, stream: String, session: String) -> Result<()> {
        let _ = request(
            self.path_url(&path::resource(&stream, &session)),
            "DELETE",
            self.admin_authorization.clone(),
            "",
        )
        .await?;
        Ok(())
    }
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
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_millis(5000))
        .build()?;
    let response = client
        .request(Method::from_str(method)?, url)
        .headers(headers)
        .body(body)
        .send()
        .await?;
    let success = response.status().is_success();
    let body = response.text().await?;
    if !success {
        return Err(AppError::InternalServerError(anyhow!(body)));
    }
    Ok(body)
}
