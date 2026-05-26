use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::protocol;
use crate::utils;
use crate::{SCHEME_RTSP_CLIENT, SCHEME_RTSP_SERVER};
use rtsp::constants::media_type;

#[derive(Debug)]
pub enum OutputScheme {
    RtspServer,
    RtspClient,
    Rtp,
}

pub struct OutputTarget {
    scheme: OutputScheme,
    media_info: rtsp::MediaInfo,
    target_host: String,
    interleaved_channels: Option<rtsp::channels::InterleavedChannel>,
    port_update_rx: Option<UnboundedReceiver<rtsp::PortUpdate>>,
}

impl OutputTarget {
    pub fn scheme(&self) -> &OutputScheme {
        &self.scheme
    }

    pub fn media_info(&self) -> &rtsp::MediaInfo {
        &self.media_info
    }

    pub fn target_host(&self) -> &str {
        &self.target_host
    }
    pub fn take_channels(&mut self) -> Option<rtsp::channels::InterleavedChannel> {
        self.interleaved_channels.take()
    }

    pub fn take_port_update_rx(&mut self) -> Option<UnboundedReceiver<rtsp::PortUpdate>> {
        self.port_update_rx.take()
    }

    pub fn from_media_info(media_info: rtsp::MediaInfo) -> Self {
        Self {
            scheme: OutputScheme::RtspServer,
            media_info,
            target_host: String::new(),
            interleaved_channels: None,
            port_update_rx: None,
        }
    }
}

pub async fn setup_output_target(
    ct: CancellationToken,
    target_url: &str,
    answer_sdp: &str,
    sdp_file: Option<String>,
    codec_info: &rtsp::CodecInfo,
    notify: Arc<Notify>,
) -> Result<OutputTarget> {
    let input = utils::parse_input_url(target_url)?;
    info!("Processing output URL: {}", target_url);

    let (target_host, listen_host) = utils::host::parse_host(&input);
    info!("Target host: {}, Listen host: {}", target_host, listen_host);

    let has_video_param = input.query_pairs().any(|(k, _)| k == media_type::VIDEO);
    let has_audio_param = input.query_pairs().any(|(k, _)| k == media_type::AUDIO);
    let has_any_media_param = has_video_param || has_audio_param;

    // Only include codecs the user explicitly requested.
    // If neither is specified (e.g. rtp://host), include all available.
    let video_codec_filter = if has_any_media_param && !has_video_param {
        None
    } else {
        codec_info.video_codec.as_ref()
    };
    let audio_codec_filter = if has_any_media_param && !has_audio_param {
        None
    } else {
        codec_info.audio_codec.as_ref()
    };

    let filtered_sdp = rtsp::filter_sdp(answer_sdp, video_codec_filter, audio_codec_filter)?;

    let scheme = match input.scheme() {
        SCHEME_RTSP_SERVER => OutputScheme::RtspServer,
        SCHEME_RTSP_CLIENT => OutputScheme::RtspClient,
        _ => OutputScheme::Rtp,
    };

    let (media_info, channels, port_update_rx) = match scheme {
        OutputScheme::RtspServer => {
            let port = input.port().unwrap_or(0);
            let (media_info, channels, port_update_rx) =
                protocol::rtsp::setup_server_for_pull(ct, &listen_host, port, filtered_sdp, notify)
                    .await?;
            (media_info, channels, Some(port_update_rx))
        }
        OutputScheme::RtspClient => {
            let (media_info, channels) =
                protocol::rtsp::setup_client_for_push(target_url, &target_host, filtered_sdp)
                    .await?;
            (media_info, channels, None)
        }
        OutputScheme::Rtp => {
            let media_info =
                protocol::rtp::setup_rtp_output(&input, filtered_sdp, sdp_file, notify).await?;
            (media_info, None, None)
        }
    };

    Ok(OutputTarget {
        scheme,
        media_info,
        target_host,
        interleaved_channels: channels,
        port_update_rx,
    })
}
