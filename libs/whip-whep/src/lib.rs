use std::str::FromStr;

use anyhow::Result;

use reqwest::{
    header::{HeaderMap, HeaderValue},
    Body, Method, Response,
};
use webrtc::{
    ice_transport::ice_server::RTCIceServer,
    peer_connection::sdp::session_description::RTCSessionDescription,
};

pub async fn get_answer(url: String, sdp: String) -> Result<(RTCSessionDescription, String)> {
    let mut header_map = HeaderMap::new();
    header_map.insert("Content-Type", HeaderValue::from_str("application/sdp")?);
    let response = request(url, "POST", header_map, sdp).await?;
    if response.status() != 201 {
        return Err(anyhow::anyhow!("get answer error"));
    }
    let etag = response
        .headers()
        .get("E-Tag")
        .ok_or_else(|| anyhow::anyhow!("response no E-Tag header"))?
        .to_str()?
        .to_owned();
    let sdp = RTCSessionDescription::answer(String::from_utf8(response.bytes().await?.to_vec())?)?;
    Ok((sdp, etag))
}

pub async fn get_ide_servers(url: String) -> Result<Vec<RTCIceServer>> {
    let respone = request(url, "OPTIONS", Default::default(), "").await?;
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

pub async fn remove_resource(url: String, key: String) -> Result<()> {
    let mut header_map = HeaderMap::new();
    header_map.insert("If-Match", HeaderValue::from_str(&key)?);
    let response = request(url, "DELETE", header_map, "").await?;
    if response.status() != 204 {
        return Err(anyhow::anyhow!("response statsu not is 204"));
    }
    Ok(())
}

pub async fn request<T: Into<Body>>(
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
