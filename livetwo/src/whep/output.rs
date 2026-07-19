use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::SCHEME_RTSP_CLIENT;
use crate::protocol;
use crate::utils;
use rtsp::constants::media_type;

#[derive(Debug)]
pub enum OutputScheme {
    RtspClient,
    Rtp,
}

pub struct OutputTarget {
    connection_id: u32,
    scheme: OutputScheme,
    media_info: rtsp::MediaInfo,
    target_host: String,
    interleaved_channels: Option<rtsp::channels::InterleavedChannel>,
}

impl OutputTarget {
    pub fn connection_id(&self) -> u32 {
        self.connection_id
    }

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
}

pub async fn setup_output_target(
    _ct: CancellationToken,
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
        SCHEME_RTSP_CLIENT => OutputScheme::RtspClient,
        crate::SCHEME_RTP_SDP | "rtp" => OutputScheme::Rtp,
        scheme => return Err(anyhow!("Unsupported output URL scheme: {scheme}")),
    };

    match scheme {
        OutputScheme::RtspClient => {
            let (media_info, channels) =
                protocol::rtsp::setup_client_for_push(target_url, &target_host, filtered_sdp)
                    .await?;
            Ok(OutputTarget {
                connection_id: 1,
                scheme,
                media_info,
                target_host,
                interleaved_channels: channels,
            })
        }
        OutputScheme::Rtp => {
            let media_info =
                protocol::rtp::setup_rtp_output(&input, filtered_sdp, sdp_file, notify).await?;
            Ok(OutputTarget {
                connection_id: 1,
                scheme,
                media_info,
                target_host,
                interleaved_channels: None,
            })
        }
    }
}
