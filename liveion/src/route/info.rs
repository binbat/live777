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
    vec![
        #[cfg(feature = "webui")]
        "webui".to_string(),
        #[cfg(feature = "cascade")]
        "cascade".to_string(),
        #[cfg(feature = "net4mqtt")]
        "net4mqtt".to_string(),
        #[cfg(feature = "recorder")]
        "recorder".to_string(),
        #[cfg(feature = "source")]
        "source".to_string(),
        #[cfg(feature = "source-sdp")]
        "source-sdp".to_string(),
        #[cfg(feature = "source-rtsp")]
        "source-rtsp".to_string(),
        #[cfg(feature = "source-whep")]
        "source-whep".to_string(),
        #[cfg(feature = "source-all")]
        "source-all".to_string(),
    ]
}
