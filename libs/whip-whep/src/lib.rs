use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Body, Method, Response,
};
use std::str::FromStr;
use webrtc::{
    ice_transport::ice_server::RTCIceServer,
    peer_connection::sdp::session_description::RTCSessionDescription,
};
#[derive(Clone)]
pub struct Client {
    url: String,
    defulat_headers: HeaderMap,
}

impl Client {
    pub fn get_auth_header_map(
        account: Option<String>,
        token: Option<String>,
    ) -> Option<HeaderMap> {
        let mut header_map = HeaderMap::new();
        if let Some(auth_account) = account {
            let encoded = STANDARD.encode(auth_account);
            header_map.insert(
                "Authorization",
                format!("Basic {}", encoded).parse().unwrap(),
            );
            Some(header_map)
        } else if let Some(auth_token) = token {
            header_map.insert(
                "Authorization",
                format!("Bearer {}", auth_token).parse().unwrap(),
            );
            Some(header_map)
        } else {
            None
        }
    }

    pub fn new(url: String, defulat_headers: Option<HeaderMap>) -> Self {
        Client {
            url,
            defulat_headers: defulat_headers.unwrap_or(Default::default()),
        }
    }

    pub async fn get_answer(&self, sdp: String) -> Result<(RTCSessionDescription, String)> {
        let mut header_map = self.defulat_headers.clone();
        header_map.insert("Content-Type", HeaderValue::from_str("application/sdp")?);
        let response = request(self.url.clone(), "POST", header_map, sdp).await?;
        if response.status() != 201 {
            return Err(anyhow::anyhow!("get answer error"));
        }
        let etag = response
            .headers()
            .get("E-Tag")
            .ok_or_else(|| anyhow::anyhow!("response no E-Tag header"))?
            .to_str()?
            .to_owned();
        let sdp =
            RTCSessionDescription::answer(String::from_utf8(response.bytes().await?.to_vec())?)?;
        Ok((sdp, etag))
    }

    pub async fn get_ide_servers(&self) -> Result<Vec<RTCIceServer>> {
        let respone = request(
            self.url.clone(),
            "OPTIONS",
            self.defulat_headers.clone(),
            "",
        )
        .await?;
        if respone.status() != 204 {
            return Err(anyhow::anyhow!("get ide servers error"));
        }
        let links = respone.headers().get_all("Link");
        let mut _ice_servers = vec![];
        for link in links {
            let link_header = parse_link_header::parse_with_rel(link.to_str()?)?;
            for (rel, mut link) in link_header {
                if &rel != "ice-server" {
                    continue;
                }
                _ice_servers.push(RTCIceServer {
                    urls: vec![link
                        .uri
                        .to_string()
                        .replacen("://", ":", 1)
                        .replace("/", "")],
                    username: link.queries.remove("username").unwrap_or("".to_owned()),
                    credential: link.queries.remove("credential").unwrap_or("".to_owned()),
                    credential_type: link
                        .queries
                        .remove("credential-type")
                        .unwrap_or("".to_owned())
                        .as_str()
                        .into(),
                })
            }
        }
        Ok(_ice_servers)
    }

    pub async fn remove_resource(&self, key: String) -> Result<()> {
        let mut header_map = self.defulat_headers.clone();
        header_map.insert("If-Match", HeaderValue::from_str(&key)?);
        let response = request(self.url.clone(), "DELETE", header_map, "").await?;
        if response.status() != 204 {
            return Err(anyhow::anyhow!("response statsu not is 204"));
        }
        Ok(())
    }
}

async fn request<T: Into<Body>>(
    url: String,
    method: &str,
    headers: HeaderMap,
    body: T,
) -> Result<Response> {
    let client = reqwest::Client::new();
    client
        .request(Method::from_str(method)?, url)
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(|e| e.into())
}
