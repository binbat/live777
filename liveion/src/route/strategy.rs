use crate::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

pub fn route() -> Router<AppState> {
    Router::new().route(&api::path::strategy(), get(show))
}

async fn show(
    State(state): State<AppState>,
) -> crate::result::Result<Json<api::strategy::Strategy>> {
    Ok(Json(state.config.strategy))
}
