use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Body, Method, Response, StatusCode,
};
use std::str::FromStr;
use url::Url;
use webrtc::{
    ice_transport::ice_server::RTCIceServer,
    peer_connection::sdp::session_description::RTCSessionDescription,
};

#[derive(Clone)]
pub struct Client {
    url: String,
    resource_url: Option<String>,
    default_headers: HeaderMap,
}

impl Client {
    pub fn get_auth_header_map(basic: Option<String>, token: Option<String>) -> Option<HeaderMap> {
        let mut header_map = HeaderMap::new();
        if let Some(auth_basic) = basic {
            let encoded = STANDARD.encode(auth_basic);
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
            resource_url: None,
            default_headers: defulat_headers.unwrap_or_default(),
        }
    }

    pub async fn wish(
        &mut self,
        sdp: String,
    ) -> Result<(RTCSessionDescription, Vec<RTCIceServer>)> {
        let mut header_map = self.default_headers.clone();
        header_map.insert("Content-Type", HeaderValue::from_str("application/sdp")?);
        let response = request(self.url.clone(), "POST", header_map, sdp).await?;
        if response.status() != StatusCode::CREATED {
            return Err(anyhow::anyhow!(get_response_error(response).await));
        }
        let resource_url = response
            .headers()
            .get("location")
            .ok_or_else(|| anyhow::anyhow!("Response missing location header"))?
            .to_str()?
            .to_owned();
        let mut url = Url::parse(self.url.as_str())?;
        match Url::parse(resource_url.as_str()) {
            Ok(url) => {
                self.resource_url = Some(url.into());
            }
            Err(_) => {
                url.set_path(resource_url.as_str());
                self.resource_url = Some(url.into());
            }
        }
        let ice_servers = Self::parse_ide_servers(&response)?;
        let sdp =
            RTCSessionDescription::answer(String::from_utf8(response.bytes().await?.to_vec())?)?;
        Ok((sdp, ice_servers))
    }

    fn parse_ide_servers(response: &Response) -> Result<Vec<RTCIceServer>> {
        let links = response.headers().get_all("Link");
        let mut ice_servers = vec![];
        for link in links {
            let mut link = link.to_str()?.to_owned();
            link = link.replacen(':', "://", 1);
            let link_header = parse_link_header::parse_with_rel(&link)?;
            for (rel, mut link) in link_header {
                if &rel != "ice-server" {
                    continue;
                }
                ice_servers.push(RTCIceServer {
                    urls: vec![link.uri.to_string().replacen("://", ":", 1)],
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
        Ok(ice_servers)
    }

    pub async fn remove_resource(&self) -> Result<()> {
        let resource_url = self
            .resource_url
            .clone()
            .ok_or(anyhow::anyhow!("there is no resource url"))?;
        let header_map = self.default_headers.clone();
        let response = request(resource_url, "DELETE", header_map, "").await?;
        if response.status() != StatusCode::NO_CONTENT {
            Err(anyhow::anyhow!(get_response_error(response).await))
        } else {
            Ok(())
        }
    }
}

async fn get_response_error(response: Response) -> String {
    format!(
        "[HTTP] {}\n==> Body BEGIN\n{}\n==> Body END",
        response.status(),
        response.text().await.unwrap(),
    )
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
