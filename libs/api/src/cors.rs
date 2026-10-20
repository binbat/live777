//! CORS (Cross-Origin Resource Sharing) configuration shared by the HTTP
//! servers (liveion and liveman).

use http::HeaderValue;
use serde::{Deserialize, Serialize};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// CORS allowed origins (`http.cors`).
///
/// TOML form: `cors = ["https://example.com", ...]`.
///
/// - Empty (the default): no CORS headers; browsers block cross-origin
///   access.
/// - An entry of `"*"`: allow any origin.
/// - Otherwise only the listed origins are allowed.
///
/// The list gates origins only: allowed methods, request headers and
/// exposed response headers stay permissive so WHIP/WHEP browser clients
/// keep working (they need the `Authorization` request header and the
/// `Location`/`ETag`/`Link` response headers).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cors(pub Vec<String>);

impl From<Vec<String>> for Cors {
    fn from(origins: Vec<String>) -> Self {
        Cors(origins)
    }
}

impl Cors {
    /// Build the HTTP middleware for this configuration.
    pub fn layer(&self) -> CorsLayer {
        if self.0.iter().any(|o| o.trim() == "*") {
            CorsLayer::permissive()
        } else {
            let list: Vec<HeaderValue> = self
                .0
                .iter()
                .filter_map(|origin| {
                    // Invalid entries are rejected by `validate` at
                    // startup; skip them here so building the layer
                    // can never fail.
                    HeaderValue::from_str(origin.trim())
                        .map_err(|_| {
                            tracing::warn!("http.cors: ignoring invalid origin {origin:?}")
                        })
                        .ok()
                })
                .collect();
            if list.is_empty() {
                CorsLayer::new()
            } else {
                CorsLayer::new()
                    .allow_origin(AllowOrigin::list(list))
                    .allow_methods(Any)
                    .allow_headers(Any)
                    .expose_headers(Any)
            }
        }
    }

    /// Startup validation: every entry must be usable as an HTTP header
    /// value and must include a scheme (the `Origin` header browsers send
    /// always carries one, so a bare host would never match).
    pub fn validate(&self) -> Result<(), String> {
        for origin in &self.0 {
            let origin = origin.trim();
            if origin == "*" {
                continue;
            }
            HeaderValue::from_str(origin).map_err(|e| {
                format!("invalid origin {origin:?}: not a valid header value ({e})")
            })?;
            if !origin.contains("://") {
                return Err(format!(
                    "invalid origin {origin:?}: an origin must include a scheme, \
                     e.g. \"https://example.com\""
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Http {
        cors: Cors,
    }

    #[test]
    fn default_is_disabled() {
        assert_eq!(Cors::default(), Cors(vec![]));
    }

    #[test]
    fn deserializes_origin_list() {
        let h: Http = serde_json::from_str(r#"{"cors": ["https://example.com", "*"]}"#).unwrap();
        assert_eq!(h.cors, Cors(vec!["https://example.com".into(), "*".into()]));
    }

    #[test]
    fn rejects_the_removed_boolean_form() {
        // Breaking change: `cors = true` must fail to parse, not silently
        // become something else.
        assert!(serde_json::from_str::<Http>(r#"{"cors": true}"#).is_err());
        assert!(serde_json::from_str::<Http>(r#"{"cors": false}"#).is_err());
    }

    #[test]
    fn validate_accepts_wildcard_and_origins() {
        Cors::default().validate().unwrap();
        Cors(vec!["*".into()]).validate().unwrap();
        Cors(vec![
            "https://example.com".into(),
            "http://127.0.0.1:8080".into(),
        ])
        .validate()
        .unwrap();
    }

    #[test]
    fn validate_rejects_origins_without_scheme() {
        let err = Cors(vec!["example.com".into()]).validate().unwrap_err();
        assert!(err.contains("scheme"), "{err}");
    }

    #[test]
    fn validate_rejects_invalid_header_values() {
        let err = Cors(vec!["https://exa\u{0}mple.com".into()])
            .validate()
            .unwrap_err();
        assert!(err.contains("header value"), "{err}");
    }
}
