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
    /// Options of an `rtp://` output (multicast TTL/interface); default for
    /// every other scheme.
    rtp_options: protocol::RtpOutputOptions,
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

    pub fn rtp_options(&self) -> &protocol::RtpOutputOptions {
        &self.rtp_options
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

    // Only the rtp:// scheme reads the URL's own port; sdp:// keeps its
    // historical query-only port behavior. The authority-port form already
    // addresses both tracks (video on P, audio on P + 2), so there
    // ?video=/?audio= only override ports and must not act as track
    // selectors.
    let authority_port = input.scheme() == "rtp" && input.port().is_some();

    let has_video_param =
        authority_port || input.query_pairs().any(|(k, _)| k == media_type::VIDEO);
    let has_audio_param =
        authority_port || input.query_pairs().any(|(k, _)| k == media_type::AUDIO);
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
                rtp_options: Default::default(),
            })
        }
        OutputScheme::Rtp => {
            let (media_info, rtp_options) = protocol::rtp::setup_rtp_output(
                &input,
                filtered_sdp,
                sdp_file,
                notify,
                authority_port,
            )
            .await?;
            Ok(OutputTarget {
                connection_id: 1,
                scheme,
                media_info,
                target_host,
                interleaved_channels: None,
                rtp_options,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal AV answer SDP: video H264 (PT 96) and audio Opus (PT 111).
    const AV_SDP: &str = "v=0\r\n\
        o=- 0 0 IN IP4 127.0.0.1\r\n\
        s=test\r\n\
        t=0 0\r\n\
        m=video 9 RTP/AVP 96\r\n\
        a=rtpmap:96 H264/90000\r\n\
        m=audio 9 RTP/AVP 111\r\n\
        a=rtpmap:111 OPUS/48000/2\r\n";

    fn av_codec_info() -> rtsp::CodecInfo {
        let codec = |mime_type: &str, payload_type: u8| {
            rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters {
                rtp_codec: rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
                    mime_type: mime_type.to_string(),
                    ..Default::default()
                },
                payload_type,
            }
        };
        rtsp::CodecInfo {
            video_codec: Some(codec("video/H264", 96)),
            audio_codec: Some(codec("audio/opus", 111)),
        }
    }

    fn rtp_send_port(transport: Option<rtsp::TransportInfo>) -> Option<u16> {
        match transport {
            Some(rtsp::TransportInfo::Udp { rtp_send_port, .. }) => rtp_send_port,
            _ => None,
        }
    }

    /// The full setup path (including the SDP codec filter) for an rtp://
    /// output URL, returning the produced media info.
    async fn setup_rtp_target(url: &str) -> rtsp::MediaInfo {
        let sdp_file = tempfile::NamedTempFile::new().unwrap();
        let path = sdp_file.path().to_string_lossy().into_owned();
        let target = setup_output_target(
            CancellationToken::new(),
            url,
            AV_SDP,
            Some(path),
            &av_codec_info(),
            Arc::new(Notify::new()),
        )
        .await
        .unwrap();
        target.media_info().clone()
    }

    /// rtp://host:port addresses video with the URL port and audio with
    /// port + 2; both tracks survive the SDP codec filter.
    #[tokio::test]
    async fn rtp_authority_port_addresses_both_tracks() {
        let info = setup_rtp_target("rtp://127.0.0.1:1720").await;
        assert_eq!(rtp_send_port(info.video_transport), Some(1720));
        assert_eq!(rtp_send_port(info.audio_transport), Some(1722));
    }

    /// ?video=/?audio= override the authority port per track without
    /// dropping the other track: with an authority port they are port
    /// overrides, not track selectors.
    #[tokio::test]
    async fn rtp_query_port_override_keeps_the_other_track() {
        let info = setup_rtp_target("rtp://127.0.0.1:1720?audio=6000").await;
        assert_eq!(rtp_send_port(info.video_transport), Some(1720));
        assert_eq!(rtp_send_port(info.audio_transport), Some(6000));
    }

    /// The legacy query-only form keeps its track-selector behavior:
    /// ?audio= alone still means audio-only.
    #[tokio::test]
    async fn rtp_query_only_form_still_selects_tracks() {
        let info = setup_rtp_target("rtp://127.0.0.1?audio=6000").await;
        assert!(info.video_transport.is_none());
        assert_eq!(rtp_send_port(info.audio_transport), Some(6000));
    }
}
