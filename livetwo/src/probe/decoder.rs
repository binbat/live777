use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use rsmpeg::{
    avcodec::{AVCodecContext, AVPacket},
    ffi,
};
use rtc::rtp::packet::Packet;
use rtc_shared::marshal::Unmarshal;
use std::sync::mpsc;
use tracing::{debug, warn};

use crate::payload::{RePayload, RePayloadCodec};

/// Assembles RTP payloads into complete encoded frames and decodes them with
/// FFmpeg through rsmpeg.
/// Number of consecutive decode errors tolerated before the first successful
/// frame. This avoids failing on startup noise (e.g. VP8 interframes before the
/// first keyframe) while still surfacing persistent stream corruption.
const STARTUP_ERROR_THRESHOLD: u32 = 10;

pub struct RtpFrameDecoder {
    repayload: RePayloadCodec,
    frame_count: u32,
    width: u32,
    height: u32,
    codec_context: AVCodecContext,
    codec_context_open: bool,
    consecutive_failures: u32,
}

impl RtpFrameDecoder {
    pub fn new(mime_type: impl Into<String>, sprop_params: Option<&str>) -> Result<Self> {
        let mime_type = mime_type.into();
        let mime_lc = mime_type.to_ascii_lowercase();

        let codec_id = match mime_lc.as_str() {
            "video/vp8" => ffi::AV_CODEC_ID_VP8,
            "video/vp9" => ffi::AV_CODEC_ID_VP9,
            "video/h264" => ffi::AV_CODEC_ID_H264,
            "video/hevc" | "video/h265" => ffi::AV_CODEC_ID_HEVC,
            _ => return Err(anyhow!("Unsupported codec for FFI decoding: {mime_type}")),
        };

        let decoder = rsmpeg::avcodec::AVCodec::find_decoder(codec_id)
            .ok_or_else(|| anyhow!("Failed to find FFmpeg decoder for {mime_type}"))?;
        let codec_context = AVCodecContext::new(&decoder);

        let mut repayload = RePayloadCodec::new(mime_type.clone());

        if let Some(params) = sprop_params {
            debug!("Seeding decoder from sprop params: {params}");
            if mime_lc == "video/h264"
                && let Some((sps, pps)) = parse_h264_sprop(params)
            {
                debug!(
                    "Loaded H.264 SPS ({} bytes) and PPS ({} bytes)",
                    sps.len(),
                    pps.len()
                );
                repayload.set_h264_params(sps, pps);
            }
            if (mime_lc == "video/hevc" || mime_lc == "video/h265")
                && let Some((vps, sps, pps)) = parse_h265_sprop(params)
            {
                debug!(
                    "Loaded H.265 VPS/{} SPS/{} PPS/{} bytes",
                    vps.len(),
                    sps.len(),
                    pps.len()
                );
                repayload.set_h265_params(vps, sps, pps);
            }
        }

        Ok(Self {
            repayload,
            frame_count: 0,
            width: 0,
            height: 0,
            codec_context,
            codec_context_open: false,
            consecutive_failures: 0,
        })
    }

    /// Feed a raw RTP packet into the decoder.
    pub fn feed_rtp(&mut self, raw_rtp: &[u8]) -> Result<()> {
        let packet = Packet::unmarshal(&mut Cursor::new(raw_rtp))
            .map_err(|e| anyhow!("Failed to unmarshal RTP packet: {e}"))?;

        if let Some(frame_data) = self.repayload.process(&packet) {
            if let Err(e) = self.decode_packet(&frame_data) {
                self.consecutive_failures += 1;
                // Before the first successful frame, errors are usually startup
                // noise (e.g. VP8 interframes before the first keyframe). Once
                // decoding has produced output, treat further errors as fatal.
                // We also fail if we see many consecutive errors before the
                // first frame, which indicates a real problem such as a
                // parameter-set mismatch.
                if self.frame_count > 0 || self.consecutive_failures >= STARTUP_ERROR_THRESHOLD {
                    return Err(e);
                }
                warn!(size = frame_data.len(), "Failed to decode frame: {e}");
            } else {
                self.consecutive_failures = 0;
            }
        }

        Ok(())
    }

