pub mod gst_rtp;
pub mod gst_whep;
pub mod livetwo;
#[cfg(feature = "whepwright")]
pub mod playwright;
#[cfg(feature = "rsmpeg")]
pub mod rsmpeg_receiver;

use anyhow::{Context, Result, anyhow};

use crate::profile::MediaProfile;

#[derive(Debug, Default)]
pub struct PlayResult {
    pub success: bool,
    pub connected: bool,
    pub error: Option<String>,
    pub video_width: u32,
    pub video_height: u32,
    pub video_tracks: u32,
    pub audio_tracks: u32,
    pub duration_ms: u64,
    /// Codec names reported by the player/validator for each received stream,
    /// e.g. `["vp8", "opus"]`. Empty when the player does not probe codecs.
    pub codecs: Vec<String>,
    /// Audio channel count of the first audio stream, when probed.
    pub audio_channels: u32,
}

#[async_trait::async_trait]
pub trait Player: Send + Sync {
    fn name(&self) -> &'static str;

    /// Play `whep_url` and validate the received media against `profile`.
    async fn play(&self, whep_url: &str, profile: &MediaProfile) -> anyhow::Result<PlayResult>;
}

/// Split a WHEP URL into `(base, stream_id)`, e.g.
/// `http://host:port/whep/777` → `("http://host:port", "777")`.
pub(crate) fn parse_whep_url(whep_url: &str) -> Result<(String, String)> {
    let parsed = url::Url::parse(whep_url).context("Invalid WHEP URL")?;
    // `url::Host` renders IPv6 addresses with the required brackets.
    let host = parsed.host().ok_or_else(|| anyhow!("Missing host"))?;
    let base = format!("{}://{}", parsed.scheme(), host);
    let base = if let Some(port) = parsed.port() {
        format!("{base}:{port}")
    } else {
        base
    };

    let path = parsed.path();
    let stream_id = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Failed to parse stream id from WHEP URL"))?
        .to_string();

    Ok((base, stream_id))
}

/// Poll liveion until the stream has a Connected subscribe session, the WHEP
/// task exits, or the attempts budget runs out. Returns `(connected, error)`.
pub(crate) async fn wait_subscribe_connected(
    base_url: &str,
    stream_id: &str,
    handle_whep: &mut Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
) -> (bool, Option<String>) {
    let mut last_error = None;

    for _ in 0..300 {
        let res = match reqwest::get(format!("{base_url}{}", api::path::streams(""))).await {
            Ok(res) => res,
            Err(e) => {
                last_error = Some(format!("failed to query liveion streams: {e:?}"));
                break;
            }
        };

        if res.status() != http::StatusCode::OK {
            last_error = Some(format!("liveion returned {}", res.status()));
            break;
        }

        if let Ok(body) = res.json::<Vec<api::response::Stream>>().await
            && let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && stream
                .subscribe
                .sessions
                .iter()
                .any(|s| s.state == api::response::RTCPeerConnectionState::Connected)
        {
            return (true, None);
        }

        if let Some(handle) = handle_whep.as_ref()
            && handle.is_finished()
        {
            match handle_whep.take().unwrap().await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => last_error = Some(format!("{e:?}")),
                Err(e) => last_error = Some(format!("{e:?}")),
            }
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    (false, last_error)
}
