use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use rsmpeg::avcodec::{AVCodec, AVCodecContext};
use rsmpeg::avutil::{AVDictionary, AVFrame};
use rsmpeg::ffi;

/// Supported video codecs for the synthetic generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Vp8,
    Vp9,
    H264,
    H265,
    Av1,
}

/// Supported audio codecs for the synthetic generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Opus,
    G722,
}

impl VideoCodec {
    pub fn from_cli(codec: cli::Codec) -> Option<Self> {
        match codec {
            cli::Codec::Vp8 => Some(VideoCodec::Vp8),
            cli::Codec::Vp9 => Some(VideoCodec::Vp9),
            cli::Codec::H264 => Some(VideoCodec::H264),
            cli::Codec::H265 => Some(VideoCodec::H265),
            cli::Codec::AV1 => Some(VideoCodec::Av1),
            _ => None,
        }
    }

    pub(crate) fn ffmpeg_encoder(&self) -> &'static CStr {
        match self {
            VideoCodec::Vp8 => c"libvpx",
            VideoCodec::Vp9 => c"libvpx-vp9",
            VideoCodec::H264 => c"libx264",
            VideoCodec::H265 => c"libx265",
            VideoCodec::Av1 => c"libsvtav1",
        }
    }

    pub(crate) fn ffmpeg_name(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "VP8",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::H264 => "H264",
            VideoCodec::H265 => "H265",
            VideoCodec::Av1 => "AV1",
        }
    }

    /// Default RTP payload type for this codec in the webrtc-rs media engine.
    ///
    /// These values match `MediaEngine::register_default_codecs` defaults so
    /// that WHIP/WHEP negotiation does not remap the payload type. This is the
    /// canonical definition; other modules reference it rather than
    /// duplicating the table.
    pub(crate) fn payload_type(&self) -> u8 {
        match self {
            VideoCodec::Vp8 => 96,
            VideoCodec::Vp9 => 98,
            VideoCodec::H264 => 102,
            // Match the rtc media engine default for H265 so that WHIP/WHEP
            // negotiation does not remap the payload type.
            VideoCodec::H265 => 126,
            VideoCodec::Av1 => 41,
        }
    }
}

impl AudioCodec {
    pub fn from_cli(codec: cli::Codec) -> Option<Self> {
        match codec {
            cli::Codec::Opus => Some(AudioCodec::Opus),
            cli::Codec::G722 => Some(AudioCodec::G722),
            _ => None,
        }
    }

    pub(crate) fn ffmpeg_encoder(&self) -> &'static CStr {
        match self {
            AudioCodec::Opus => c"libopus",
            AudioCodec::G722 => c"g722",
        }
    }

    pub(crate) fn ffmpeg_name(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "OPUS",
            AudioCodec::G722 => "G722",
        }
    }

    /// Default RTP payload type for this codec in the webrtc-rs media engine.
    ///
    /// These values match `MediaEngine::register_default_codecs` defaults so
    /// that WHIP/WHEP negotiation does not remap the payload type. This is the
    /// canonical definition; other modules reference it rather than
    /// duplicating the table.
    pub(crate) fn payload_type(&self) -> u8 {
        match self {
            AudioCodec::Opus => 111,
            AudioCodec::G722 => 9,
        }
    }

    pub(crate) fn sample_rate(&self) -> i32 {
        match self {
            // Opus always uses 48 kHz internally.
            AudioCodec::Opus => 48000,
            // G722 actual sample rate is 16 kHz.
            AudioCodec::G722 => 16000,
        }
    }

    pub(crate) fn rtp_clock_rate(&self) -> i32 {
        match self {
            AudioCodec::Opus => 48000,
            AudioCodec::G722 => 8000,
        }
    }

    pub(crate) fn channels(&self) -> i32 {
        match self {
            AudioCodec::Opus => 2,
            AudioCodec::G722 => 1,
        }
    }
}

/// Parse HEVC parameter sets from an Annex B bitstream and return base64-encoded
/// VPS, SPS and PPS.
fn parse_annex_b_hevc_parameter_sets(data: &[u8]) -> Option<(String, String, String)> {
    let (vps, sps, pps) = crate::payload::extract_hevc_parameter_sets(data)?;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    Some((encode(&vps), encode(&sps), encode(&pps)))
}

