use std::time::Duration;

use anyhow::{Context, Result, anyhow};

pub mod loadtest;
pub mod packetizer;
pub mod publisher;
pub mod source;

pub use loadtest::{LoadtestConfig, LoadtestStatsSnapshot, run_loadtest};
pub use packetizer::{Packetizer, PacketizerConfig};
pub use publisher::{PublishOutcome, Publisher, PublisherConfig};
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
/// Format: `synth://<vcodec>?audio=<acodec>&width=<px>&height=<px>&fps=<n>&duration=<secs>&ice=<spec>`
/// Example: `synth://h264?audio=opus&width=1280&height=720&fps=30`
///
/// `ice` is a repeatable ICE server spec (`<url>[,<username>[,<credential>]]`,
/// see [`iceserver::IceServer`]); when present it replaces
/// `default_ice_servers` entirely.
pub fn publisher_from_input(
    input: &str,
    whip_url: String,
    token: Option<String>,
    default_ice_servers: Vec<webrtc::peer_connection::RTCIceServer>,
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
        ice_servers: default_ice_servers,
    };

    let mut ice_specs: Option<Vec<iceserver::IceServer>> = None;
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
            "ice" => ice_specs.get_or_insert_with(Vec::new).push(
                value
                    .parse()
                    .map_err(|e| anyhow!("Invalid synth ICE server spec: '{value}': {e}"))?,
            ),
            other => anyhow::bail!("Unknown synth input parameter: '{other}'"),
        }
    }
    if let Some(specs) = ice_specs {
        config.ice_servers = iceserver::to_rtc_ice_servers(specs);
    }

    Ok(Some(Publisher::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_ice_servers() -> Vec<webrtc::peer_connection::RTCIceServer> {
        Vec::new()
    }

    #[test]
    fn non_synth_input_returns_none() {
        assert!(
            publisher_from_input(
                "sdp://0.0.0.0:5004/test.sdp",
                "http://x/whip/1".into(),
                None,
                no_ice_servers()
            )
            .unwrap()
            .is_none()
        );
        assert!(
            publisher_from_input(
                "rtsp://127.0.0.1:8554/cam",
                "http://x/whip/1".into(),
                None,
                no_ice_servers()
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn synth_input_parses_all_parameters() {
        let config = publisher_from_input(
            "synth://h264?audio=opus&width=1280&height=720&fps=30&duration=10&ice=turn:turn.example.com:3478,user,pass",
            "http://x/whip/1".into(),
            Some("tok".into()),
            no_ice_servers(),
        )
        .unwrap()
        .expect("synth input should produce a publisher")
        .config_for_test();

        assert_eq!(config.video_codec, VideoCodec::H264);
        assert_eq!(config.audio_codec, Some(AudioCodec::Opus));
        assert_eq!((config.width, config.height, config.fps), (1280, 720, 30));
        assert_eq!(config.duration, Some(Duration::from_secs(10)));
        assert_eq!(config.ice_servers.len(), 1);
        assert_eq!(
            config.ice_servers[0].urls,
            vec!["turn:turn.example.com:3478?transport=udp"]
        );
        assert_eq!(config.ice_servers[0].username, "user");
        assert_eq!(config.ice_servers[0].credential, "pass");
    }

    #[test]
    fn synth_input_ice_param_replaces_default() {
        let config = publisher_from_input(
            "synth://vp8?ice=&ice=stun:stun.example.com:3478",
            "http://x/whip/1".into(),
            None,
            iceserver::default_rtc_ice_servers(),
        )
        .unwrap()
        .expect("synth input should produce a publisher")
        .config_for_test();

        // The empty `ice` entry is dropped; only the explicit server remains,
        // replacing the caller-provided default.
        assert_eq!(config.ice_servers.len(), 1);
        assert_eq!(
            config.ice_servers[0].urls,
            vec!["stun:stun.example.com:3478"]
        );
    }

    #[test]
    fn synth_input_defaults() {
        let config = publisher_from_input(
            "synth://vp8",
            "http://x/whip/1".into(),
            None,
            iceserver::default_rtc_ice_servers(),
        )
        .unwrap()
        .expect("synth input should produce a publisher")
        .config_for_test();

        assert_eq!(config.video_codec, VideoCodec::Vp8);
        assert_eq!(config.audio_codec, None);
        assert_eq!((config.width, config.height, config.fps), (640, 480, 30));
        assert_eq!(config.duration, None);
        assert_eq!(config.ice_servers.len(), 1);
        assert_eq!(
            config.ice_servers[0].urls,
            vec![iceserver::DEFAULT_ICE_SERVER_URL]
        );
    }

    #[test]
    fn synth_input_rejects_bad_codec_and_unknown_param() {
        assert!(
            publisher_from_input(
                "synth://mjpeg",
                "http://x/whip/1".into(),
                None,
                no_ice_servers()
            )
            .is_err(),
            "non-synthetic codec must be rejected"
        );
        assert!(
            publisher_from_input(
                "synth://vp8?bogus=1",
                "http://x/whip/1".into(),
                None,
                no_ice_servers()
            )
            .is_err(),
            "unknown parameter must be rejected"
        );
        assert!(
            publisher_from_input(
                "synth://vp8?ice=turn:turn.example.com:3478",
                "http://x/whip/1".into(),
                None,
                no_ice_servers()
            )
            .is_err(),
            "TURN server without credentials must be rejected"
        );
    }
}
