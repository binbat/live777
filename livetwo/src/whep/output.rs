use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self};
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
    connection_id: u32,
    scheme: OutputScheme,
    media_info: rtsp::MediaInfo,
    target_host: String,
    interleaved_channels: Option<rtsp::channels::InterleavedChannel>,
    port_update_rx: Option<mpsc::Receiver<OutputTarget>>,
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

    pub fn take_port_update_rx(&mut self) -> Option<mpsc::Receiver<OutputTarget>> {
        self.port_update_rx.take()
    }

    /// True when this target was created by an RTSP server pull session.
    /// In this mode the actual transport is handled by the framework's
    /// mpsc channels; the `media_info` transport fields preserve the
    /// original client-negotiated parameters for diagnostics.
    pub fn is_rtsp_server_pull(&self) -> bool {
        matches!(self.scheme, OutputScheme::RtspServer)
    }

    pub fn from_rtsp_session(session: protocol::rtsp::RtspPullSession) -> Self {
        Self {
            connection_id: session.connection_id,
            scheme: OutputScheme::RtspServer,
            media_info: session.media_info,
            target_host: String::new(),
            interleaved_channels: Some(session.channels),
            port_update_rx: None,
        }
    }

    fn with_port_update_rx(mut self, port_update_rx: mpsc::Receiver<OutputTarget>) -> Self {
        self.port_update_rx = Some(port_update_rx);
        self
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
        crate::SCHEME_RTP_SDP | "rtp" => OutputScheme::Rtp,
        scheme => return Err(anyhow!("Unsupported output URL scheme: {scheme}")),
    };

    match scheme {
        OutputScheme::RtspServer => {
            let port = input.port().unwrap_or(0);
            let (first, mut update_rx) =
                protocol::rtsp::setup_server_for_pull(ct, &listen_host, port, filtered_sdp).await?;
            let (target_tx, target_rx) = mpsc::channel::<OutputTarget>(16);
            tokio::spawn(async move {
                while let Some(session) = update_rx.recv().await {
                    if target_tx
                        .send(OutputTarget::from_rtsp_session(session))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            Ok(OutputTarget::from_rtsp_session(first).with_port_update_rx(target_rx))
        }
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
                port_update_rx: None,
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
                port_update_rx: None,
            })
        }
    }
}
