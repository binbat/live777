use std::{net::SocketAddr, process::Command};

use anyhow::{Context, Result};

use super::{ProcessHandle, Source, SourceHandle};
use crate::profile::{AudioCodec, MediaProfile};

/// RTP source implemented by spawning `gst-launch-1.0`.
///
/// Mirrors [`super::ffmpeg::FfmpegSource`] with GStreamer pipelines instead of
/// an FFmpeg command line: `videotestsrc`/`audiotestsrc` → encoder → RTP
/// payloader → `udpsink`, ingested by whipinto.
#[derive(Debug, Clone, Copy)]
pub struct GstRtpSource {
    pub profile: MediaProfile,
}

impl GstRtpSource {
    pub fn new(profile: MediaProfile) -> Self {
        Self { profile }
    }

    /// GStreamer elements required by this profile, for
    /// [`crate::runner::require_gst`].
    pub fn required_elements(&self) -> Vec<&'static str> {
        let mut elements = vec!["udpsink"];
        if let Some(video) = self.profile.video {
            elements.push("videotestsrc");
            elements.push(video.codec.gst_encoder().0);
            elements.push(video.codec.gst_pay());
        }
        if let Some(audio) = self.profile.audio {
            elements.push("audiotestsrc");
            elements.push(audio.gst_encoder().0);
            elements.push(audio.gst_pay());
        }
        elements
    }
}

/// A video pipeline chain ending in an arbitrary sink description.
pub fn video_chain_sink(
    codec: crate::profile::VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    sink: &str,
) -> String {
    let (encoder, encoder_args) = codec.gst_encoder();
    // The payloader's default pt is 96 for every codec; pin it so the RTP
    // packets match the payload type declared in the generated SDP.
    format!(
        "videotestsrc is-live=true ! video/x-raw,width={width},height={height},framerate={fps}/1 ! {encoder} {encoder_args} ! {} pt={} ! {sink}",
        codec.gst_pay(),
        codec.payload_type()
    )
}

fn audio_chain(codec: AudioCodec, host: std::net::IpAddr, port: u16) -> String {
    let (encoder, encoder_args) = codec.gst_encoder();
    // Pin the payload type to match the generated SDP, same as the video chain.
    format!(
        "audiotestsrc is-live=true ! {encoder} {encoder_args} ! {} pt={} ! udpsink host={host} port={port}",
        codec.gst_pay(),
        codec.payload_type()
    )
}

fn video_chain(
    codec: crate::profile::VideoCodec,
    width: u32,
    height: u32,
    fps: u32,
    host: std::net::IpAddr,
    port: u16,
) -> String {
    video_chain_sink(
        codec,
        width,
        height,
        fps,
        &format!("udpsink host={host} port={port}"),
    )
}

/// Build the gst-launch pipeline description for the whole profile.
pub fn pipeline(
    profile: &MediaProfile,
    video_addr: Option<SocketAddr>,
    audio_addr: Option<SocketAddr>,
) -> Result<String> {
    let mut chains = Vec::new();
    if let Some(video) = profile.video {
        let addr = video_addr.context("video address required")?;
        chains.push(video_chain(
            video.codec,
            video.width,
            video.height,
            video.fps,
            addr.ip(),
            addr.port(),
        ));
    }
    if let Some(audio) = profile.audio {
        let addr = audio_addr.context("audio address required")?;
        chains.push(audio_chain(audio, addr.ip(), addr.port()));
    }
    if chains.is_empty() {
        anyhow::bail!("media profile has no tracks");
    }
    Ok(chains.join(" "))
}

impl Source for GstRtpSource {
    fn name(&self) -> String {
        format!("gst-rtp-{}", self.profile.name())
    }

    fn profile(&self) -> MediaProfile {
        self.profile
    }

    fn start_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        audio_addr: Option<SocketAddr>,
    ) -> Result<Box<dyn SourceHandle>> {
        let pipeline = pipeline(&self.profile, video_addr, audio_addr)?;
        let child = Command::new("gst-launch-1.0")
            .arg("-q")
            .args(pipeline.split_whitespace())
            .spawn()
            .with_context(|| format!("Failed to spawn gst-launch-1.0: {pipeline}"))?;
        Ok(Box::new(ProcessHandle::new(child)))
    }

    fn sdp_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        audio_addr: Option<SocketAddr>,
    ) -> String {
        let mut sdp = String::from(
            "v=0\r\n\
             o=- 0 0 IN IP4 127.0.0.1\r\n\
             s=gstreamer test stream\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n",
        );

        if let Some(video) = self.profile.video {
            let pt = video.codec.payload_type();
            let port = video_addr.expect("video address required").port();
            sdp.push_str(&format!(
                "m=video {port} RTP/AVP {pt}\r\n\
                 {}\r\n",
                video.codec.sdp_rtpmap(pt),
            ));
        }

        if let (Some(audio), Some(addr)) = (self.profile.audio, audio_addr) {
            let pt = audio.payload_type();
            sdp.push_str(&format!(
                "m=audio {} RTP/AVP {pt}\r\n\
                 {}\r\n",
                addr.port(),
                audio.sdp_rtpmap(pt),
            ));
        }

        sdp
    }
}
