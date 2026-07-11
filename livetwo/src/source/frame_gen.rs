use std::collections::VecDeque;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avutil::{AVChannelLayout, AVDictionary, AVFrame};
use rsmpeg::ffi;
use tracing::{debug, info};

use super::rsmpeg_gen::{AudioCodec, VideoCodec};
use crate::whipsynth::source::SourceFrame;

/// A single encoded media frame produced by the rsmpeg generator.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Encoded bitstream.
    ///
    /// For H.264/H.265 this is Annex-B NAL units separated by start codes.
    /// For VP8/VP9 this is a complete frame.
    /// For AV1 this is one or more OBUs in low-overhead/Annex-B format.
    /// For Opus/G722 this is a complete encoded audio packet.
    pub data: Vec<u8>,
    /// Presentation timestamp in `time_base` units.
    pub pts: i64,
    /// Time base for `pts`.
    pub time_base: ffi::AVRational,
    /// Whether this frame is a keyframe.
    pub is_keyframe: bool,
}

/// A media frame produced by the generator.
#[derive(Debug, Clone)]
pub enum MediaFrame {
    Video(EncodedFrame),
    Audio(EncodedFrame),
}

/// Configuration for [`FrameGenerator`].
#[derive(Debug, Clone)]
pub struct FrameGeneratorConfig {
    pub video_codec: VideoCodec,
    pub audio_codec: Option<AudioCodec>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration: Option<Duration>,
}

impl FrameGeneratorConfig {
    /// Build a frame generator config from the legacy RTP-oriented config.
    pub fn from_generator_config(config: &super::rsmpeg_gen::GeneratorConfig) -> Self {
        Self {
            video_codec: config.video_codec,
            audio_codec: config.audio_codec,
            width: config.width,
            height: config.height,
            fps: config.fps,
            duration: config.duration,
        }
    }
}

/// Encoded-frame generator backed by FFmpeg/rsmpeg.
///
/// The generator produces synchronized video and audio frames at the requested
/// frame rate. It does not perform any RTP encapsulation; callers are
/// responsible for packetization.
pub struct FrameGenerator {
    video_codec: VideoCodec,
    audio_codec: Option<AudioCodec>,
    video_ctx: Option<OutputContext>,
    audio_ctx: Option<OutputContext>,
    video_pts: i64,
    audio_pts: i64,
    video_time_base: ffi::AVRational,
    audio_time_base: ffi::AVRational,
    audio_sample_rate: i32,
    samples_per_frame: i64,
    start: std::time::Instant,
    frame_index: u64,
    duration: Option<Duration>,
    fps: u32,
    pending_audio_frames: VecDeque<EncodedFrame>,
}

struct OutputContext {
    codec_ctx: AVCodecContext,
    /// Reusable video frame buffer to avoid allocating an `AVFrame` and its
    /// underlying buffer for every encoded frame. `None` for audio contexts.
    frame: Option<AVFrame>,
}

impl FrameGenerator {
    /// Create a new generator from the supplied configuration.
    pub fn new(config: &FrameGeneratorConfig) -> Result<Self> {
        info!(
            video_codec = ?config.video_codec,
            audio_codec = ?config.audio_codec,
            width = config.width,
            height = config.height,
            fps = config.fps,
            "Creating rsmpeg frame generator"
        );

        let video_ctx = {
            let ctx = open_video_output(config).context("Failed to open video encoder")?;
            OutputContext {
                codec_ctx: ctx.codec_ctx,
                frame: Some(ctx.frame),
            }
        };

        let audio_ctx = match config.audio_codec {
            Some(audio_codec) => Some({
                let ctx = open_audio_output(audio_codec).context("Failed to open audio encoder")?;
                OutputContext {
                    codec_ctx: ctx.codec_ctx,
                    frame: None,
                }
            }),
            None => None,
        };

        let audio_sample_rate = config.audio_codec.map(|c| c.sample_rate()).unwrap_or(48000);

        let samples_per_frame = config
            .audio_codec
            .map(|c| match c {
                AudioCodec::Opus => 960, // 20 ms at 48 kHz
                AudioCodec::G722 => 320, // 20 ms at 16 kHz
            })
            .unwrap_or(960) as i64;

        Ok(Self {
            video_codec: config.video_codec,
            audio_codec: config.audio_codec,
            video_ctx: Some(video_ctx),
            audio_ctx,
            video_pts: 0,
            audio_pts: 0,
            video_time_base: ffi::AVRational {
                num: 1,
                den: config.fps as i32,
            },
            audio_time_base: ffi::AVRational {
                num: 1,
                den: audio_sample_rate,
            },
            audio_sample_rate,
            samples_per_frame,
            start: std::time::Instant::now(),
            frame_index: 0,
            duration: config.duration,
            fps: config.fps,
            pending_audio_frames: VecDeque::new(),
        })
    }

