use axum::Router;

use crate::AppState;

pub fn route() -> Router<AppState> {
    Router::new()
}
