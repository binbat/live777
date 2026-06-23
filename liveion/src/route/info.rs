use axum::{Json, Router, routing::get};

use crate::AppState;
use api::response::ServerInfo;

pub fn route() -> Router<AppState> {
    Router::new().route(api::path::INFO, get(info))
}

async fn info() -> Json<ServerInfo> {
    Json(ServerInfo {
        version: version::VERSION.to_string(),
        git_hash: version::SHORT_COMMIT.to_string(),
        build_time: version::BUILD_TIME_3339.to_string(),
        features: enabled_features(),
    })
}

fn enabled_features() -> Vec<String> {
    #[allow(unused_mut)]
    let mut features = Vec::with_capacity(8);

    #[cfg(feature = "webui")]
    features.push("webui".to_string());
    #[cfg(feature = "cascade")]
    features.push("cascade".to_string());
    #[cfg(feature = "net4mqtt")]
    features.push("net4mqtt".to_string());
    #[cfg(feature = "recorder")]
    features.push("recorder".to_string());
    #[cfg(feature = "source")]
    features.push("source".to_string());
    #[cfg(feature = "source-sdp")]
    features.push("source-sdp".to_string());
    #[cfg(feature = "source-rtsp")]
    features.push("source-rtsp".to_string());
    #[cfg(feature = "source-all")]
    features.push("source-all".to_string());

    features
}
