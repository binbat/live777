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
/// Maximum decode failures allowed before the first successful frame. This
/// prevents obviously broken streams (wrong sprop, corrupted first packet,
/// mismatched codec) from spinning until the outer timeout, while still
/// tolerating startup noise such as VP8 interframes before the first keyframe.
const MAX_EARLY_FAILURES: u32 = 100;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodedAudio {
    pub frame_count: u32,
    pub sample_rate: u32,
    pub channels: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Video,
    Audio,
}

#[derive(Debug, Default)]
struct RtpTimestampExtender {
    extended: Option<i64>,
}

impl RtpTimestampExtender {
    fn extend(&mut self, timestamp: u32) -> i64 {
        let next = match self.extended {
            Some(previous) => previous + (timestamp.wrapping_sub(previous as u32) as i32) as i64,
            None => timestamp as i64,
        };
        self.extended = Some(next);
        next
    }
}

pub struct RtpFrameDecoder {
    repayload: Option<RePayloadCodec>,
    media_kind: MediaKind,
    frame_count: u32,
    width: u32,
    height: u32,
    sample_rate: u32,
    channels: u32,
    codec_context: AVCodecContext,
    codec_context_open: bool,
    early_failure_count: u32,
    timestamps: RtpTimestampExtender,
}

/// Map a lowercased track mime type to the matching FFmpeg decoder id.
fn codec_id_for_mime(mime_lc: &str) -> Option<ffi::AVCodecID> {
    match mime_lc {
        "video/vp8" => Some(ffi::AV_CODEC_ID_VP8),
        "video/vp9" => Some(ffi::AV_CODEC_ID_VP9),
        "video/h264" => Some(ffi::AV_CODEC_ID_H264),
        "video/hevc" | "video/h265" => Some(ffi::AV_CODEC_ID_HEVC),
        "video/av1" => Some(ffi::AV_CODEC_ID_AV1),
        "audio/opus" => Some(ffi::AV_CODEC_ID_OPUS),
        "audio/g722" => Some(ffi::AV_CODEC_ID_ADPCM_G722),
        "audio/pcmu" => Some(ffi::AV_CODEC_ID_PCM_MULAW),
        "audio/pcma" => Some(ffi::AV_CODEC_ID_PCM_ALAW),
        _ => None,
    }
}

fn rtp_clock_rate(mime_lc: &str) -> i32 {
    match mime_lc {
        "audio/opus" => 48_000,
        "audio/g722" | "audio/pcmu" | "audio/pcma" => 8_000,
        _ => 90_000,
    }
}

/// Whether the FFI decoder can handle the given track mime type.
pub fn supports_mime(mime_type: &str) -> bool {
    codec_id_for_mime(&mime_type.to_ascii_lowercase()).is_some()
}

