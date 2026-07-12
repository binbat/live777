use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::SCHEME_RTSP_CLIENT;
use crate::protocol;
use crate::utils;

#[derive(Debug, Clone)]
pub enum InputScheme {
    RtspClient,
    Rtp,
}

pub struct InputSource {
    scheme: InputScheme,
    media_info: rtsp::MediaInfo,
    target_host: String,
    listen_host: String,
    interleaved_channels: Option<rtsp::channels::InterleavedChannel>,
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
        }
    }
}

pub async fn setup_input_source(_ct: CancellationToken, target_url: &str) -> Result<InputSource> {
    let input = utils::parse_input_url(target_url)?;
    info!("Processing input URL: {}", input);

    let (target_host, listen_host) = utils::host::parse_host(&input);
    info!("Target host: {}, Listen host: {}", target_host, listen_host);

    let scheme = match input.scheme() {
        SCHEME_RTSP_CLIENT => InputScheme::RtspClient,
        _ => InputScheme::Rtp,
    };

    let (media_info, final_host, final_listen_host, channels) = match scheme {
        InputScheme::RtspClient => {
            let (media_info, channels) =
                protocol::rtsp::setup_client_for_pull(target_url, &target_host).await?;
            (media_info, target_host, listen_host, channels)
        }
        InputScheme::Rtp => {
            let (media_info, host) = protocol::rtp::setup_rtp_input(target_url).await?;
            let listen_host = utils::host::derive_listen_host(&host);
            (media_info, host, listen_host, None)
        }
    };

    Ok(InputSource {
        scheme,
        media_info,
        target_host: final_host,
        listen_host: final_listen_host,
        interleaved_channels: channels,
    })
}
