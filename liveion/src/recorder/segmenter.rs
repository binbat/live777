use crate::recorder::codec::{CodecAdapter, VideoCodec, create_video_adapter};
use crate::recorder::fmp4::{Fmp4Writer, Mp4Sample};
use crate::recorder::pli_backoff::PliBackoff;
use anyhow::Result;
use bytes::Bytes;
use chrono::{SecondsFormat, Utc};
use opendal::Operator;
use tracing::info;

/// Default duration of each segment in seconds
const DEFAULT_SEG_DURATION: u64 = 10;

const MANIFEST_FILENAME: &str = "manifest.mpd";
const VIDEO_INIT_FILENAME: &str = "v_init.m4s";
const AUDIO_INIT_FILENAME: &str = "a_init.m4s";
const VIDEO_SEGMENT_FILENAME_PREFIX: &str = "v_seg_";
const AUDIO_SEGMENT_FILENAME_PREFIX: &str = "a_seg_";
const SEGMENT_FILE_EXTENSION: &str = ".m4s";
const VIDEO_SEGMENT_TEMPLATE: &str = "v_seg_$Number%04d$.m4s";
const AUDIO_SEGMENT_TEMPLATE: &str = "a_seg_$Number%04d$.m4s";

const DEFAULT_AUDIO_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_AUDIO_CHANNELS: u16 = 2;
const DEFAULT_AUDIO_CODEC: &str = "opus";

const VIDEO_TRACK_ID: u32 = 1;
const AUDIO_TRACK_ID: u32 = 2;
const MANIFEST_UPDATE_PERIOD_SECS: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingMediaOutcome {
    Complete,
    Degraded,
    Failed,
}

/// Represents a completed segment with its actual duration
#[derive(Debug, Clone)]
struct SegmentInfo {
    start_time: u64, // Start time in timescale units
    duration: u64,   // Actual duration in timescale units
}

#[derive(Debug, Clone)]
struct ExpectedVideo {
    codec_mime: String,
    payload_type: Option<u8>,
    ssrc: Option<u32>,
}

pub struct Segmenter {
    op: Operator,
    stream: String,
    path_prefix: String,
    uploader: Option<std::sync::Arc<crate::recorder::uploader::UploadManager>>,
    local_dir: Option<std::path::PathBuf>,
    manifest_start_time: chrono::DateTime<Utc>,
    timescale: u32,
    // Length of each segment (in timescale units) for fast comparison
    seg_duration_ticks: u64,

    // fragment index
    video_seg_index: u32,
    // Decode timestamp of current video segment start (in timescale units)
    video_seg_start_dts: u64,
    video_track_id: Option<u32>,

    // Audio track id (Opus)
    audio_track_id: Option<u32>,

    // All video samples buffered for the current segment (already converted to AVCC length prefix)
    video_samples: Vec<Mp4Sample>,

    // Audio samples buffered for current audio segment
    audio_samples: Vec<Mp4Sample>,

    // video info
    video_width: u32,
    video_height: u32,

    // codec string like "avc1.42E01E"
    video_codec: String,

    video_current_pts: u64,

    /// Frames per second, updated dynamically from incoming stream (timescale/duration)
    frame_rate: u32,

    /// Accumulated media size (bytes) for bitrate estimation
    total_bytes: u64,

    /// Accumulated media duration (in timescale ticks)
    total_ticks: u64,

    // encapsulated fmp4 writer once initialized
    fmp4_writer: Option<Fmp4Writer>,

    // audio writer for Opus
    audio_writer: Option<Fmp4Writer>,
    audio_seg_index: u32,
    audio_seg_start_pts: u64,
    // track pts for audio
    audio_current_pts: u64,

    // audio track metadata
    audio_sample_rate: u32,
    audio_channels: u16,
    audio_codec: String,

    // audio bitrate stats
    audio_total_bytes: u64,
    audio_total_ticks: u64,

    // keyframe request tracking with intelligent backoff
    pli_backoff: PliBackoff,

    // active video codec adapter
    video_codec_kind: Option<VideoCodec>,
    video_adapter: Option<Box<dyn CodecAdapter>>,

    /// List of completed segments with their actual durations
    segments: Vec<SegmentInfo>,

    /// Audio segments with their actual durations
    audio_segments: Vec<SegmentInfo>,

    expected_video: Option<ExpectedVideo>,
    pending_video_codec_config: Option<Vec<Vec<u8>>>,
}

