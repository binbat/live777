use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::{Source, SourceHandle, VideoCodec};

/// Synthetic WHIP source implemented with `livetwo::whipsynth`, the same
/// publisher used by the `whipsynth` CLI.
#[derive(Debug, Clone, Copy)]
pub struct WhipgenSource {
    pub video_codec: VideoCodec,
    pub audio_codec: Option<WhipgenAudioCodec>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhipgenAudioCodec {
    Opus,
    G722,
}

impl Default for WhipgenSource {
    fn default() -> Self {
        Self {
            video_codec: VideoCodec::Vp8,
            audio_codec: Some(WhipgenAudioCodec::Opus),
            width: 640,
            height: 480,
            fps: 30,
        }
    }
}

impl WhipgenSource {
    pub fn new(video_codec: VideoCodec) -> Self {
        Self {
            video_codec,
            audio_codec: None,
            width: 640,
            height: 480,
            fps: 30,
        }
    }

    pub fn with_audio(mut self, audio_codec: WhipgenAudioCodec) -> Self {
        self.audio_codec = Some(audio_codec);
        self
    }

    /// Return H265 sprop parameters for this source's resolution and frame rate,
    /// if applicable.
    #[allow(dead_code)]
    pub fn sprop_params(&self) -> Option<String> {
        if self.video_codec != VideoCodec::H265 {
            return None;
        }
        livetwo::source::extract_h265_sprop(self.width, self.height, self.fps)
    }
}

impl Source for WhipgenSource {
    fn name(&self) -> &'static str {
        match (self.video_codec, self.audio_codec) {
            (VideoCodec::Vp8, None) => "whipsynth-vp8",
            (VideoCodec::Vp8, Some(WhipgenAudioCodec::Opus)) => "whipsynth-vp8-opus",
            (VideoCodec::Vp8, Some(WhipgenAudioCodec::G722)) => "whipsynth-vp8-g722",
            (VideoCodec::H264, None) => "whipsynth-h264",
            (VideoCodec::H264, Some(WhipgenAudioCodec::Opus)) => "whipsynth-h264-opus",
            (VideoCodec::Vp9, None) => "whipsynth-vp9",
            (VideoCodec::Vp9, Some(WhipgenAudioCodec::Opus)) => "whipsynth-vp9-opus",
            (VideoCodec::Av1, None) => "whipsynth-av1",
            (VideoCodec::Av1, Some(WhipgenAudioCodec::Opus)) => "whipsynth-av1-opus",
            _ => "whipsynth",
        }
    }

    fn has_audio(&self) -> bool {
        self.audio_codec.is_some()
    }

    fn start(&self, _target_addr: SocketAddr) -> Result<Box<dyn SourceHandle>> {
        anyhow::bail!("WhipgenSource uses direct WHIP publishing; call start_direct")
    }

    fn sdp(&self, _listen_addr: SocketAddr) -> String {
        String::new()
    }

    fn publishes_directly(&self) -> bool {
        true
    }

    fn start_direct(&self, whip_url: &str) -> Result<Box<dyn SourceHandle>> {
        let ct = CancellationToken::new();
        let run_ct = ct.clone();

        let video_codec = to_livetwo_video_codec(self.video_codec);
        let audio_codec = self.audio_codec.map(to_livetwo_audio_codec);

        let config = livetwo::whipsynth::PublisherConfig {
            whip_url: whip_url.to_owned(),
            token: None,
            video_codec,
            audio_codec,
            width: self.width,
            height: self.height,
            fps: self.fps,
            duration: Some(Duration::from_secs(30)),
        };

        tokio::spawn(async move {
            let publisher = livetwo::whipsynth::Publisher::new(config);
            if let Err(e) = publisher.run(run_ct).await {
                tracing::error!(error = ?e, "whipsynth publisher error");
            }
        });

        Ok(Box::new(WhipgenHandle { ct }))
    }
}

struct WhipgenHandle {
    ct: CancellationToken,
}

impl SourceHandle for WhipgenHandle {
    fn stop(self: Box<Self>) {
        self.ct.cancel();
    }
}

fn to_livetwo_video_codec(codec: VideoCodec) -> livetwo::source::VideoCodec {
    match codec {
        VideoCodec::Vp8 => livetwo::source::VideoCodec::Vp8,
        VideoCodec::H264 => livetwo::source::VideoCodec::H264,
        VideoCodec::H265 => livetwo::source::VideoCodec::H265,
        VideoCodec::Vp9 => livetwo::source::VideoCodec::Vp9,
        VideoCodec::Av1 => livetwo::source::VideoCodec::Av1,
    }
}

fn to_livetwo_audio_codec(codec: WhipgenAudioCodec) -> livetwo::source::AudioCodec {
    match codec {
        WhipgenAudioCodec::Opus => livetwo::source::AudioCodec::Opus,
        WhipgenAudioCodec::G722 => livetwo::source::AudioCodec::G722,
    }
}