    /// Return the configured video codec, if any.
    pub fn video_codec(&self) -> Option<VideoCodec> {
        Some(self.video_codec)
    }

    /// Return the configured audio codec, if any.
    pub fn audio_codec(&self) -> Option<AudioCodec> {
        self.audio_codec
    }

    /// Produce the next encoded media frame.
    ///
    /// Returns [`SourceFrame::End`] when the configured duration has elapsed or
    /// the generator has been exhausted. Returns [`SourceFrame::Empty`] when the
    /// encoder is buffering input frames and no output is available yet (common
    /// for VP9). The caller should throttle based on the frame rate.
    pub fn next_frame(&mut self) -> Result<SourceFrame> {
        if let Some(frame) = self.pending_audio_frames.pop_front() {
            return Ok(SourceFrame::Frame(MediaFrame::Audio(frame)));
        }

        if let Some(duration) = self.duration
            && self.start.elapsed() >= duration
        {
            return Ok(SourceFrame::End);
        }

        // Generate one video frame.
        let video_frame = {
            let ctx = self.video_ctx.as_mut().context("video encoder missing")?;
            encode_video_frame(ctx, self.frame_index, self.video_pts, self.video_time_base)
                .context("Failed to encode video frame")?
        };

        self.video_pts += 1;
        self.frame_index += 1;

        // Generate enough audio to stay aligned with the video timeline.
        if let Some(ref mut audio) = self.audio_ctx {
            let sample_rate = self.audio_sample_rate as i64;
            let target_audio_pts = self.video_pts * sample_rate / self.fps as i64;
            while self.audio_pts + self.samples_per_frame <= target_audio_pts {
                let frames = encode_audio_frame(
                    audio,
                    self.audio_pts,
                    self.audio_time_base,
                    self.samples_per_frame as i32,
                )
                .context("Failed to encode audio frame")?;
                self.audio_pts += self.samples_per_frame;
                self.pending_audio_frames.extend(frames);
            }
        }

        // Throttle to roughly the target frame rate. Sleep the full remaining
        // amount so low frame-rate generators (e.g. 1 fps tests) do not wake up
        // every 10 ms and busy-wait for the next frame.
        let expected_elapsed = Duration::from_secs_f64(self.frame_index as f64 / self.fps as f64);
        if let Some(sleep) = expected_elapsed.checked_sub(self.start.elapsed()) {
            std::thread::sleep(sleep);
        }

        if let Some(frame) = video_frame {
            Ok(SourceFrame::Frame(MediaFrame::Video(frame)))
        } else if let Some(frame) = self.pending_audio_frames.pop_front() {
            Ok(SourceFrame::Frame(MediaFrame::Audio(frame)))
        } else {
            Ok(SourceFrame::Empty)
        }
    }

    /// Flush remaining encoder output and close the generator.
    pub fn flush(&mut self) -> Result<()> {
        if let Some(ref mut video) = self.video_ctx {
            let _ = flush_encoder(video)?;
        }
        if let Some(ref mut audio) = self.audio_ctx {
            let _ = flush_encoder(audio)?;
        }
        Ok(())
    }
}

