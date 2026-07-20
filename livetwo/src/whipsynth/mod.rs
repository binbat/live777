use std::time::Duration;

use anyhow::{Context, Result, anyhow};

pub mod loadtest;
pub mod packetizer;
pub mod publisher;
pub mod source;

pub use loadtest::{LoadtestConfig, LoadtestStatsSnapshot, run_loadtest};
pub use packetizer::{Packetizer, PacketizerConfig};
pub use publisher::{Publisher, PublisherConfig};
pub use source::SourceHandle;

use crate::source::{AudioCodec, VideoCodec};

/// Runtime statistics for a WHIP publisher session.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub failed_writes: u64,
    pub nack_count: u64,
    pub pli_count: u64,
    pub connected_duration: Duration,
}

/// Parse a synthetic input URL into a [`Publisher`].
///
/// Returns `Ok(None)` when `input` is not a `synth://` URL, so callers can
/// fall through to the RTP/RTSP input path.
///
/// Format: `synth://<vcodec>?audio=<acodec>&width=<px>&height=<px>&fps=<n>&duration=<secs>&stun=<url>`
/// Example: `synth://h264?audio=opus&width=1280&height=720&fps=30`
pub fn publisher_from_input(
    input: &str,
    whip_url: String,
    token: Option<String>,
) -> Result<Option<Publisher>> {
    let prefix = format!("{}://", crate::SCHEME_SYNTH);
    if !input.starts_with(&prefix) {
        return Ok(None);
    }

    let url =
        url::Url::parse(input).with_context(|| format!("Invalid synthetic input URL: {input}"))?;

    let vcodec_name = url.host_str().unwrap_or_default();
    let video_cli = cli::codec_from_str(vcodec_name).with_context(|| {
        format!("Invalid synth video codec: '{vcodec_name}' (expected vp8, vp9, h264, h265, av1)")
    })?;
    let video_codec = VideoCodec::from_cli(video_cli)
        .ok_or_else(|| anyhow!("Unsupported synth video codec: {vcodec_name}"))?;

    let mut config = PublisherConfig {
        whip_url,
        token,
        video_codec,
        audio_codec: None,
        width: 640,
        height: 480,
        fps: 30,
        duration: None,
        stun_server: crate::whip::core::DEFAULT_STUN_SERVER.to_string(),
    };

    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "audio" => {
                let audio_cli = cli::codec_from_str(&value).with_context(|| {
                    format!("Invalid synth audio codec: '{value}' (expected opus, g722)")
                })?;
                config.audio_codec = Some(
                    AudioCodec::from_cli(audio_cli)
                        .ok_or_else(|| anyhow!("Unsupported synth audio codec: {value}"))?,
                );
            }
            "width" => {
                config.width = value
                    .parse()
                    .with_context(|| format!("Invalid synth width: '{value}'"))?;
            }
            "height" => {
                config.height = value
                    .parse()
                    .with_context(|| format!("Invalid synth height: '{value}'"))?;
            }
            "fps" => {
                config.fps = value
                    .parse()
                    .with_context(|| format!("Invalid synth fps: '{value}'"))?;
            }
            "duration" => {
                config.duration =
                    Some(Duration::from_secs(value.parse().with_context(|| {
                        format!("Invalid synth duration: '{value}'")
                    })?));
            }
            "stun" => config.stun_server = value.into_owned(),
            other => anyhow::bail!("Unknown synth input parameter: '{other}'"),
        }
    }

    Ok(Some(Publisher::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_synth_input_returns_none() {
        assert!(
            publisher_from_input(
                "sdp://0.0.0.0:5004/test.sdp",
                "http://x/whip/1".into(),
                None
            )
            .unwrap()
            .is_none()
        );
        assert!(
            publisher_from_input("rtsp://127.0.0.1:8554/cam", "http://x/whip/1".into(), None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn synth_input_parses_all_parameters() {
        let config = publisher_from_input(
            "synth://h264?audio=opus&width=1280&height=720&fps=30&duration=10&stun=stun:example.com:3478",
            "http://x/whip/1".into(),
            Some("tok".into()),
        )
        .unwrap()
        .expect("synth input should produce a publisher")
        .config_for_test();

        assert_eq!(config.video_codec, VideoCodec::H264);
        assert_eq!(config.audio_codec, Some(AudioCodec::Opus));
        assert_eq!((config.width, config.height, config.fps), (1280, 720, 30));
        assert_eq!(config.duration, Some(Duration::from_secs(10)));
        assert_eq!(config.stun_server, "stun:example.com:3478");
    }

    #[test]
    fn synth_input_defaults() {
        let config = publisher_from_input("synth://vp8", "http://x/whip/1".into(), None)
            .unwrap()
            .expect("synth input should produce a publisher")
            .config_for_test();

        assert_eq!(config.video_codec, VideoCodec::Vp8);
        assert_eq!(config.audio_codec, None);
        assert_eq!((config.width, config.height, config.fps), (640, 480, 30));
        assert_eq!(config.duration, None);
        assert_eq!(config.stun_server, crate::whip::core::DEFAULT_STUN_SERVER);
    }

    #[test]
    fn synth_input_rejects_bad_codec_and_unknown_param() {
        assert!(
            publisher_from_input("synth://mjpeg", "http://x/whip/1".into(), None).is_err(),
            "non-synthetic codec must be rejected"
        );
        assert!(
            publisher_from_input("synth://vp8?bogus=1", "http://x/whip/1".into(), None).is_err(),
            "unknown parameter must be rejected"
        );
    }
}