    fn decode_packet(&mut self, data: &[u8]) -> Result<()> {
        if !self.codec_context_open {
            self.codec_context
                .open(None)
                .context("Failed to open decoder")?;
            self.codec_context_open = true;
        }

        let packet =
            av_packet_from_bytes(data).context("Failed to create AVPacket from frame data")?;

        if let Err(e) = self.codec_context.send_packet(Some(&packet)) {
            warn!(size = data.len(), "Failed to send packet to decoder: {e}");
            return Err(e.into());
        }

        loop {
            match self.codec_context.receive_frame() {
                Ok(frame) => {
                    self.frame_count += 1;
                    self.width = frame.width as u32;
                    self.height = frame.height as u32;
                    debug!(
                        "Decoded frame {} {}x{}",
                        self.frame_count, self.width, self.height
                    );
                }
                Err(
                    rsmpeg::error::RsmpegError::DecoderDrainError
                    | rsmpeg::error::RsmpegError::DecoderFlushedError,
                ) => break,
                Err(e) => return Err(e.into()),
            }
        }

        Ok(())
    }

    pub fn finish(&mut self) -> Result<(u32, u32, u32)> {
        // If no frames were ever fed, the codec context was never opened;
        // just return zeros instead of trying to flush an uninitialized decoder.
        if !self.codec_context_open {
            return Ok((self.width, self.height, self.frame_count));
        }

        // The decoder may already be drained/flushed if the last processed
        // packet produced an EOF signal; ignore those cases and read any
        // trailing frames.
        if let Err(e) = self.codec_context.send_packet(None)
            && !matches!(
                e,
                rsmpeg::error::RsmpegError::DecoderDrainError
                    | rsmpeg::error::RsmpegError::DecoderFlushedError
            )
        {
            return Err(e.into());
        }
        loop {
            match self.codec_context.receive_frame() {
                Ok(frame) => {
                    self.frame_count += 1;
                    self.width = frame.width as u32;
                    self.height = frame.height as u32;
                }
                Err(
                    rsmpeg::error::RsmpegError::DecoderDrainError
                    | rsmpeg::error::RsmpegError::DecoderFlushedError,
                ) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok((self.width, self.height, self.frame_count))
    }
}

/// Create an AVPacket that owns a copy of the provided bytes.
fn av_packet_from_bytes(data: &[u8]) -> Result<AVPacket> {
    let len_i32 = i32::try_from(data.len())
        .map_err(|_| anyhow!("frame data too large for AVPacket: {} bytes", data.len()))?;
    let mut packet = AVPacket::new();
    let ret = unsafe { rsmpeg::ffi::av_new_packet(packet.as_mut_ptr(), len_i32) };
    if ret < 0 {
        return Err(anyhow!("av_new_packet failed with {ret}"));
    }
    unsafe {
        let pkt = packet.as_mut_ptr();
        std::ptr::copy_nonoverlapping(data.as_ptr(), (*pkt).data, data.len());
    }
    Ok(packet)
}

/// Run a blocking FFmpeg decoder that reads RTP packets from `packet_rx` and
/// decodes them until the cancellation token fires or the timeout expires.
pub fn run_ffi_decoder(
    mime_type: String,
    sprop_params: Option<&str>,
    packet_rx: mpsc::Receiver<Vec<u8>>,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<(u32, u32, u32)> {
    let mut decoder = RtpFrameDecoder::new(mime_type, sprop_params)?;
    let deadline = std::time::Instant::now() + timeout;

    while std::time::Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
        match packet_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(packet) => decoder.feed_rtp(&packet)?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    decoder.finish()
}

/// Parse H.264 `sprop-parameter-sets=BASE64_SPS,BASE64_PPS` and return the
/// decoded SPS and PPS NAL units (without Annex-B start codes).
fn parse_h264_sprop(params: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let value = params
        .split(';')
        .filter_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k.trim().eq_ignore_ascii_case("sprop-parameter-sets")).then_some(v.trim())
        })
        .next()?;

    let mut parts = value.split(',');
    let sps_b64 = parts.next()?;
    let pps_b64 = parts.next()?;
    let sps = base64::engine::general_purpose::STANDARD
        .decode(sps_b64)
        .ok()?;
    let pps = base64::engine::general_purpose::STANDARD
        .decode(pps_b64)
        .ok()?;
    Some((sps, pps))
}