impl Drop for FrameGenerator {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

struct OpenedVideoOutput {
    codec_ctx: AVCodecContext,
    frame: AVFrame,
}

fn open_video_output(config: &FrameGeneratorConfig) -> Result<OpenedVideoOutput> {
    let codec = AVCodec::find_encoder_by_name(config.video_codec.ffmpeg_encoder())
        .ok_or_else(|| anyhow!("Encoder {} not found", config.video_codec.ffmpeg_name()))?;

    let mut codec_ctx = AVCodecContext::new(&codec);
    codec_ctx.set_width(config.width as i32);
    codec_ctx.set_height(config.height as i32);
    codec_ctx.set_time_base(ffi::AVRational {
        num: 1,
        den: config.fps as i32,
    });
    codec_ctx.set_framerate(ffi::AVRational {
        num: config.fps as i32,
        den: 1,
    });
    codec_ctx.set_pix_fmt(ffi::AV_PIX_FMT_YUV420P);
    codec_ctx.set_gop_size(config.fps as i32);
    codec_ctx.set_max_b_frames(0);

    // Pre-allocate the reusable video frame so each encoded frame only needs
    // to ensure the buffer is writable rather than creating a new buffer.
    let mut frame = AVFrame::new();
    frame.set_width(codec_ctx.width);
    frame.set_height(codec_ctx.height);
    frame.set_format(codec_ctx.pix_fmt);
    frame
        .alloc_buffer()
        .context("Failed to allocate reusable video frame buffer")?;

    // Codec-specific options.
    let mut opts = match config.video_codec {
        VideoCodec::Vp8 => AVDictionary::new(c"deadline", c"realtime", 0),
        VideoCodec::Vp9 => AVDictionary::new(c"deadline", c"realtime", 0),
        VideoCodec::H264 => AVDictionary::new(c"profile", c"baseline", 0),
        VideoCodec::H265 => AVDictionary::new(c"preset", c"ultrafast", 0),
        VideoCodec::Av1 => AVDictionary::new(c"preset", c"10", 0),
    };
    match config.video_codec {
        VideoCodec::Vp8 => {
            opts = opts.set(c"speed", c"4", 0);
            opts = opts.set(c"b", c"200000", 0);
        }
        VideoCodec::Vp9 => {
            opts = opts.set(c"profile", c"0", 0);
            opts = opts.set(c"speed", c"6", 0);
        }
        VideoCodec::H264 => {
            opts = opts.set(c"level", c"3.1", 0);
            opts = opts.set(c"tune", c"zerolatency", 0);
        }
        VideoCodec::H265 => {
            opts = opts.set(c"tune", c"zerolatency", 0);
            opts = opts.set(c"crf", c"28", 0);
        }
        VideoCodec::Av1 => {
            // Low-delay prediction structure and an explicit key-frame interval
            // are required for real-time WHIP streaming. Without `keyint` the
            // SVT-AV1 encoder ignores the FFmpeg GOP size and emits key frames
            // only every ~5 s, which causes receivers to stall after any loss
            // or decode error (see live777#169). `scd=0` and `lookahead=0` keep
            // the stream low-latency and avoid buffering future frames.
            let svt_params = std::ffi::CString::new(format!(
                "tune=0:pred-struct=1:keyint={}:scd=0:lookahead=0",
                config.fps
            ))
            .context("invalid SVT-AV1 parameters")?;
            opts = opts.set(c"svtav1-params", svt_params.as_c_str(), 0);
        }
    }

    codec_ctx.open(Some(opts)).with_context(|| {
        format!(
            "Failed to open {} encoder",
            config.video_codec.ffmpeg_name()
        )
    })?;

    Ok(OpenedVideoOutput { codec_ctx, frame })
}

struct OpenedAudioOutput {
    codec_ctx: AVCodecContext,
}

fn open_audio_output(audio_codec: AudioCodec) -> Result<OpenedAudioOutput> {
    let codec = AVCodec::find_encoder_by_name(audio_codec.ffmpeg_encoder())
        .ok_or_else(|| anyhow!("Encoder {} not found", audio_codec.ffmpeg_name()))?;

    let sample_rate = audio_codec.sample_rate();
    let channels = audio_codec.channels();
    let sample_fmt = match audio_codec {
        AudioCodec::Opus => ffi::AV_SAMPLE_FMT_FLT,
        AudioCodec::G722 => ffi::AV_SAMPLE_FMT_S16,
    };

    let mut codec_ctx = AVCodecContext::new(&codec);
    codec_ctx.set_sample_rate(sample_rate);
    codec_ctx.set_sample_fmt(sample_fmt);
    codec_ctx.set_time_base(ffi::AVRational {
        num: 1,
        den: sample_rate,
    });

    let ch_layout = AVChannelLayout::from_nb_channels(channels);
    codec_ctx.set_ch_layout(ch_layout.into_inner());

    let mut opts = AVDictionary::new(c"application", c"audio", 0);
    if matches!(audio_codec, AudioCodec::Opus) {
        opts = opts.set(c"vbr", c"off", 0);
    }

    codec_ctx
        .open(Some(opts))
        .with_context(|| format!("Failed to open {} encoder", audio_codec.ffmpeg_name()))?;

    Ok(OpenedAudioOutput { codec_ctx })
}

fn encode_video_frame(
    ctx: &mut OutputContext,
    frame_index: u64,
    pts: i64,
    time_base: ffi::AVRational,
) -> Result<Option<EncodedFrame>> {
    let codec_ctx = &mut ctx.codec_ctx;
    let frame = ctx.frame.as_mut().context("reusable video frame missing")?;
    frame
        .make_writable()
        .context("Failed to make video frame writable")?;
    frame.set_pts(pts);
    frame.set_time_base(time_base);

    fill_test_pattern(
        &frame.data,
        &frame.linesize,
        codec_ctx.width as usize,
        codec_ctx.height as usize,
        frame_index,
    );

    encode_frame(codec_ctx, frame)
}

fn encode_audio_frame(
    ctx: &mut OutputContext,
    pts: i64,
    time_base: ffi::AVRational,
    samples: i32,
) -> Result<Vec<EncodedFrame>> {
    let mut frame = AVFrame::new();
    frame.set_sample_rate(ctx.codec_ctx.sample_rate);
    frame.set_format(ctx.codec_ctx.sample_fmt);
    frame.set_nb_samples(samples);
    frame.set_ch_layout(ctx.codec_ctx.ch_layout);
    frame
        .alloc_buffer()
        .context("Failed to allocate audio frame buffer")?;
    frame
        .make_writable()
        .context("Failed to make audio frame writable")?;
    frame.set_pts(pts);
    frame.set_time_base(time_base);

    fill_sine_wave(
        &frame.data,
        &frame.linesize,
        samples,
        ctx.codec_ctx.sample_rate,
        ctx.codec_ctx.ch_layout.nb_channels,
        ctx.codec_ctx.sample_fmt,
    );

    encode_frame(&mut ctx.codec_ctx, &frame).map(|opt| opt.into_iter().collect())
}

/// Send a frame to the encoder and collect all output packets into a single
/// encoded frame.
fn encode_frame(codec_ctx: &mut AVCodecContext, frame: &AVFrame) -> Result<Option<EncodedFrame>> {
    codec_ctx
        .send_frame(Some(frame))
        .context("Failed to send frame to encoder")?;

    let mut data = Vec::new();
    let mut pts = frame.pts;
    let mut is_keyframe = false;

    loop {
        let packet = match codec_ctx.receive_packet() {
            Ok(packet) => packet,
            Err(rsmpeg::error::RsmpegError::EncoderDrainError) => break,
            Err(e) => return Err(e.into()),
        };

        debug!(pts = packet.pts, size = packet.size, "encoded packet");

        if data.is_empty() {
            pts = packet.pts;
            is_keyframe = (packet.flags & ffi::AV_PKT_FLAG_KEY as i32) != 0;
        } else {
            // If multiple packets are returned for a single input frame,
            // concatenate them. Preserve start codes already present in the
            // Annex-B bitstream.
            is_keyframe |= (packet.flags & ffi::AV_PKT_FLAG_KEY as i32) != 0;
        }
        let slice = unsafe { std::slice::from_raw_parts(packet.data, packet.size as usize) };
        data.extend_from_slice(slice);
    }

    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(EncodedFrame {
            data,
            pts,
            time_base: codec_ctx.time_base,
            is_keyframe,
        }))
    }
}

