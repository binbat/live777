pub mod cascade;
pub mod node;
pub mod proxy;
pub mod recorder;
pub mod source;
#[cfg(feature = "recorder")]
pub mod storage;
#[cfg(not(feature = "recorder"))]
pub mod storage {
    use axum::Router;

    use crate::AppState;

    pub fn route() -> Router<AppState> {
        Router::new()
    }
}
pub mod stream;
pub mod utils;
