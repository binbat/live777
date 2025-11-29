use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use md5::{Digest, Md5};
use rtsp_types::headers;
use url::Url;

#[derive(Debug, Clone)]
pub struct AuthParams {
    pub username: String,
    pub password: String,
}

impl AuthParams {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }

    pub fn from_url(url: &Url) -> Option<Self> {
        let username = url.username();
        let password = url.password();

        if username.is_empty() {
            return None;
        }

        Some(Self {
            username: username.to_string(),
            password: password.unwrap_or("").to_string(),
        })
    }

    pub fn generate_digest_response(
        &self,
        realm: &str,
        nonce: &str,
        uri: &str,
        method: &str,
    ) -> String {
        generate_digest_response(&self.username, &self.password, uri, realm, nonce, method)
    }

    pub fn generate_basic_auth(&self) -> String {
        let credentials = format!("{}:{}", self.username, self.password);
        format!("Basic {}", BASE64.encode(credentials.as_bytes()))
    }
}

pub fn generate_digest_response(
    username: &str,
    password: &str,
    uri: &str,
    realm: &str,
    nonce: &str,
    method: &str,
) -> String {
    let ha1 = format!("{}:{}:{}", username, realm, password);
    let ha1_hash = format!("{:x}", Md5::digest(ha1.as_bytes()));

    let ha2 = format!("{}:{}", method, uri);
    let ha2_hash = format!("{:x}", Md5::digest(ha2.as_bytes()));

    let response = format!("{}:{}:{}", ha1_hash, nonce, ha2_hash);
    format!("{:x}", Md5::digest(response.as_bytes()))
}

pub fn parse_auth_header(header: &headers::HeaderValue) -> Result<(String, String)> {
    let header_str = header.as_str();

    let params_str = header_str
        .strip_prefix("Digest ")
        .unwrap_or(header_str)
        .trim();

    let mut realm = String::new();
    let mut nonce = String::new();

    for part in params_str.split(',') {
        let part = part.trim();

        if let Some((key, value)) = part.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');

            match key {
                "realm" => {
                    realm = value.to_string();
                    tracing::trace!("Found realm: {}", realm);
                }
                "nonce" => {
                    nonce = value.to_string();
                    tracing::trace!("Found nonce: {}", nonce);
                }
                _ => {
                    tracing::trace!("Ignoring parameter: {}={}", key, value);
                }
            }
        }
    }

    if realm.is_empty() {
        return Err(anyhow!(
            "Missing 'realm' in WWW-Authenticate header: {}",
            header_str
        ));
    }

    if nonce.is_empty() {
        return Err(anyhow!(
            "Missing 'nonce' in WWW-Authenticate header: {}",
            header_str
        ));
    }

    tracing::debug!(
        "Successfully parsed - realm: '{}', nonce length: {}",
        realm,
        nonce.len()
    );
    Ok((realm, nonce))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_params_from_url() {
        let url = Url::parse("rtsp://user:pass@example.com/stream").unwrap();
        let auth = AuthParams::from_url(&url).unwrap();
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "pass");
    }

    #[test]
    fn test_basic_auth() {
        let auth = AuthParams {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let header = auth.generate_basic_auth();
        assert!(header.starts_with("Basic "));
    }

    #[test]
    fn test_digest_response() {
        let response =
            generate_digest_response("user", "pass", "/stream", "RTSP", "abc123", "DESCRIBE");
        assert!(!response.is_empty());
    }
}
