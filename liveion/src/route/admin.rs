use axum::Router;

#[cfg(feature = "cascade")]
use axum::Json;
#[cfg(feature = "cascade")]
use axum::extract::{Path, State};
#[cfg(feature = "cascade")]
use axum::routing::post;

use crate::AppState;
#[cfg(feature = "cascade")]
use crate::error::AppError;
#[cfg(feature = "cascade")]
use crate::result::Result;

pub fn route() -> Router<AppState> {
    #[cfg(feature = "cascade")]
    {
        Router::new().route(&api::path::cascade("{stream}"), post(cascade))
    }
    #[cfg(not(feature = "cascade"))]
    {
        Router::new()
    }
}

#[cfg(feature = "cascade")]
async fn cascade(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(body): Json<api::request::Cascade>,
) -> Result<String> {
    let api::request::Cascade {
        source_url,
        target_url,
        token,
    } = body;

    match (source_url, target_url) {
        (None, None) => {
            return Err(AppError::throw(
                "src and dst cannot be empty at the same time",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(AppError::throw(
                "src and dst cannot be non-empty at the same time",
            ));
        }
        (Some(source_url), None) => {
            state
                .stream_manager
                .cascade_pull(stream, source_url, token)
                .await?;
        }
        (None, Some(target_url)) => {
            state
                .stream_manager
                .cascade_push(stream, target_url, token)
                .await?;
        }
    }
    Ok("".to_string())
}
