use std::sync::Arc;

use crate::config::Config;
use crate::stream::manager::Manager;

pub mod admin;
pub mod recorder;
pub mod session;
pub mod sdp;
pub mod strategy;
pub mod stream;
pub mod whep;
pub mod whip;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub stream_manager: Arc<Manager>,
}