impl Segmenter {
    pub async fn new(
        op: Operator,
        stream: String,
        root_prefix: String,
        uploader: Option<std::sync::Arc<crate::recorder::uploader::UploadManager>>,
        local_dir: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            op,
            stream: stream.clone(),
            path_prefix: root_prefix,
            uploader,
            local_dir: local_dir.map(std::path::PathBuf::from),
            manifest_start_time: Utc::now(),
            timescale: 90_000,
            seg_duration_ticks: 90_000u64 * DEFAULT_SEG_DURATION,
            video_seg_index: 0,
            video_seg_start_dts: 0,
            video_track_id: None,

            audio_track_id: None,

            video_samples: Vec::new(),

            audio_samples: Vec::new(),

            video_width: 0,
            video_height: 0,
            video_codec: String::new(),
            video_current_pts: 0,
            frame_rate: 0,
            total_bytes: 0,
            total_ticks: 0,
            fmp4_writer: None,

            audio_writer: None,
            audio_seg_index: 0,
            audio_seg_start_pts: 0,
            audio_current_pts: 0,

            audio_sample_rate: DEFAULT_AUDIO_SAMPLE_RATE,
            audio_channels: DEFAULT_AUDIO_CHANNELS,
            audio_codec: DEFAULT_AUDIO_CODEC.to_string(),

            audio_total_bytes: 0,
            audio_total_ticks: 0,

            pli_backoff: PliBackoff::default(),

            video_codec_kind: None,
            video_adapter: None,
            segments: Vec::new(),
            audio_segments: Vec::new(),
            expected_video: None,
            pending_video_codec_config: None,
        })
    }

    pub fn expect_video_track(
        &mut self,
        codec_mime: impl Into<String>,
        payload_type: Option<u8>,
        ssrc: Option<u32>,
        fmtp: Option<&str>,
    ) {
        let codec_mime = codec_mime.into();
        self.expected_video = Some(ExpectedVideo {
            codec_mime: codec_mime.clone(),
            payload_type,
            ssrc,
        });
        tracing::info!(
            "[segmenter] recorder start stream={} video codec={} payload_type={:?} ssrc={:?} fmtp={}",
            self.stream,
            codec_mime,
            payload_type,
            ssrc,
            fmtp.unwrap_or("")
        );
    }

    pub fn configure_video_from_track_metadata(
        &mut self,
        codec_mime: &str,
        codec_config: Option<Vec<Vec<u8>>>,
        dimensions: Option<(u32, u32)>,
    ) {
        if let Some(codec) = video_codec_from_mime(codec_mime) {
            self.ensure_video_adapter(codec);
        }
        if let Some(expected_video) = self.expected_video.as_mut() {
            expected_video.codec_mime = codec_mime.to_string();
        } else {
            self.expected_video = Some(ExpectedVideo {
                codec_mime: codec_mime.to_string(),
                payload_type: None,
                ssrc: None,
            });
        }
        if let Some(config) = codec_config.filter(|config| !config.is_empty()) {
            self.pending_video_codec_config = Some(config);
            tracing::info!(
                "[segmenter] {} video codec config initialized from track metadata ({})",
                self.stream,
                codec_mime
            );
        }
        if let Some((width, height)) = dimensions
            && width > 0
            && height > 0
        {
            self.video_width = width;
            self.video_height = height;
        }
        self.refresh_video_metadata();
    }

    /// Feed one H.264 Frame (Annex-B format, may contain multiple NALUs)
    /// `duration_ticks` – frame duration in the same timescale as self.timescale (90000 for H264)
    pub async fn push_h264(&mut self, frame: Bytes, duration_ticks: u32) -> Result<()> {
        self.push_video_frame(VideoCodec::H264, frame, None, duration_ticks)
            .await
    }

    /// Feed one H.265 frame (Annex-B format)
    pub async fn push_h265(&mut self, frame: Bytes, duration_ticks: u32) -> Result<()> {
        self.push_video_frame(VideoCodec::H265, frame, None, duration_ticks)
            .await
    }

    /// Feed Opus audio sample from RTP payload
    /// `duration_ticks` – duration in the 48 kHz time base (i.e. RTP timestamp delta)
    pub async fn push_opus(&mut self, payload: Bytes, duration_ticks: u32) -> Result<()> {
        // Initialize writer if not yet done
        if self.audio_track_id.is_none() {
            self.init_audio_writer().await?;
        }

        let size_bytes = payload.len();
        let sample_start = self.audio_current_pts;
        if self.audio_samples.is_empty() {
            self.audio_seg_start_pts = sample_start;
        }
        let sample = Mp4Sample {
            duration: duration_ticks,
            is_sync: true, // audio samples are always sync
            bytes: payload,
        };
        self.audio_samples.push(sample);
        self.audio_current_pts += duration_ticks as u64;

        // stats
        self.audio_total_bytes += size_bytes as u64;
        self.audio_total_ticks += duration_ticks as u64;
        self.roll_audio_segment(false).await?;
        Ok(())
    }

    pub fn configure_audio_track(
        &mut self,
        sample_rate: u32,
        channels: u16,
        codec: impl Into<String>,
        fmtp: Option<&str>,
    ) {
        if sample_rate > 0 {
            self.audio_sample_rate = sample_rate;
        }

        let mut derived_channels = channels;
        if let Some(fmtp_line) = fmtp
            && let Some(parsed) = parse_channels_from_fmtp(fmtp_line)
        {
            derived_channels = parsed;
        }

        if derived_channels > 0 {
            self.audio_channels = derived_channels;
        }

        let codec_input = codec.into();
        if !codec_input.is_empty() {
            let normalized = codec_input
                .rsplit('/')
                .next()
                .unwrap_or(codec_input.as_str())
                .to_ascii_lowercase();
            self.audio_codec = normalized;
        }
    }

    /// Feed one VP9 frame (already reassembled by RTP parser)
    pub async fn push_vp9(&mut self, frame: Bytes, duration_ticks: u32) -> Result<()> {
        self.push_video_frame(VideoCodec::Vp9, frame, None, duration_ticks)
            .await
    }

    /// Feed one AV1 temporal unit (already reassembled by RTP parser)
    pub async fn push_av1(&mut self, frame: Bytes, duration_ticks: u32) -> Result<()> {
        self.push_video_frame(VideoCodec::Av1, frame, None, duration_ticks)
            .await
    }

    async fn push_video_frame(
        &mut self,
        codec: VideoCodec,
        frame: Bytes,
        explicit_sync: Option<bool>,
        duration_ticks: u32,
    ) -> Result<()> {
        self.ensure_video_adapter(codec);

        let (payload, adapter_sync, config_ready) = {
            let adapter = self
                .video_adapter
                .as_mut()
                .expect("video adapter should be initialized");
            adapter.convert_frame(&frame)
        };

        self.refresh_video_metadata();
        let adapter_ready = self.current_video_adapter_ready();

        let is_sync = explicit_sync.unwrap_or(false) || adapter_sync;

        if is_sync {
            self.pli_backoff.record_keyframe();
            tracing::debug!("[segmenter] {} {:?} keyframe detected", self.stream, codec);
        }
        if config_ready {
            tracing::info!(
                "[segmenter] {} {:?} codec config event detected",
                self.stream,
                codec
            );
        }

        let dur = if duration_ticks == 0 {
            3_000
        } else {
            duration_ticks
        };

        if is_sync && (self.video_current_pts - self.video_seg_start_dts >= self.seg_duration_ticks)
        {
            self.roll_segment().await?;
        }

        if self.video_track_id.is_none() {
            if config_ready || adapter_ready || self.metadata_allows_video_init(codec) {
                self.refresh_video_metadata();
                self.init_writer().await?;
            } else {
                return Ok(());
            }
        }

        let sample_bytes = Bytes::from(payload);
        let sample_len = sample_bytes.len() as u64;

        let sample = Mp4Sample {
            duration: dur,
            is_sync,
            bytes: sample_bytes,
        };
        self.video_samples.push(sample);
        self.video_current_pts += dur as u64;

        self.total_bytes += sample_len;
        self.total_ticks += dur as u64;

        let fps_cur = self.timescale.checked_div(dur).unwrap_or(0);
        if fps_cur > 0 && (self.frame_rate == 0 || fps_cur > self.frame_rate) {
            self.frame_rate = fps_cur;
        }

        self.refresh_video_metadata();

        Ok(())
    }

    fn ensure_video_adapter(&mut self, codec: VideoCodec) {
        let adapter_missing = self.video_adapter.is_none();
        let codec_changed = self
            .video_codec_kind
            .map(|kind| kind != codec)
            .unwrap_or(true);

        if adapter_missing || codec_changed {
            self.reset_video_state();
            self.video_adapter = Some(create_video_adapter(codec));
            self.video_codec_kind = Some(codec);

            if let Some(adapter) = self.video_adapter.as_ref() {
                let timescale = adapter.timescale();
                if timescale > 0 {
                    self.timescale = timescale;
                    self.seg_duration_ticks = timescale as u64 * DEFAULT_SEG_DURATION;
                }
            }
        }
    }

    fn reset_video_state(&mut self) {
        self.video_track_id = None;
        self.fmp4_writer = None;
        self.video_samples.clear();
        self.video_seg_index = 0;
        self.video_seg_start_dts = 0;
        self.video_current_pts = 0;
        self.segments.clear();
        self.total_bytes = 0;
        self.total_ticks = 0;
        self.frame_rate = 0;
        self.video_width = 0;
        self.video_height = 0;
        self.video_codec.clear();
        self.pli_backoff.hard_reset();
    }

    fn refresh_video_metadata(&mut self) {
        if let Some(adapter) = self.video_adapter.as_ref() {
            let adapter = adapter.as_ref();
            let width = adapter.width();
            let height = adapter.height();
            if width > 0 {
                self.video_width = width;
            }
            if height > 0 {
                self.video_height = height;
            }
            if let Some(cs) = adapter.codec_string()
                && self.video_codec != cs
            {
                self.video_codec = cs;
            }
        }
        if self.video_codec.is_empty()
            && let Some(codec_mime) = self
                .expected_video
                .as_ref()
                .map(|video| video.codec_mime.as_str())
        {
            self.video_codec = default_codec_string(codec_mime).to_string();
        }
    }

    fn current_video_adapter_ready(&self) -> bool {
        self.video_adapter
            .as_ref()
            .map(|adapter| adapter.ready())
            .unwrap_or(false)
    }

    fn metadata_allows_video_init(&self, codec: VideoCodec) -> bool {
        match codec {
            VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Av1 => self
                .pending_video_codec_config
                .as_ref()
                .is_some_and(|config| !config.is_empty()),
            VideoCodec::Vp9 => self.video_width > 0 && self.video_height > 0,
        }
    }

    /// Check if we need to request a keyframe due to timeout
    pub fn should_request_keyframe(&self) -> bool {
        self.pli_backoff.should_request()
    }

    /// Record that a PLI request was sent
    pub fn record_pli_request(&mut self) {
        self.pli_backoff.record_request();
    }

    /// Get PLI backoff statistics for logging
    pub fn pli_stats(&self) -> String {
        self.pli_backoff.state_summary()
    }

    pub fn media_outcome(&self) -> RecordingMediaOutcome {
        let has_video = self.video_track_id.is_some() && !self.segments.is_empty();
        let has_audio = self.audio_track_id.is_some() && !self.audio_segments.is_empty();

        if let Some(expected_video) = self.expected_video.as_ref()
            && !has_video
        {
            tracing::warn!(
                "[segmenter] {} expected video output is missing codec={} payload_type={:?} ssrc={:?}",
                self.stream,
                expected_video.codec_mime,
                expected_video.payload_type,
                expected_video.ssrc
            );
            return if has_audio {
                RecordingMediaOutcome::Degraded
            } else {
                RecordingMediaOutcome::Failed
            };
        }

        if has_video || has_audio {
            RecordingMediaOutcome::Complete
        } else {
            RecordingMediaOutcome::Failed
        }
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.roll_segment().await?;
        self.roll_audio_segment(true).await?;
        self.write_manifest(false).await?;
        Ok(())
    }

    async fn init_writer(&mut self) -> Result<()> {
        self.refresh_video_metadata();
        // Get video width/height from adapter (only meaningful for H264 path)
        let (mut width, mut height) = (self.video_width, self.video_height);
        if (width == 0 || height == 0)
            && let Some(adapter) = self.video_adapter.as_ref()
        {
            let w = adapter.width();
            let h = adapter.height();
            if w != 0 && h != 0 {
                width = w;
                height = h;
            }
        }

        // Derive codec string from adapter if not set (H264)
        if self.video_codec.is_empty()
            && let Some(adapter) = self.video_adapter.as_ref()
            && let Some(cs) = adapter.codec_string()
        {
            self.video_codec = cs;
        }

        // Save to member fields for generating the MPD
        self.video_width = if width == 0 { 1280 } else { width };
        self.video_height = if height == 0 { 720 } else { height };

        // Build init segment via our new fMP4 writer (track_id fixed to 1)
        let track_id = VIDEO_TRACK_ID;
        let lower_codec = self.video_codec.to_ascii_lowercase();
        let codec_config = if let Some(adapter) = self.video_adapter.as_ref() {
            if lower_codec.starts_with("avc1")
                || lower_codec.starts_with("av01")
                || lower_codec.starts_with("hev1")
                || lower_codec.starts_with("hvc1")
            {
                adapter
                    .codec_config()
                    .filter(|config| !config.is_empty())
                    .or_else(|| self.pending_video_codec_config.clone())
                    .unwrap_or_default()
            } else {
                vec![]
            }
        } else if lower_codec.starts_with("avc1")
            || lower_codec.starts_with("av01")
            || lower_codec.starts_with("hev1")
            || lower_codec.starts_with("hvc1")
        {
            self.pending_video_codec_config.clone().unwrap_or_default()
        } else {
            vec![]
        };

        let fmp4_writer = Fmp4Writer::new(
            self.timescale,
            track_id,
            self.video_width,
            self.video_height,
            self.video_codec.clone(),
            codec_config,
        );

        let init_bytes = fmp4_writer.build_init_segment();
        self.video_track_id = Some(track_id);
        self.fmp4_writer = Some(fmp4_writer);

        self.store_file(VIDEO_INIT_FILENAME, init_bytes)
            .await
            .map_err(|e| {
                tracing::error!(
                    "[segmenter] failed to store init.m4s for stream {}: {}",
                    self.stream,
                    e
                );
                e
            })?;
        info!("[segmenter] {} v_init.m4s written", self.stream);
        self.pending_video_codec_config = None;

        // Generate or update the MPD manifest
        self.write_manifest(true).await?;

        // Start a new segment: reset timers and caches
        self.open_new_segment().await?;
        Ok(())
    }

    async fn init_audio_writer(&mut self) -> Result<()> {
        let track_id = AUDIO_TRACK_ID;
        let channels = if self.audio_channels == 0 {
            DEFAULT_AUDIO_CHANNELS
        } else {
            self.audio_channels
        };
        let sample_rate = if self.audio_sample_rate == 0 {
            DEFAULT_AUDIO_SAMPLE_RATE
        } else {
            self.audio_sample_rate
        };
        let codec_string = if self.audio_codec.is_empty() {
            DEFAULT_AUDIO_CODEC.to_string()
        } else {
            self.audio_codec.clone()
        };

        let writer = Fmp4Writer::new_audio(
            sample_rate,
            track_id,
            channels,
            sample_rate,
            codec_string.clone(),
            vec![],
        );

        let init_bytes = writer.build_init_segment();
        self.audio_sample_rate = sample_rate;
        self.audio_channels = channels;
        self.audio_codec = codec_string.clone();
        self.store_file(AUDIO_INIT_FILENAME, init_bytes)
            .await
            .map_err(|e| {
                tracing::error!(
                    "[segmenter] failed to store audio_init.m4s for stream {}: {}",
                    self.stream,
                    e
                );
                e
            })?;
        self.audio_track_id = Some(track_id);
        self.audio_writer = Some(writer);
        self.audio_seg_index = 0;
        self.audio_seg_start_pts = self.audio_current_pts;

        tracing::info!("[segmenter] {} audio_init.m4s written", self.stream);
        self.write_manifest(true).await?;
        Ok(())
    }

    async fn open_new_segment(&mut self) -> Result<()> {
        self.video_samples.clear();
        self.video_seg_start_dts = self.video_current_pts;
        self.video_seg_index += 1;
        Ok(())
    }

    async fn roll_segment(&mut self) -> Result<()> {
        // Return immediately if not ready (no samples or tracks haven't been set up)
        if self.video_samples.is_empty() || self.video_track_id.is_none() {
            return Ok(());
        }

        let base_time = self.video_seg_start_dts;
        let segment_end_time = self.video_current_pts;
        let actual_duration = segment_end_time - base_time;

        let writer = self
            .fmp4_writer
            .as_ref()
            .expect("fmp4 writer not initialized");

        let fragment = writer.build_fragment(self.video_seg_index, base_time, &self.video_samples);
        let filename = format!(
            "{prefix}{index:04}{ext}",
            prefix = VIDEO_SEGMENT_FILENAME_PREFIX,
            index = self.video_seg_index,
            ext = SEGMENT_FILE_EXTENSION
        );
        self.store_file(&filename, fragment).await.map_err(|e| {
            tracing::error!(
                "[segmenter] failed to store video segment {} for stream {}: {}",
                filename,
                self.stream,
                e
            );
            e
        })?;
        info!("[segmenter] {} {} written", self.stream, filename);

        // Record the completed segment with its actual duration
        self.segments.push(SegmentInfo {
            start_time: base_time,
            duration: actual_duration,
        });

        // Clear the cache and start the next segment
        self.open_new_segment().await?;

        // Update the MPD manifest
        self.write_manifest(true).await?;
        Ok(())
    }

    async fn roll_audio_segment(&mut self, force: bool) -> Result<()> {
        if self.audio_writer.is_none() || self.audio_samples.is_empty() {
            return Ok(());
        }

        let writer = self
            .audio_writer
            .as_ref()
            .expect("audio writer must exist when rolling audio segments");
        let segment_start = self.audio_seg_start_pts;
        let segment_end = self.audio_current_pts;
        let segment_duration = segment_end.saturating_sub(segment_start);
        let target_duration = writer.timescale as u64 * DEFAULT_SEG_DURATION;

        if !force && segment_duration < target_duration {
            return Ok(());
        }

        self.audio_seg_index += 1;
        let current_index = self.audio_seg_index;

        let fragment = writer.build_fragment(current_index, segment_start, &self.audio_samples);
        let filename = format!(
            "{prefix}{index:04}{ext}",
            prefix = AUDIO_SEGMENT_FILENAME_PREFIX,
            index = current_index,
            ext = SEGMENT_FILE_EXTENSION
        );
        self.store_file(&filename, fragment).await.map_err(|e| {
            tracing::error!(
                "[segmenter] failed to store audio segment {} for stream {}: {}",
                filename,
                self.stream,
                e
            );
            e
        })?;
        info!("[segmenter] {} {} written", self.stream, filename);

        self.audio_segments.push(SegmentInfo {
            start_time: segment_start,
            duration: segment_duration,
        });

        self.audio_samples.clear();
        self.audio_seg_start_pts = self.audio_current_pts;

        self.write_manifest(true).await?;
        Ok(())
    }

    async fn write_manifest(&self, is_active: bool) -> Result<()> {
        let video_track_ready = self.video_track_id.is_some();
        let audio_track_ready = self.audio_writer.is_some();

        if !video_track_ready && !audio_track_ready {
            return Ok(());
        }

        let has_video_segments = video_track_ready && !self.segments.is_empty();
        let has_audio_segments = audio_track_ready && !self.audio_segments.is_empty();

        let mut media_duration_secs = 0f64;
        let mut max_segment_duration_secs = 0f64;

        if has_video_segments {
            if let Some(last) = self.segments.last() {
                let end_ticks = last.start_time + last.duration;
                media_duration_secs =
                    media_duration_secs.max(end_ticks as f64 / self.timescale as f64);
            }
            if let Some(max_video_dur) = self.segments.iter().map(|s| s.duration).max() {
                max_segment_duration_secs =
                    max_segment_duration_secs.max(max_video_dur as f64 / self.timescale as f64);
            }
        }

        if has_audio_segments {
            let writer = self
                .audio_writer
                .as_ref()
                .expect("audio writer missing while writing manifest");
            if let Some(last) = self.audio_segments.last() {
                let end_ticks = last.start_time + last.duration;
                media_duration_secs =
                    media_duration_secs.max(end_ticks as f64 / writer.timescale as f64);
            }
            if let Some(max_audio_dur) = self.audio_segments.iter().map(|s| s.duration).max() {
                max_segment_duration_secs =
                    max_segment_duration_secs.max(max_audio_dur as f64 / writer.timescale as f64);
            }
        }

        // Fallback values when only one adaptation is present to avoid zero durations.
        if max_segment_duration_secs == 0.0 {
            max_segment_duration_secs = DEFAULT_SEG_DURATION as f64;
        }
        if media_duration_secs == 0.0 {
            media_duration_secs = max_segment_duration_secs;
        }

        let max_segment_duration = format!("PT{max_segment_duration_secs:.3}S");
        let min_buffer_time = if max_segment_duration_secs * 3.0 > 0.0 {
            format!("PT{:.3}S", max_segment_duration_secs * 3.0)
        } else {
            "PT1S".to_string()
        };
        let publish_time = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let availability_start_time = self
            .manifest_start_time
            .to_rfc3339_opts(SecondsFormat::Millis, true);

        let mut adaptation_sets = String::new();

        if video_track_ready {
            let video_bandwidth = if self.total_ticks > 0 {
                self.total_bytes
                    .saturating_mul(8)
                    .saturating_mul(self.timescale as u64)
                    / self.total_ticks.max(1)
            } else {
                0
            };

            let video_segment_timeline = self.generate_segment_timeline(&self.segments);

            let fps_val = if self.frame_rate > 0 {
                self.frame_rate
            } else {
                30
            };

            let par_str = if self.video_width > 0 && self.video_height > 0 {
                let mut w = self.video_width;
                let mut h = self.video_height;
                while h != 0 {
                    let tmp = h;
                    h = w % h;
                    w = tmp;
                }
                match (
                    self.video_width.checked_div(w),
                    self.video_height.checked_div(w),
                ) {
                    (Some(video_width), Some(video_height)) => {
                        format!("{video_width}:{video_height}")
                    }
                    _ => "1:1".to_string(),
                }
            } else {
                "1:1".to_string()
            };

            let video_section = format!(
                "        <AdaptationSet id=\"0\" contentType=\"video\" startWithSAP=\"1\" segmentAlignment=\"true\" bitstreamSwitching=\"true\" frameRate=\"{fps}/1\" maxWidth=\"{width}\" maxHeight=\"{height}\" par=\"{par}\">\n            <Representation id=\"0\" mimeType=\"video/mp4\" codecs=\"{codec}\" bandwidth=\"{bandwidth}\" width=\"{width}\" height=\"{height}\" sar=\"1:1\">\n                <SegmentTemplate timescale=\"{timescale}\" initialization=\"{video_init}\" media=\"{video_media}\" startNumber=\"1\">\n{video_timeline}\n                </SegmentTemplate>\n            </Representation>\n        </AdaptationSet>\n",
                fps = fps_val,
                width = self.video_width,
                height = self.video_height,
                par = par_str,
                codec = self.video_codec,
                bandwidth = video_bandwidth,
                timescale = self.timescale,
                video_init = VIDEO_INIT_FILENAME,
                video_media = VIDEO_SEGMENT_TEMPLATE,
                video_timeline = video_segment_timeline,
            );
            adaptation_sets.push_str(&video_section);
        }

        if audio_track_ready {
            let writer = self.audio_writer.as_ref().unwrap();
            let audio_bandwidth = if self.audio_total_ticks > 0 {
                self.audio_total_bytes
                    .saturating_mul(8)
                    .saturating_mul(writer.timescale as u64)
                    / self.audio_total_ticks.max(1)
            } else {
                0
            };
            let audio_segment_timeline = self.generate_segment_timeline(&self.audio_segments);
            let audio_adaptation_id = if video_track_ready { 1 } else { 0 };
            let audio_representation_id = if video_track_ready { 1 } else { 0 };

            let audio_section = format!(
                "        <AdaptationSet id=\"{adapt_id}\" contentType=\"audio\" segmentAlignment=\"true\">\n            <Representation id=\"{rep_id}\" mimeType=\"audio/mp4\" codecs=\"{codec}\" bandwidth=\"{bandwidth}\" audioSamplingRate=\"{sample_rate}\" >\n                <SegmentTemplate timescale=\"{timescale}\" initialization=\"{audio_init}\" media=\"{audio_media}\" startNumber=\"1\">\n{audio_timeline}\n                </SegmentTemplate>\n            </Representation>\n        </AdaptationSet>\n",
                adapt_id = audio_adaptation_id,
                rep_id = audio_representation_id,
                codec = writer.codec_string,
                bandwidth = audio_bandwidth,
                sample_rate = writer.sample_rate,
                timescale = writer.timescale,
                audio_init = AUDIO_INIT_FILENAME,
                audio_media = AUDIO_SEGMENT_TEMPLATE,
                audio_timeline = audio_segment_timeline,
            );
            adaptation_sets.push_str(&audio_section);
        }

        let mpd_body = if is_active {
            format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<MPD xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n\
     xmlns=\"urn:mpeg:dash:schema:mpd:2011\"\n\
     xmlns:xlink=\"http://www.w3.org/1999/xlink\"\n\
     xsi:schemaLocation=\"urn:mpeg:DASH:schema:MPD:2011 http://standards.iso.org/ittf/PubliclyAvailableStandards/MPEG-DASH_schema_files/DASH-MPD.xsd\"\n\
     profiles=\"urn:mpeg:dash:profile:isoff-live:2011\"\n\
     type=\"dynamic\"\n\
     availabilityStartTime=\"{availability_start_time}\"\n\
     publishTime=\"{publish_time}\"\n\
     minimumUpdatePeriod=\"PT{minimum_update_period}S\"\n\
     maxSegmentDuration=\"{max_seg_dur}\"\n\
     minBufferTime=\"{min_buf}\">\n\
    <ProgramInformation/>\n    <ServiceDescription id=\"0\"/>\n    <Period id=\"0\" start=\"PT0.0S\">\n{adapt_sets}    </Period>\n</MPD>\n",
                availability_start_time = availability_start_time,
                publish_time = publish_time,
                minimum_update_period = MANIFEST_UPDATE_PERIOD_SECS,
                max_seg_dur = max_segment_duration,
                min_buf = min_buffer_time,
                adapt_sets = adaptation_sets,
            )
        } else {
            let media_presentation_duration = format!("PT{media_duration_secs:.3}S");
            format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<MPD xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n\
     xmlns=\"urn:mpeg:dash:schema:mpd:2011\"\n\
     xmlns:xlink=\"http://www.w3.org/1999/xlink\"\n\
     xsi:schemaLocation=\"urn:mpeg:DASH:schema:MPD:2011 http://standards.iso.org/ittf/PubliclyAvailableStandards/MPEG-DASH_schema_files/DASH-MPD.xsd\"\n\
     profiles=\"urn:mpeg:dash:profile:isoff-live:2011\"\n\
     type=\"static\"\n\
     mediaPresentationDuration=\"{media_duration}\"\n\
     maxSegmentDuration=\"{max_seg_dur}\"\n\
     minBufferTime=\"{min_buf}\">\n\
    <ProgramInformation/>\n    <ServiceDescription id=\"0\"/>\n    <Period id=\"0\" start=\"PT0.0S\">\n{adapt_sets}    </Period>\n</MPD>\n",
                media_duration = media_presentation_duration,
                max_seg_dur = max_segment_duration,
                min_buf = min_buffer_time,
                adapt_sets = adaptation_sets,
            )
        };

        self.store_file(MANIFEST_FILENAME, mpd_body.into_bytes())
            .await
            .map_err(|e| {
                tracing::error!(
                    "[segmenter] failed to store manifest.mpd for stream {}: {}",
                    self.stream,
                    e
                );
                e
            })
    }

    /// Generate SegmentTimeline XML from segment info
    fn generate_segment_timeline(&self, segments: &[SegmentInfo]) -> String {
        if segments.is_empty() {
            return "                    <SegmentTimeline></SegmentTimeline>".to_string();
        }

        let mut timeline = String::from("                    <SegmentTimeline>\n");

        // Simply output each segment without grouping for now to avoid timeline gaps
        // DASH players are very sensitive to timeline accuracy
        for segment in segments {
            timeline.push_str(&format!(
                "                        <S t=\"{}\" d=\"{}\" />\n",
                segment.start_time, segment.duration
            ));
        }

        timeline.push_str("                    </SegmentTimeline>");
        timeline
    }

    async fn store_file(&self, name: &str, data: Vec<u8>) -> Result<()> {
        let path = format!("{}/{}", self.path_prefix, name);
        let data_size = data.len();

        tracing::debug!(
            "[segmenter] storing file {} ({} bytes) for stream {}",
            path,
            data_size,
            self.stream
        );

        self.store_file_now(path, data).await
    }

    async fn store_file_now(&self, path: String, data: Vec<u8>) -> Result<()> {
        if let Some(uploader) = self.uploader.as_ref()
            && let Some(local_dir) = self.local_dir.as_ref()
        {
            let local_path = local_dir.join(&path);
            if let Some(parent) = local_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&local_path, data).await?;
            uploader
                .enqueue(path, local_path.to_string_lossy().to_string())
                .await?;
        } else {
            self.op.write(&path, data).await?;
        }

        Ok(())
    }
}