/// Key for the H265 sprop cache: `(width, height, fps)`.
type H265SpropKey = (u32, u32, u32);
/// Cached value for an H265 sprop string, or `None` if extraction failed.
type H265SpropValue = Option<String>;

/// Cache of `(width, height, fps)` → sprop string. Opening the libx265
/// encoder is expensive (~100 ms+), so we memoize results keyed by the
/// resolution and frame rate triplet.
static H265_SPROP_CACHE: LazyLock<Mutex<HashMap<H265SpropKey, H265SpropValue>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Drain all pending encoded packets from `codec_ctx` into `encoded`.
///
/// Stops on encoder drain (`EncoderDrainError`) or any other receive error
/// (logged at debug level). Mirrors the drain pattern used by the frame
/// generator's `encode_frame` / `flush_encoder`.
fn drain_encoded_packets(codec_ctx: &mut AVCodecContext, encoded: &mut Vec<u8>) {
    loop {
        match codec_ctx.receive_packet() {
            Ok(packet) if packet.size > 0 => {
                let data = unsafe { std::slice::from_raw_parts(packet.data, packet.size as usize) };
                encoded.extend_from_slice(data);
            }
            Ok(_) => {}
            Err(rsmpeg::error::RsmpegError::EncoderDrainError) => break,
            Err(e) => {
                tracing::debug!(error = ?e, "H265 sprop extraction: failed to receive packet");
                break;
            }
        }
    }
}

/// libx265 encoder options (preset / tune / crf) shared by the frame generator
/// and the H265 sprop extractor, so the advertised sprop is derived from the
/// same encoder configuration as the actual streamed bitstream.
pub(crate) fn h265_encoder_opts() -> AVDictionary {
    let mut opts = AVDictionary::new(c"preset", c"ultrafast", 0);
    opts = opts.set(c"tune", c"zerolatency", 0);
    opts = opts.set(c"crf", c"28", 0);
    opts
}

