use anyhow::Result;
use reqwest::{
    header::{self, HeaderMap, HeaderValue},
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
    pub url: String,
    pub session_url: Option<String>,
    pub default_headers: HeaderMap,
}

impl Client {
    pub fn get_auth_header_map(token: Option<String>) -> Option<HeaderMap> {
        let mut header_map = HeaderMap::new();
        if let Some(auth_token) = token {
            header_map.insert(
                header::AUTHORIZATION,
                format!("Bearer {auth_token}").parse().unwrap(),
            );
            Some(header_map)
        } else {
            None
        }
    }

    pub fn get_authorization_header_map(authorization: Option<String>) -> Option<HeaderMap> {
        authorization.map(|authorization| {
            let mut header_map = HeaderMap::new();
            header_map.insert(header::AUTHORIZATION, authorization.parse().unwrap());
            header_map
        })
    }

    pub fn new(url: String, defulat_headers: Option<HeaderMap>) -> Self {
        Client {
            url,
            session_url: None,
            default_headers: defulat_headers.unwrap_or_default(),
        }
    }

    pub fn build(
        url: String,
        session_url: Option<String>,
        defulat_headers: Option<HeaderMap>,
    ) -> Self {
        Client {
            url,
            session_url,
            default_headers: defulat_headers.unwrap_or_default(),
        }
    }

    pub async fn wish(
        &mut self,
        sdp: String,
    ) -> Result<(RTCSessionDescription, Vec<RTCIceServer>)> {
        let mut header_map = self.default_headers.clone();
        header_map.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str("application/sdp")?,
        );
        let response = request(self.url.clone(), "POST", header_map, sdp).await?;
        if response.status() != StatusCode::CREATED {
            return Err(anyhow::anyhow!(get_response_error(response).await));
        }
        let session_url = response
            .headers()
            .get(header::LOCATION)
            .ok_or_else(|| anyhow::anyhow!("Response missing location header"))?
            .to_str()?
            .to_owned();
        let mut url = Url::parse(self.url.as_str())?;
        match Url::parse(session_url.as_str()) {
            Ok(url) => {
                self.session_url = Some(url.into());
            }
            Err(_) => {
                url.set_path(session_url.as_str());
                self.session_url = Some(url.into());
            }
        }
        let ice_servers = Self::parse_ide_servers(&response)?;
        let sdp =
            RTCSessionDescription::answer(String::from_utf8(response.bytes().await?.to_vec())?)?;
        Ok((sdp, ice_servers))
    }

    fn parse_ide_servers(response: &Response) -> Result<Vec<RTCIceServer>> {
        let links = response.headers().get_all(header::LINK);
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
                    urls: vec![link.raw_uri.to_string().replacen("://", ":", 1)],
                    username: link.params.remove("username").unwrap_or("".to_owned()),
                    credential: link.params.remove("credential").unwrap_or("".to_owned()),
                    credential_type: link
                        .params
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
        let session_url = self
            .session_url
            .clone()
            .ok_or(anyhow::anyhow!("there is no resource url"))?;
        let header_map = self.default_headers.clone();
        let response = request(session_url, "DELETE", header_map, "").await?;
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

#[cfg(test)]
mod tests {
    use http::header;
    use http::response::Builder;
    use reqwest::Response;
    use webrtc::ice_transport::ice_credential_type::RTCIceCredentialType;

    use crate::Client;

    #[test]
    fn test_from_http_response() {
        let response = Builder::new()
            .header(header::LINK, r#"<stun:stun.22333.fun>; rel="ice-server""#)
            .header(header::LINK, r#"<stun:stun.l.google.com:19302>; rel="ice-server""#)
            .header(header::LINK, r#"<turn:turn.22333.fun>; rel="ice-server"; username="live777"; credential="live777"; credential-type="password""#)
            .body("")
            .unwrap();
        let response = Response::from(response);

        let ice_servers = Client::parse_ide_servers(&response).unwrap();
        assert_eq!(ice_servers.len(), 3);
        assert_eq!(
            ice_servers.first().unwrap().urls.first().unwrap(),
            "stun:stun.22333.fun"
        );
        assert_eq!(
            ice_servers.get(1).unwrap().urls.first().unwrap(),
            "stun:stun.l.google.com:19302"
        );

        assert_eq!(
            ice_servers.get(2).unwrap().urls.first().unwrap(),
            "turn:turn.22333.fun"
        );
        assert_eq!(ice_servers.get(2).unwrap().username, "live777");
        assert_eq!(ice_servers.get(2).unwrap().credential, "live777");
        assert_eq!(
            ice_servers.get(2).unwrap().credential_type,
            RTCIceCredentialType::Password
        );

        println!("{ice_servers:?}");
    }
}