/// Parse H.265 `sprop-vps=...;sprop-sps=...;sprop-pps=...` and return the
/// decoded VPS, SPS and PPS NAL units (without Annex-B start codes).
fn parse_h265_sprop(params: &str) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let mut vps = None;
    let mut sps = None;
    let mut pps = None;

    for part in params.split(';') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        let key = k.trim();
        let value = v.trim();
        let decoded = match base64::engine::general_purpose::STANDARD.decode(value) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if key.eq_ignore_ascii_case("sprop-vps") {
            vps = Some(decoded);
        } else if key.eq_ignore_ascii_case("sprop-sps") {
            sps = Some(decoded);
        } else if key.eq_ignore_ascii_case("sprop-pps") {
            pps = Some(decoded);
        }
    }

    Some((vps?, sps?, pps?))
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    use super::{parse_h264_sprop, parse_h265_sprop};

    #[test]
    fn parse_h264_sprop_extracts_sps_pps() {
        let sps = vec![0x67, 0x42, 0x00, 0x1f, 0xe9, 0x01, 0x68, 0x1a, 0x74];
        let pps = vec![0x68, 0xce, 0x3c, 0x80];
        let params = format!(
            "profile-level-id=42001f;packetization-mode=1;sprop-parameter-sets={},{};",
            base64::engine::general_purpose::STANDARD.encode(&sps),
            base64::engine::general_purpose::STANDARD.encode(&pps),
        );

        let (parsed_sps, parsed_pps) = parse_h264_sprop(&params).expect("parse failed");
        assert_eq!(parsed_sps, sps);
        assert_eq!(parsed_pps, pps);
    }

    #[test]
    fn parse_h264_sprop_missing_pps_returns_none() {
        let sps = vec![0x67, 0x42, 0x00, 0x1f];
        let params = format!(
            "sprop-parameter-sets={}",
            base64::engine::general_purpose::STANDARD.encode(&sps)
        );
        assert!(parse_h264_sprop(&params).is_none());
    }

    #[test]
    fn parse_h265_sprop_extracts_vps_sps_pps() {
        let vps = vec![0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60];
        let sps = vec![0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90];
        let pps = vec![0x44, 0x01, 0xc1, 0x72, 0xb4, 0x62, 0x40];
        let params = format!(
            "profile-id=1;tier-flag=0;sprop-vps={};sprop-sps={};sprop-pps={}",
            base64::engine::general_purpose::STANDARD.encode(&vps),
            base64::engine::general_purpose::STANDARD.encode(&sps),
            base64::engine::general_purpose::STANDARD.encode(&pps),
        );

        let (parsed_vps, parsed_sps, parsed_pps) = parse_h265_sprop(&params).expect("parse failed");
        assert_eq!(parsed_vps, vps);
        assert_eq!(parsed_sps, sps);
        assert_eq!(parsed_pps, pps);
    }

    #[test]
    fn parse_h265_sprop_missing_vps_returns_none() {
        let sps = vec![0x42, 0x01, 0x01, 0x01];
        let pps = vec![0x44, 0x01, 0xc1];
        let params = format!(
            "profile-id=1;sprop-sps={};sprop-pps={}",
            base64::engine::general_purpose::STANDARD.encode(&sps),
            base64::engine::general_purpose::STANDARD.encode(&pps),
        );
        assert!(parse_h265_sprop(&params).is_none());
    }

    #[test]
    fn parse_h265_sprop_ignores_non_base64_fields() {
        let vps = vec![0x40, 0x01];
        let sps = vec![0x42, 0x01];
        let pps = vec![0x44, 0x01];
        let params = format!(
            "profile-id=1;tier-flag=0;sprop-vps={};sprop-sps={};sprop-pps={}",
            base64::engine::general_purpose::STANDARD.encode(&vps),
            base64::engine::general_purpose::STANDARD.encode(&sps),
            base64::engine::general_purpose::STANDARD.encode(&pps),
        );

        // profile-id=1 and tier-flag=0 are not valid base64, parser should
        // skip them and still extract the parameter sets.
        let (parsed_vps, parsed_sps, parsed_pps) = parse_h265_sprop(&params).expect("parse failed");
        assert_eq!(parsed_vps, vps);
        assert_eq!(parsed_sps, sps);
        assert_eq!(parsed_pps, pps);
    }
}