fn video_codec_from_mime(codec_mime: &str) -> Option<VideoCodec> {
    let codec = codec_mime.to_ascii_lowercase();
    if codec == "video/h264" {
        Some(VideoCodec::H264)
    } else if codec == "video/h265" || codec == "video/hevc" {
        Some(VideoCodec::H265)
    } else if codec == "video/vp9" {
        Some(VideoCodec::Vp9)
    } else if codec == "video/av1" {
        Some(VideoCodec::Av1)
    } else {
        None
    }
}

fn default_codec_string(codec_mime: &str) -> &'static str {
    match codec_mime.to_ascii_lowercase().as_str() {
        "video/h264" => "avc1",
        "video/h265" | "video/hevc" => "hev1",
        "video/vp9" => "vp09.00.10.08.01.02.02.02.00",
        "video/av1" => "av01.0.08M.08",
        _ => "avc1",
    }
}

fn parse_channels_from_fmtp(fmtp: &str) -> Option<u16> {
    fmtp.split(';').map(str::trim).find_map(|part| {
        if let Some(value) = part.strip_prefix("channels=") {
            return value.trim().parse::<u16>().ok().filter(|v| *v > 0);
        }
        if let Some(value) = part.strip_prefix("stereo=") {
            return match value.trim() {
                "1" => Some(2),
                "0" => Some(1),
                _ => None,
            };
        }
        if let Some(value) = part.strip_prefix("sprop-stereo=") {
            return match value.trim() {
                "1" => Some(2),
                "0" => Some(1),
                _ => None,
            };
        }
        None
    })
}
