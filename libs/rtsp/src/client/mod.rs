pub mod auth;
pub mod session;

pub use auth::AuthParams;
pub use session::{RtspSession, setup_rtsp_session};

use crate::types::SessionMode;

#[derive(Clone, Debug)]
pub enum RtspMode {
    Pull,
    Push,
}

impl RtspMode {
    pub fn transport_mode(&self) -> Option<rtsp_types::headers::transport::TransportMode> {
        match self {
            RtspMode::Pull => None,
            RtspMode::Push => Some(rtsp_types::headers::transport::TransportMode::Record),
        }
    }

    pub fn to_session_mode(&self) -> SessionMode {
        match self {
            RtspMode::Pull => SessionMode::Pull,
            RtspMode::Push => SessionMode::Push,
        }
    }
}

impl From<SessionMode> for RtspMode {
    fn from(mode: SessionMode) -> Self {
        match mode {
            SessionMode::Pull => RtspMode::Pull,
            SessionMode::Push => RtspMode::Push,
            SessionMode::Mixed => unreachable!("Mixed mode is server-only"),
        }
    }
}
