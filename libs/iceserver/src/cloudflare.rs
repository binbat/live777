use anyhow::{anyhow, Result};
use http::header;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::IceServer;

#[derive(Debug, Serialize, Deserialize)]
struct IceServersResponse {
    #[serde(rename = "iceServers")]
    pub ice_servers: Vec<IceServer>,
}

/// https://developers.cloudflare.com/realtime/turn/generate-credentials/
pub async fn request_iceserver(
    key_id: String,
    api_token: String,
    ttl: u64,
) -> Result<Vec<IceServer>> {
    let url = format!(
        "https://rtc.live.cloudflare.com/v1/turn/keys/{}/credentials/generate-ice-servers",
        key_id
    );

    let client = Client::new();
    let response = client
        .post(&url)
        .header(header::AUTHORIZATION, format!("Bearer {}", api_token))
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({
            "ttl": ttl
        }))
        .send()
        .await?;

    if response.status().is_success() {
        let ice_servers_response: IceServersResponse = response.json().await?;
        Ok(ice_servers_response.ice_servers)
    } else {
        let status = response.status();
        let error_text = response.text().await?;
        Err(anyhow!(
            "Cloudflare request failed status: {}, {}",
            status,
            error_text
        ))
    }
}