/// Open a temporary H265 encoder, encode a few blank frames and extract the SDP
/// sprop parameters from the emitted Annex B parameter sets. Returns a string
/// like `sprop-vps=...;sprop-sps=...;sprop-pps=...`.
///
/// Results are cached internally; repeated calls with the same `(width, height,
/// fps)` tuple return the cached result without re-opening the encoder.
pub fn extract_h265_sprop(width: u32, height: u32, fps: u32) -> Option<String> {
    let key = (width, height, fps);
    if let Some(cached) = H265_SPROP_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        return cached.clone();
    }

    let codec = match AVCodec::find_encoder_by_name(c"libx265") {
        Some(codec) => codec,
        None => {
            tracing::debug!("H265 sprop extraction: libx265 encoder not found");
            // Do not cache failures; the encoder may be installed later.
            return None;
        }
    };
    // H.265/x265 requires even dimensions for YUV420P. Round up to the next
    // even value so the advertised sprop resolution matches the actual encoded
    // stream produced by FrameGenerator.
    let width = (width.max(2) + 1) & !1;
    let height = (height.max(2) + 1) & !1;

    let mut codec_ctx = AVCodecContext::new(&codec);
    codec_ctx.set_width(width as i32);
    codec_ctx.set_height(height as i32);
    codec_ctx.set_time_base(ffi::AVRational {
        num: 1,
        den: fps as i32,
    });
    codec_ctx.set_framerate(ffi::AVRational {
        num: fps as i32,
        den: 1,
    });
    codec_ctx.set_pix_fmt(ffi::AV_PIX_FMT_YUV420P);
    codec_ctx.set_gop_size(fps as i32);
    codec_ctx.set_max_b_frames(0);

    let opts = h265_encoder_opts();

    if let Err(e) = codec_ctx.open(Some(opts)) {
        tracing::debug!(error = ?e, "H265 sprop extraction: failed to open libx265 encoder");
        return None;
    }

    let time_base = ffi::AVRational {
        num: 1,
        den: fps as i32,
    };
    let mut encoded = Vec::new();

    for i in 0..5 {
        let mut frame = AVFrame::new();
        frame.set_width(width as i32);
        frame.set_height(height as i32);
        frame.set_format(ffi::AV_PIX_FMT_YUV420P);
        if let Err(e) = frame.alloc_buffer() {
            tracing::debug!(error = ?e, frame = i, "H265 sprop extraction: failed to allocate frame buffer");
            continue;
        }
        if let Err(e) = frame.make_writable() {
            tracing::debug!(error = ?e, frame = i, "H265 sprop extraction: failed to make frame writable");
            continue;
        }
        // SAFETY: We've verified that all three plane pointers are non-null and
        // all three line-sizes are positive. The frame is in YUV420P format,
        // so plane sizes are width×height for Y and (width/2)×(height/2) for
        // U and V. `write_bytes` writes exactly the number of bytes that the
        // plane owns (calculated from linesize × height) — no out-of-bounds
        // write is possible.
        unsafe {
            assert!(
                !frame.data[0].is_null() && !frame.data[1].is_null() && !frame.data[2].is_null(),
                "H265 sprop extraction: frame data pointers must not be null"
            );
            assert!(
                frame.linesize[0] > 0 && frame.linesize[1] > 0 && frame.linesize[2] > 0,
                "H265 sprop extraction: frame linesizes must be positive"
            );
            std::ptr::write_bytes(
                frame.data[0],
                0,
                (frame.linesize[0] * height as i32) as usize,
            );
            std::ptr::write_bytes(
                frame.data[1],
                128,
                (frame.linesize[1] * (height as i32 / 2)) as usize,
            );
            std::ptr::write_bytes(
                frame.data[2],
                128,
                (frame.linesize[2] * (height as i32 / 2)) as usize,
            );
        }
        frame.set_pts(i as i64);
        frame.set_time_base(time_base);

        if let Err(e) = codec_ctx.send_frame(Some(&frame)) {
            tracing::debug!(error = ?e, frame = i, "H265 sprop extraction: failed to send frame");
            continue;
        }
        drain_encoded_packets(&mut codec_ctx, &mut encoded);
        if !encoded.is_empty() {
            break;
        }
    }

    if encoded.is_empty() {
        tracing::debug!("H265 sprop extraction: encoder produced no data");
        // Do not cache failures; the next attempt may succeed.
        return None;
    }

    // Flush to collect any trailing parameter sets.
    if let Err(e) = codec_ctx.send_frame(None) {
        tracing::debug!(error = ?e, "H265 sprop extraction: failed to send flush frame");
    }
    drain_encoded_packets(&mut codec_ctx, &mut encoded);

    let (vps, sps, pps) = match parse_annex_b_hevc_parameter_sets(&encoded) {
        Some(params) => params,
        None => {
            tracing::debug!(
                encoded_len = encoded.len(),
                "H265 sprop extraction: failed to parse parameter sets from encoded data"
            );
            // Do not cache failures; a later encoder run may produce valid sets.
            return None;
        }
    };

    // x265 with 8-bit 4:2:0 output produces Main profile (profile-id=1) in the
    // Main tier (tier-flag=0). Advertising these parameters makes the SDP more
    // standards-compliant and lets receivers verify codec compatibility.
    // level-id is omitted; RFC 7798 infers 93 (Level 3.1) when absent, which is
    // higher than the levels used by the synthetic sources here.
    let fmtp = format!("profile-id=1;tier-flag=0;sprop-vps={vps};sprop-sps={sps};sprop-pps={pps}");
    let mut cache = H265_SPROP_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(key, Some(fmtp.clone()));
    Some(fmtp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_h265_sprop_returns_valid_parameter_sets() {
        let sprop = extract_h265_sprop(320, 240, 15).expect("H265 sprop extraction failed");
        assert!(sprop.contains("profile-id=1"));
        assert!(sprop.contains("tier-flag=0"));
        assert!(sprop.contains("sprop-vps="));
        assert!(sprop.contains("sprop-sps="));
        assert!(sprop.contains("sprop-pps="));
        // sprop-* values should be non-empty base64.
        for key in ["sprop-vps", "sprop-sps", "sprop-pps"] {
            let b64 = sprop
                .split(';')
                .find_map(|part| {
                    let (k, v) = part.split_once('=')?;
                    k.trim()
                        .eq_ignore_ascii_case(key)
                        .then(|| v.trim().to_owned())
                })
                .unwrap_or_else(|| panic!("missing {key}"));
            assert!(!b64.is_empty());
            assert!(
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .is_ok()
            );
        }
    }
}
