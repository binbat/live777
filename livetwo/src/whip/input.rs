use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::info;

use crate::protocol;
use crate::utils;
use crate::{SCHEME_RTSP_CLIENT, SCHEME_RTSP_SERVER};

#[derive(Debug, Clone)]
pub enum InputScheme {
    RtspServer,
    RtspClient,
    Rtp,
}

pub struct InputSource {
    scheme: InputScheme,
    media_info: rtsp::MediaInfo,
    target_host: String,
    listen_host: String,
    interleaved_channels: Option<rtsp::channels::InterleavedChannel>,
    port_update_rx: Option<UnboundedReceiver<rtsp::PortUpdate>>,
}

impl InputSource {
    pub fn new_with_media_info(
        scheme: InputScheme,
        media_info: rtsp::MediaInfo,
        target_host: String,
        listen_host: String,
    ) -> Self {
        Self {
            scheme,
            media_info,
            target_host,
            listen_host,
            interleaved_channels: None,
            port_update_rx: None,
        }
    }

    pub fn scheme(&self) -> &InputScheme {
        &self.scheme
    }

    pub fn media_info(&self) -> &rtsp::MediaInfo {
        &self.media_info
    }

    pub fn media_info_mut(&mut self) -> &mut rtsp::MediaInfo {
        &mut self.media_info
    }

    pub fn target_host(&self) -> &str {
        &self.target_host
    }

    pub fn listen_host(&self) -> &str {
        &self.listen_host
    }

    pub fn take_channels(&mut self) -> Option<rtsp::channels::InterleavedChannel> {
        self.interleaved_channels.take()
    }

    pub fn take_port_update_rx(&mut self) -> Option<UnboundedReceiver<rtsp::PortUpdate>> {
        self.port_update_rx.take()
    }

    pub fn address_config(&self) -> (&str, &str) {
        (&self.target_host, &self.listen_host)
    }

    pub fn with_updated_media_info(&self, new_media_info: rtsp::MediaInfo) -> Self {
        Self {
            scheme: self.scheme.clone(),
            media_info: new_media_info,
            target_host: self.target_host.clone(),
            listen_host: self.listen_host.clone(),
            interleaved_channels: None,
            port_update_rx: None,
        }
    }
}

pub async fn setup_input_source(
    target_url: &str,
    complete_tx: UnboundedSender<()>,
) -> Result<InputSource> {
    let input = utils::parse_input_url(target_url)?;
    info!("Processing input URL: {}", input);

    let (target_host, listen_host) = utils::host::parse_host(&input);
    info!("Target host: {}, Listen host: {}", target_host, listen_host);

    let scheme = match input.scheme() {
        SCHEME_RTSP_SERVER => InputScheme::RtspServer,
        SCHEME_RTSP_CLIENT => InputScheme::RtspClient,
        _ => InputScheme::Rtp,
    };

    let (media_info, final_host, channels, port_update_rx) = match scheme {
        InputScheme::RtspServer => {
            let video_port = input.port().unwrap_or(0);
            let (media_info, channels, port_update_rx) =
                protocol::rtsp::setup_server_for_push(&listen_host, video_port, complete_tx)
                    .await?;
            (media_info, target_host, channels, Some(port_update_rx))
        }
        InputScheme::RtspClient => {
            let (media_info, channels) =
                protocol::rtsp::setup_client_for_pull(target_url, &target_host).await?;
            (media_info, target_host, channels, None)
        }
        InputScheme::Rtp => {
            let (media_info, host) = protocol::rtp::setup_rtp_input(target_url).await?;
            (media_info, host, None, None)
        }
    };

    Ok(InputSource {
        scheme,
        media_info,
        target_host: final_host,
        listen_host,
        interleaved_channels: channels,
        port_update_rx,
    })
}