fn flush_encoder(ctx: &mut OutputContext) -> Result<Vec<EncodedFrame>> {
    ctx.codec_ctx
        .send_frame(None)
        .context("Failed to send flush frame")?;

    let mut frames = Vec::new();
    loop {
        let packet = match ctx.codec_ctx.receive_packet() {
            Ok(packet) => packet,
            Err(rsmpeg::error::RsmpegError::EncoderDrainError) => break,
            Err(e) => return Err(e.into()),
        };
        let data = unsafe { std::slice::from_raw_parts(packet.data, packet.size as usize) };
        frames.push(EncodedFrame {
            data: data.to_vec(),
            pts: packet.pts,
            time_base: ctx.codec_ctx.time_base,
            is_keyframe: (packet.flags & ffi::AV_PKT_FLAG_KEY as i32) != 0,
        });
    }
    Ok(frames)
}

/// Fill a YUV420P frame with a moving color-bar test pattern.
fn fill_test_pattern(
    data: &[*mut u8],
    linesize: &[i32],
    width: usize,
    height: usize,
    frame_index: u64,
) {
    assert!(
        linesize.len() >= 3 && data.len() >= 3,
        "fill_test_pattern expects at least three planes"
    );
    assert!(
        linesize[0] > 0 && linesize[1] > 0 && linesize[2] > 0,
        "video linesizes must be positive, got {:?}",
        linesize
    );

    let y_stride = linesize[0] as usize;
    let u_stride = linesize[1] as usize;
    let v_stride = linesize[2] as usize;
    let y_ptr = data[0];
    let u_ptr = data[1];
    let v_ptr = data[2];

    assert!(!y_ptr.is_null() && !u_ptr.is_null() && !v_ptr.is_null());
    assert!(
        y_stride >= width,
        "Y stride {y_stride} smaller than width {width}"
    );
    assert!(
        u_stride >= width / 2,
        "U stride {u_stride} smaller than width/2"
    );
    assert!(
        v_stride >= width / 2,
        "V stride {v_stride} smaller than width/2"
    );

    let shift = (frame_index % width as u64) as usize;

    unsafe {
        for y in 0..height {
            for x in 0..width {
                let bar = ((x + shift) * 8 / width) % 8;
                let value = match bar {
                    0 => 180u8,
                    1 => 180,
                    2 => 168,
                    3 => 16,
                    4 => 133,
                    5 => 63,
                    6 => 16,
                    _ => 128,
                };
                *y_ptr.add(y * y_stride + x) = value;
            }
        }

        for y in 0..height / 2 {
            for x in 0..width / 2 {
                let bar = ((x * 2 + shift) * 8 / width) % 8;
                let (u, v) = match bar {
                    0 => (128u8, 128u8),
                    1 => (128, 128),
                    2 => (44, 255),
                    3 => (255, 107),
                    4 => (202, 21),
                    5 => (63, 193),
                    6 => (255, 81),
                    _ => (128, 128),
                };
                *u_ptr.add(y * u_stride + x) = u;
                *v_ptr.add(y * v_stride + x) = v;
            }
        }
    }
}

