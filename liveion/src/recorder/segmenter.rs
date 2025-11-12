use crate::recorder::codec::{CodecAdapter, VideoCodec, create_video_adapter};
use crate::recorder::fmp4::{Fmp4Writer, Mp4Sample};
use crate::recorder::pli_backoff::PliBackoff;
use anyhow::Result;
use bytes::Bytes;
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

/// Represents a completed segment with its actual duration
#[derive(Debug, Clone)]
struct SegmentInfo {
    start_time: u64, // Start time in timescale units
    duration: u64,   // Actual duration in timescale units
}

pub struct Segmenter {
    op: Operator,
    stream: String,
    path_prefix: String,
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
}

impl Segmenter {
    pub async fn new(op: Operator, stream: String, root_prefix: String) -> Result<Self> {
        Ok(Self {
            op,
            stream: stream.clone(),
            path_prefix: root_prefix,
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
        })
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
            if config_ready || adapter_ready {
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

        let fps_cur = if dur > 0 { self.timescale / dur } else { 0 };
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
    }

    fn current_video_adapter_ready(&self) -> bool {
        self.video_adapter
            .as_ref()
            .map(|adapter| adapter.ready())
            .unwrap_or(false)
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

    pub async fn flush(&mut self) -> Result<()> {
        self.roll_segment().await?;
        self.roll_audio_segment(true).await?;
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
                adapter.codec_config().unwrap_or_default()
            } else {
                vec![]
            }
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
        info!("[segmenter] {} init.m4s written", self.stream);

        // Generate or update the MPD manifest
        self.write_manifest().await?;

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
        self.write_manifest().await?;
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
        self.write_manifest().await?;
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

        self.write_manifest().await?;
        Ok(())
    }

    async fn write_manifest(&self) -> Result<()> {
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

        let media_presentation_duration = format!("PT{media_duration_secs:.3}S");
        let max_segment_duration = format!("PT{max_segment_duration_secs:.3}S");
        let min_buffer_time = if max_segment_duration_secs * 3.0 > 0.0 {
            format!("PT{:.3}S", max_segment_duration_secs * 3.0)
        } else {
            "PT1S".to_string()
        };

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
                if w == 0 {
                    "1:1".to_string()
                } else {
                    format!("{}:{}", self.video_width / w, self.video_height / w)
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

        let mpd_body = format!(
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
        );

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

        // Clone what we need for the background task.
        let op_clone = self.op.clone();
        let stream_clone = self.stream.clone();
        let path_clone = path.clone();

        // Spawn the actual write in a detached task so that slow/object‐storage latency does
        // not block the real‐time RTP processing loop. Any error will be logged.
        tokio::spawn(async move {
            if let Err(e) = op_clone.write(&path_clone, data).await {
                tracing::warn!(
                    "[segmenter] failed to write file {} (stream {}): {}",
                    path_clone,
                    stream_clone,
                    e
                );
            } else {
                tracing::debug!(
                    "[segmenter] successfully stored file {} for stream {}",
                    path_clone,
                    stream_clone
                );
            }
        });

        // Return immediately. The caller does not need to wait for persistence; worst-case we
        // lose one fragment, which is acceptable for live streaming.
        Ok(())
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
