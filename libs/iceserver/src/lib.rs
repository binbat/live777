#[cfg(feature = "cloudflare")]
pub mod cloudflare;

#[cfg(feature = "coturn")]
pub mod coturn;

use rtc::peer_connection::transport::RTCIceServer;
use serde::{Deserialize, Serialize};
use webrtc::error::Error;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IceServer {
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
}

impl IceServer {
    pub(crate) fn parse_url(&self, url_str: &str) -> webrtc::error::Result<rtc_ice::url::Url> {
        Ok(rtc_ice::url::Url::parse_url(url_str)?)
    }

    pub fn validate(&self) -> webrtc::error::Result<()> {
        self.urls()?;
        Ok(())
    }

    fn urls(&self) -> webrtc::error::Result<Vec<rtc_ice::url::Url>> {
        let mut urls = vec![];

        for url_str in &self.urls {
            let mut url = self.parse_url(url_str)?;
            if url.scheme == rtc_ice::url::SchemeType::Turn
                || url.scheme == rtc_ice::url::SchemeType::Turns
            {
                // https://www.w3.org/TR/webrtc/#set-the-configuration (step #11.3.2)
                if self.username.is_empty() || self.credential.is_empty() {
                    return Err(Error::ErrNoTurnCredentials);
                }
                url.username.clone_from(&self.username);
                url.password.clone_from(&self.credential);
            }

            urls.push(url);
        }

        Ok(urls)
    }
}

impl From<IceServer> for RTCIceServer {
    fn from(val: IceServer) -> Self {
        RTCIceServer {
            urls: val.urls,
            username: val.username,
            credential: val.credential,
        }
    }
}

pub fn format_iceserver(urls: Vec<String>, username: String, password: String) -> IceServer {
    IceServer {
        urls,
        username,
        credential: password,
    }
}

pub fn default_ice_servers() -> Vec<IceServer> {
    vec![IceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        username: "".to_string(),
        credential: "".to_string(),
    }]
}

pub fn link_header(ice_servers: Vec<IceServer>) -> Vec<String> {
    ice_servers
        .into_iter()
        .flat_map(|server| {
            let mut username = server.username;
            let mut credential = server.credential;
            if !username.is_empty() {
                username = string_encoder(&username);
                credential = string_encoder(&credential);
            }
            server.urls.into_iter().map(move |url| {
                let mut link = format!("<{url}>; rel=\"ice-server\"");
                if !username.is_empty() {
                    link = format!(
                        "{}; username=\"{}\"; credential=\"{}\"",
                        link, username, credential
                    );
                }
                link
            })
        })
        .collect()
}

fn string_encoder(s: &impl ToString) -> String {
    let s = serde_json::to_string(&s.to_string()).unwrap();
    s[1..s.len() - 1].to_string()
}