/// Fill an interleaved audio buffer with a sine wave.
fn fill_sine_wave(
    data: &[*mut u8],
    linesize: &[i32],
    samples: i32,
    sample_rate: i32,
    channels: i32,
    sample_fmt: i32,
) {
    if data.is_empty() || data[0].is_null() || linesize.is_empty() {
        return;
    }
    assert!(
        linesize[0] > 0,
        "audio linesize must be positive, got {}",
        linesize[0]
    );

    let freq = 440.0f32;
    let sample_rate = sample_rate as f32;
    let total_samples = (samples * channels) as usize;

    let sample_size = if sample_fmt == ffi::AV_SAMPLE_FMT_FLT {
        std::mem::size_of::<f32>()
    } else if sample_fmt == ffi::AV_SAMPLE_FMT_S16 {
        std::mem::size_of::<i16>()
    } else {
        return;
    };
    let buffer_size = linesize[0] as usize;
    assert!(
        buffer_size >= total_samples * sample_size,
        "audio buffer too small: {} bytes for {} samples of size {}",
        linesize[0],
        total_samples,
        sample_size
    );

    unsafe {
        if sample_fmt == ffi::AV_SAMPLE_FMT_FLT {
            let ptr = data[0] as *mut f32;
            for i in 0..total_samples {
                let t = i as f32 / sample_rate;
                *ptr.add(i) = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5;
            }
        } else if sample_fmt == ffi::AV_SAMPLE_FMT_S16 {
            let ptr = data[0] as *mut i16;
            for i in 0..total_samples {
                let t = i as f32 / sample_rate;
                let value =
                    ((2.0 * std::f32::consts::PI * freq * t).sin() * 0.5 * i16::MAX as f32) as i16;
                *ptr.add(i) = value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for live777#169: AV1 must emit key frames frequently
    /// enough for real-time WHIP streaming. The SVT-AV1 encoder ignores the
    /// FFmpeg GOP size by default and only emits key frames every ~5 s; the
    /// generator therefore configures an explicit low-delay prediction
    /// structure and `keyint=fps`.
    #[test]
    fn av1_low_delay_emits_regular_keyframes() {
        let config = FrameGeneratorConfig {
            video_codec: VideoCodec::Av1,
            audio_codec: None,
            width: 128,
            height: 128,
            fps: 30,
            duration: None,
        };

        let mut generator = FrameGenerator::new(&config).expect("create AV1 generator");
        let mut keyframes = 0;
        let mut frames = 0;
        while frames < 32 {
            match generator.next_frame().expect("generate frame") {
                SourceFrame::Frame(MediaFrame::Video(frame)) => {
                    assert!(
                        !frame.data.is_empty(),
                        "encoded AV1 frame must not be empty"
                    );
                    if frame.is_keyframe {
                        keyframes += 1;
                    }
                    frames += 1;
                }
                SourceFrame::Frame(MediaFrame::Audio(_)) => {
                    panic!("AV1-only generator produced an audio frame")
                }
                SourceFrame::Empty => {
                    // Encoder may buffer briefly; keep polling.
                }
                SourceFrame::End => panic!("generator ended before target frame count"),
            }
        }

        assert!(
            keyframes >= 2,
            "expected at least 2 key frames in 32 AV1 frames, got {keyframes}"
        );
    }
}
