use std::sync::Arc;

use crate::config::Config;
use crate::stream::manager::Manager;

pub mod admin;
pub mod recorder;
pub mod sdp;
pub mod session;
pub mod strategy;
pub mod stream;
pub mod whep;
pub mod whip;

#[cfg(feature = "source")]
pub mod source;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub stream_manager: Arc<Manager>,
}