impl RtpFrameDecoder {
    pub fn new(mime_type: impl Into<String>, sprop_params: Option<&str>) -> Result<Self> {
        let mime_type = mime_type.into();
        let mime_lc = mime_type.to_ascii_lowercase();

        let codec_id = codec_id_for_mime(&mime_lc)
            .ok_or_else(|| anyhow!("Unsupported codec for FFI decoding: {mime_type}"))?;
        let media_kind = if mime_lc.starts_with("audio/") {
            MediaKind::Audio
        } else {
            MediaKind::Video
        };

        let decoder = if mime_lc == "video/av1" {
            [c"libdav1d", c"libaom-av1"]
                .into_iter()
                .find_map(rsmpeg::avcodec::AVCodec::find_decoder_by_name)
                .or_else(|| rsmpeg::avcodec::AVCodec::find_decoder(codec_id))
        } else {
            rsmpeg::avcodec::AVCodec::find_decoder(codec_id)
        }
        .ok_or_else(|| anyhow!("Failed to find FFmpeg decoder for {mime_type}"))?;
        let mut codec_context = AVCodecContext::new(&decoder);
        codec_context.set_pkt_timebase(ffi::AVRational {
            num: 1,
            den: rtp_clock_rate(&mime_lc),
        });
        let audio_parameters = match mime_lc.as_str() {
            "audio/opus" => Some((48_000, 2)),
            "audio/g722" => Some((16_000, 1)),
            "audio/pcmu" | "audio/pcma" => Some((8_000, 1)),
            _ => None,
        };
        if let Some((sample_rate, channels)) = audio_parameters {
            codec_context.set_sample_rate(sample_rate);
            codec_context.set_ch_layout(
                rsmpeg::avutil::AVChannelLayout::from_nb_channels(channels).into_inner(),
            );
        }

        let mut repayload =
            (media_kind == MediaKind::Video).then(|| RePayloadCodec::new(mime_type.clone()));

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
                repayload
                    .as_mut()
                    .expect("video decoder has a repayloader")
                    .set_h264_params(sps, pps);
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
                repayload
                    .as_mut()
                    .expect("video decoder has a repayloader")
                    .set_h265_params(vps, sps, pps);
            }
        }

        Ok(Self {
            repayload,
            media_kind,
            frame_count: 0,
            width: 0,
            height: 0,
            sample_rate: 0,
            channels: 0,
            codec_context,
            codec_context_open: false,
            early_failure_count: 0,
            timestamps: RtpTimestampExtender::default(),
        })
    }

    /// Feed a raw RTP packet into the decoder.
    pub fn feed_rtp(&mut self, raw_rtp: &[u8]) -> Result<()> {
        let packet = Packet::unmarshal(&mut Cursor::new(raw_rtp))
            .map_err(|e| anyhow!("Failed to unmarshal RTP packet: {e}"))?;
        let pts = self.timestamps.extend(packet.header.timestamp);
        let frame_data = match &mut self.repayload {
            Some(repayload) => repayload.process(&packet),
            None => Some(packet.payload.clone()),
        };

        if let Some(frame_data) = frame_data {
            if let Err(e) = self.decode_packet(&frame_data, pts) {
                // Before the first successful frame, errors are usually startup
                // noise (e.g. VP8 interframes before the first keyframe). Once
                // decoding has produced output, treat further errors as fatal.
                if self.frame_count > 0 {
                    return Err(e);
                }
                self.early_failure_count += 1;
                if self.early_failure_count > MAX_EARLY_FAILURES {
                    return Err(e.context(format!(
                        "failed to decode first frame after {MAX_EARLY_FAILURES} attempts"
                    )));
                }
                warn!(
                    attempt = self.early_failure_count,
                    size = frame_data.len(),
                    "Failed to decode frame: {e}"
                );
            } else {
                self.early_failure_count = 0;
            }
        }

        Ok(())
    }

    fn decode_packet(&mut self, data: &[u8], pts: i64) -> Result<()> {
        if !self.codec_context_open {
            self.codec_context
                .open(None)
                .context("Failed to open decoder")?;
            self.codec_context_open = true;
        }

        let mut packet =
            av_packet_from_bytes(data).context("Failed to create AVPacket from frame data")?;
        packet.set_pts(pts);
        packet.set_dts(pts);

        if let Err(e) = self.codec_context.send_packet(Some(&packet)) {
            warn!(size = data.len(), "Failed to send packet to decoder: {e}");
            return Err(e.into());
        }

        self.drain_frames()
    }

    /// Drain all decoded frames currently available from the decoder.
    fn drain_frames(&mut self) -> Result<()> {
        loop {
            match self.codec_context.receive_frame() {
                Ok(frame) => {
                    self.frame_count += 1;
                    match self.media_kind {
                        MediaKind::Video => {
                            self.width = frame.width as u32;
                            self.height = frame.height as u32;
                            debug!(
                                "Decoded frame {} {}x{}",
                                self.frame_count, self.width, self.height
                            );
                        }
                        MediaKind::Audio => {
                            self.sample_rate = frame.sample_rate as u32;
                            self.channels = frame.ch_layout.nb_channels as u32;
                            debug!(
                                "Decoded audio frame {} ({} Hz, {} channels)",
                                self.frame_count, self.sample_rate, self.channels
                            );
                        }
                    }
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

    fn flush(&mut self) -> Result<()> {
        if !self.codec_context_open {
            return Ok(());
        }
        if let Err(e) = self.codec_context.send_packet(None)
            && !matches!(
                e,
                rsmpeg::error::RsmpegError::DecoderDrainError
                    | rsmpeg::error::RsmpegError::DecoderFlushedError
            )
        {
            return Err(e.into());
        }
        self.drain_frames()
    }

    pub fn finish(&mut self) -> Result<(u32, u32, u32)> {
        self.flush()?;
        Ok((self.width, self.height, self.frame_count))
    }

    pub fn finish_audio(&mut self) -> Result<DecodedAudio> {
        self.flush()?;
        Ok(DecodedAudio {
            frame_count: self.frame_count,
            sample_rate: self.sample_rate,
            channels: self.channels,
        })
    }
}

/// Create an AVPacket that owns a copy of the provided bytes.
///
/// # Safety
///
/// `av_new_packet` allocates a reference-counted buffer owned by the packet.
/// The caller is responsible for calling `av_packet_unref` (done by
/// `AVPacket::drop`). We copy the input bytes into this buffer with
/// `copy_nonoverlapping`, which is sound because `av_new_packet` has just
/// allocated exactly `len_i32` bytes and `(*pkt).data` points to the start
/// of that allocation.
fn av_packet_from_bytes(data: &[u8]) -> Result<AVPacket> {
    let len_i32 = i32::try_from(data.len())
        .map_err(|_| anyhow!("frame data too large for AVPacket: {} bytes", data.len()))?;
    let mut packet = AVPacket::new();
    // SAFETY: av_new_packet allocates a fresh buffer of len_i32 bytes and
    // makes packet own it. The pointer is valid until packet is dropped.
    let ret = unsafe { rsmpeg::ffi::av_new_packet(packet.as_mut_ptr(), len_i32) };
    if ret < 0 {
        return Err(anyhow!("av_new_packet failed with {ret}"));
    }
    // SAFETY: av_new_packet succeeded and should guarantee a valid buffer.
    // We still defensively check for a null data pointer before copying,
    // since a misbehaving FFmpeg build could violate the contract.
    unsafe {
        let pkt = packet.as_mut_ptr();
        let dst = (*pkt).data;
        if dst.is_null() {
            return Err(anyhow!(
                "av_new_packet returned null data pointer for {} bytes",
                data.len()
            ));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
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

pub fn run_ffi_tokio_decoder(
    mime_type: String,
    sprop_params: Option<&str>,
    mut packet_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<(u32, u32, u32)> {
    let mut decoder = RtpFrameDecoder::new(mime_type, sprop_params)?;
    let deadline = std::time::Instant::now() + timeout;

    while std::time::Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
        match packet_rx.blocking_recv() {
            Some(packet) => decoder.feed_rtp(&packet)?,
            None => break,
        }
    }

    decoder.finish()
}

pub fn run_ffi_audio_decoder(
    mime_type: String,
    mut packet_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<DecodedAudio> {
    let mut decoder = RtpFrameDecoder::new(mime_type, None)?;
    let deadline = std::time::Instant::now() + timeout;

    while std::time::Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
        match packet_rx.blocking_recv() {
            Some(packet) => decoder.feed_rtp(&packet)?,
            None => break,
        }
    }

    decoder.finish_audio()
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

    use super::{
        RtpFrameDecoder, RtpTimestampExtender, ffi, parse_h264_sprop, parse_h265_sprop,
        supports_mime,
    };

    #[test]
    fn codec_mapping_supports_av1_and_audio() {
        for mime in [
            "video/av1",
            "audio/opus",
            "audio/g722",
            "audio/pcmu",
            "audio/pcma",
        ] {
            assert!(supports_mime(mime), "missing decoder mapping for {mime}");
            assert!(supports_mime(&mime.to_ascii_uppercase()));
        }
        assert!(!supports_mime("audio/aac"));
        let Err(error) = RtpFrameDecoder::new("audio/aac", None) else {
            panic!("unsupported audio codec was accepted");
        };
        assert!(error.to_string().contains("Unsupported codec"));
        let av1 = rsmpeg::avcodec::AVCodec::find_decoder(ffi::AV_CODEC_ID_AV1).unwrap();
        assert!(["av1", "libaom-av1", "libdav1d"].contains(&av1.name().to_str().unwrap()));
    }

    #[test]
    fn decodes_av1_video() {
        let mut generator =
            crate::source::FrameGenerator::new(&crate::source::FrameGeneratorConfig {
                video_codec: crate::source::VideoCodec::Av1,
                audio_codec: None,
                width: 64,
                height: 64,
                fps: 30,
                duration: None,
            })
            .unwrap();
        let mut decoder = RtpFrameDecoder::new("video/av1", None).unwrap();
        for _ in 0..100 {
            if let crate::whipsynth::source::SourceFrame::Frame(crate::source::MediaFrame::Video(
                frame,
            )) = generator.next_frame().unwrap()
            {
                decoder.decode_packet(&frame.data, frame.pts).unwrap();
                break;
            }
        }
        let (width, height, frames) = decoder.finish().unwrap();
        assert_eq!((width, height), (64, 64));
        assert!(frames > 0);
    }

    #[test]
    fn rtp_timestamp_extender_handles_wrap_and_reordering() {
        let mut timestamps = RtpTimestampExtender::default();
        assert_eq!(timestamps.extend(u32::MAX - 10), u32::MAX as i64 - 10);
        assert_eq!(timestamps.extend(4), u32::MAX as i64 + 5);
        assert_eq!(timestamps.extend(u32::MAX - 5), u32::MAX as i64 - 5);
        assert_eq!(timestamps.extend(6), u32::MAX as i64 + 7);
    }

    #[test]
    fn decodes_and_flushes_opus_audio() {
        assert_audio_decodes(
            c"libopus",
            "audio/opus",
            ffi::AV_SAMPLE_FMT_FLT,
            960,
            48_000,
            2,
        );
    }

    #[test]
    fn decodes_and_flushes_g722_audio() {
        assert_audio_decodes(
            c"g722",
            "audio/g722",
            ffi::AV_SAMPLE_FMT_S16,
            320,
            16_000,
            1,
        );
    }

    #[test]
    fn decodes_and_flushes_pcmu_audio() {
        assert_audio_decodes(
            c"pcm_mulaw",
            "audio/pcmu",
            ffi::AV_SAMPLE_FMT_S16,
            160,
            8_000,
            1,
        );
    }

    #[test]
    fn decodes_and_flushes_pcma_audio() {
        assert_audio_decodes(
            c"pcm_alaw",
            "audio/pcma",
            ffi::AV_SAMPLE_FMT_S16,
            160,
            8_000,
            1,
        );
    }

    fn assert_audio_decodes(
        encoder_name: &std::ffi::CStr,
        mime: &str,
        sample_format: i32,
        samples: i32,
        sample_rate: u32,
        channels: u32,
    ) {
        let encoder = rsmpeg::avcodec::AVCodec::find_encoder_by_name(encoder_name).unwrap();
        let mut encoder_context = rsmpeg::avcodec::AVCodecContext::new(&encoder);
        encoder_context.set_sample_rate(sample_rate as i32);
        encoder_context.set_sample_fmt(sample_format);
        encoder_context.set_bit_rate(64_000);
        encoder_context.set_time_base(ffi::AVRational {
            num: 1,
            den: sample_rate as i32,
        });
        encoder_context.set_ch_layout(
            rsmpeg::avutil::AVChannelLayout::from_nb_channels(channels as i32).into_inner(),
        );
        encoder_context.open(None).unwrap();

        let mut decoder = RtpFrameDecoder::new(mime, None).unwrap();
        for index in 0..8 {
            let mut frame = rsmpeg::avutil::AVFrame::new();
            frame.set_sample_rate(sample_rate as i32);
            frame.set_format(sample_format);
            frame.set_nb_samples(samples);
            frame.set_ch_layout(encoder_context.ch_layout);
            frame.alloc_buffer().unwrap();
            frame.make_writable().unwrap();
            assert!(!frame.data[0].is_null() && frame.linesize[0] > 0);
            // SAFETY: alloc_buffer allocated linesize[0] bytes for the first
            // interleaved audio plane, and silence is valid for S16 and FLT.
            unsafe { std::ptr::write_bytes(frame.data[0], 0, frame.linesize[0] as usize) };
            frame.set_pts(index * samples as i64);
            encoder_context.send_frame(Some(&frame)).unwrap();

            loop {
                match encoder_context.receive_packet() {
                    Ok(packet) => {
                        assert!(packet.size > 0 && !packet.data.is_null());
                        // SAFETY: a non-empty encoded AVPacket owns size bytes.
                        let data = unsafe {
                            std::slice::from_raw_parts(packet.data, packet.size as usize)
                        };
                        decoder.decode_packet(data, packet.pts).unwrap();
                    }
                    Err(rsmpeg::error::RsmpegError::EncoderDrainError) => break,
                    Err(e) => panic!("audio encoder failed: {e}"),
                }
            }
        }

        let decoded = decoder.finish_audio().unwrap();
        assert!(decoded.frame_count > 0);
        assert_eq!(decoded.sample_rate, sample_rate);
        assert_eq!(decoded.channels, channels);
    }

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
