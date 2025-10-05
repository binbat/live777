use std::time::{Duration, Instant};

use crate::recorder::codec::CodecAdapter;
use crate::recorder::codec::h264::H264Adapter;
use crate::recorder::fmp4::{Fmp4Writer, Mp4Sample};
use anyhow::Result;
use bytes::Bytes;
use opendal::Operator;
use tracing::info;

/// Default duration of each segment in seconds
const DEFAULT_SEG_DURATION: u64 = 10;

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

    // audio bitrate stats
    audio_total_bytes: u64,
    audio_total_ticks: u64,

    // keyframe request tracking
    last_keyframe_time: Option<Instant>,
    keyframe_request_timeout: Duration,

    // video adapter for H264 (unused for VP8/VP9)
    video_adapter: Option<H264Adapter>,

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

            audio_total_bytes: 0,
            audio_total_ticks: 0,

            last_keyframe_time: None,
            keyframe_request_timeout: Duration::from_secs(10),

            video_adapter: Some(H264Adapter::new()),
            segments: Vec::new(),
            audio_segments: Vec::new(),
        })
    }

    /// Feed one H.264 Frame (Annex-B format, may contain multiple NALUs)
    /// `duration_ticks` – frame duration in the same timescale as self.timescale (90000 for H264)
    pub async fn push_h264(
        &mut self,
        frame: Bytes,
        mut is_idr: bool,
        duration_ticks: u32,
    ) -> Result<()> {
        // ------- Split frame content into multiple NALUs -------
        let (avcc_payload, detected_idr, config_ready) =
            if let Some(adapter) = self.video_adapter.as_mut() {
                adapter.convert_frame(&frame)
            } else {
                // Fallback to legacy path (should not happen)
                (frame.as_ref().to_vec(), false, false)
            };

        if detected_idr {
            is_idr = true;
        }

        // Use provided duration, fallback to 3000 ticks if it looks invalid (e.g. 0)
        let dur = if duration_ticks == 0 {
            3_000
        } else {
            duration_ticks
        };

        // Update keyframe tracking
        if is_idr {
            self.last_keyframe_time = Some(Instant::now());
        }

        // -------- Segment boundary check *before* enqueuing the new sample --------
        // We want the very first IDR *after* reaching the nominal segment length to
        // start the next segment. Therefore, if this frame is an IDR **and** the
        // accumulated duration of the *current* segment has already reached the
        // target length, we should finish the current segment _before_ adding the
        // sample.
        if is_idr && (self.video_current_pts - self.video_seg_start_dts >= self.seg_duration_ticks)
        {
            self.roll_segment().await?;
        }

        // After a possible roll, `self.video_current_pts` and `video_seg_start_dts` are intact
        // for the (potentially) new segment, so we can safely add the sample.
        let sample = Mp4Sample {
            start_time: self.video_current_pts,
            duration: dur,
            is_sync: is_idr,
            bytes: Bytes::from(avcc_payload),
        };
        self.video_samples.push(sample);
        self.video_current_pts += dur as u64;

        // Update dynamic statistics
        self.total_bytes += frame.len() as u64;
        self.total_ticks += dur as u64;

        // Dynamically update nominal FPS: prefer the highest observed value
        let fps_cur = self.timescale / dur;
        if self.frame_rate == 0 || fps_cur > self.frame_rate {
            self.frame_rate = fps_cur;
        }

        // Now that we (likely) have frame rate, generate init segment if not yet written
        if self.video_track_id.is_none() && config_ready {
            if let Some(adapter) = self.video_adapter.as_ref()
                && let Some(cfg) = adapter.codec_config()
            {
                if !cfg.is_empty() {
                    // self.sps = Some(cfg[0].clone());
                }
                if cfg.len() >= 2 {
                    // self.pps = Some(cfg[1].clone());
                }
                // Update codec string for manifest
                if self.video_codec.is_empty()
                    && let Some(cs) = adapter.codec_string()
                {
                    self.video_codec = cs;
                }
            }

            self.init_writer().await?;
            // init_writer will call open_new_segment(), clear samples, so ensure it's called before samples are enqueued
        }

        Ok(())
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
            start_time: sample_start,
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

    /// Feed one VP8 frame (already reassembled by RTP parser)
    pub async fn push_vp8(
        &mut self,
        frame: Bytes,
        is_keyframe: bool,
        duration_ticks: u32,
    ) -> Result<()> {
        // Update keyframe tracking
        if is_keyframe {
            self.last_keyframe_time = Some(Instant::now());
        }
        // For VP8, initialize only on first keyframe to capture correct dimensions
        if self.video_track_id.is_none() {
            if !is_keyframe {
                return Ok(());
            }
            if let Some((w, h)) = parse_vp8_keyframe_dimensions(frame.as_ref())
                && w > 0
                && h > 0
            {
                self.video_width = w;
                self.video_height = h;
            }
            // if dimensions still unknown, keep waiting (requesting PLI) instead of defaulting to 1280x720
            if self.video_width == 0 || self.video_height == 0 {
                return Ok(());
            }
        }
        // ensure codec string, init writer lazily
        if self.video_track_id.is_none() {
            if self.video_codec.is_empty() {
                self.video_codec = "vp08.00.10.08".to_string();
            }
            self.init_writer().await?;
        }

        let dur = if duration_ticks == 0 {
            3_000
        } else {
            duration_ticks
        };

        if is_keyframe
            && (self.video_current_pts - self.video_seg_start_dts >= self.seg_duration_ticks)
        {
            self.roll_segment().await?;
        }

        let sample = Mp4Sample {
            start_time: self.video_current_pts,
            duration: dur,
            is_sync: is_keyframe,
            bytes: frame,
        };
        self.video_samples.push(sample);
        self.video_current_pts += dur as u64;
        self.total_bytes += self
            .video_samples
            .last()
            .map(|s| s.bytes.len() as u64)
            .unwrap_or(0);
        self.total_ticks += dur as u64;

        // update fps heuristic
        let fps_cur = self.timescale / dur;
        if self.frame_rate == 0 || fps_cur > self.frame_rate {
            self.frame_rate = fps_cur;
        }
        Ok(())
    }

    /// Feed one VP9 frame (already reassembled by RTP parser)
    pub async fn push_vp9(
        &mut self,
        frame: Bytes,
        is_keyframe: bool,
        duration_ticks: u32,
    ) -> Result<()> {
        // Update keyframe tracking
        if is_keyframe {
            self.last_keyframe_time = Some(Instant::now());
        }
        // Initialize only on first keyframe to capture correct dimensions
        if self.video_track_id.is_none() {
            if !is_keyframe {
                return Ok(());
            }
            if let Some((w, h)) = parse_vp9_keyframe_dimensions(frame.as_ref())
                && w > 0
                && h > 0
            {
                self.video_width = w;
                self.video_height = h;
            }
            if self.video_width == 0 || self.video_height == 0 {
                return Ok(());
            }
            if self.video_codec.is_empty() {
                self.video_codec = "vp09.00.10.08".to_string();
            }
            self.init_writer().await?;
        }

        let dur = if duration_ticks == 0 {
            3_000
        } else {
            duration_ticks
        };

        if is_keyframe
            && (self.video_current_pts - self.video_seg_start_dts >= self.seg_duration_ticks)
        {
            self.roll_segment().await?;
        }

        let size = frame.len();
        let sample = Mp4Sample {
            start_time: self.video_current_pts,
            duration: dur,
            is_sync: is_keyframe,
            bytes: frame,
        };
        self.video_samples.push(sample);
        self.video_current_pts += dur as u64;
        self.total_bytes += size as u64;
        self.total_ticks += dur as u64;
        let fps_cur = self.timescale / dur;
        if self.frame_rate == 0 || fps_cur > self.frame_rate {
            self.frame_rate = fps_cur;
        }
        Ok(())
    }

    /// Check if we need to request a keyframe due to timeout
    pub fn should_request_keyframe(&self) -> bool {
        match self.last_keyframe_time {
            None => true, // No keyframe received yet, request one
            Some(last_time) => last_time.elapsed() >= self.keyframe_request_timeout,
        }
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.roll_segment().await?;
        self.roll_audio_segment(true).await?;
        Ok(())
    }

    async fn init_writer(&mut self) -> Result<()> {
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
        let track_id = 1u32;
        let codec_config = if self.video_codec.to_ascii_lowercase().starts_with("avc1") {
            if let Some(adapter) = self.video_adapter.as_ref() {
                adapter.codec_config().unwrap_or_default()
            } else {
                vec![]
            }
        } else {
            // VP8/VP9 do not require codec private data here; vpcC in sample entry is enough
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

        self.store_file("init.m4s", init_bytes).await.map_err(|e| {
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
        let track_id = 2u32;
        let channels = 2u16;
        let sample_rate = 48_000u32;
        let codec_string = "opus".to_string();

        let writer = Fmp4Writer::new_audio(
            sample_rate,
            track_id,
            channels,
            sample_rate,
            codec_string.clone(),
            vec![],
        );

        let init_bytes = writer.build_init_segment();
        self.store_file("audio_init.m4s", init_bytes)
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
        let filename = format!("seg_{:04}.m4s", self.video_seg_index);
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
        let filename = format!("audio_seg_{:04}.m4s", current_index);
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
        let has_video_segments = self.video_track_id.is_some() && !self.segments.is_empty();
        let has_audio_segments = self.audio_writer.is_some() && !self.audio_segments.is_empty();

        if !has_video_segments && !has_audio_segments {
            return Ok(());
        }

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

        if has_video_segments {
            let video_bandwidth = if self.total_ticks > 0 {
                self.total_bytes * 8 * self.timescale as u64 / self.total_ticks
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
                "        <AdaptationSet id=\"0\" contentType=\"video\" startWithSAP=\"1\" segmentAlignment=\"true\" bitstreamSwitching=\"true\" frameRate=\"{fps}/1\" maxWidth=\"{width}\" maxHeight=\"{height}\" par=\"{par}\">\n            <Representation id=\"0\" mimeType=\"video/mp4\" codecs=\"{codec}\" bandwidth=\"{bandwidth}\" width=\"{width}\" height=\"{height}\" sar=\"1:1\">\n                <SegmentTemplate timescale=\"{timescale}\" initialization=\"init.m4s\" media=\"seg_$Number%04d$.m4s\" startNumber=\"1\">\n{video_timeline}\n                </SegmentTemplate>\n            </Representation>\n        </AdaptationSet>\n",
                fps = fps_val,
                width = self.video_width,
                height = self.video_height,
                par = par_str,
                codec = self.video_codec,
                bandwidth = video_bandwidth,
                timescale = self.timescale,
                video_timeline = video_segment_timeline,
            );
            adaptation_sets.push_str(&video_section);
        }

        if has_audio_segments {
            let writer = self.audio_writer.as_ref().unwrap();
            let audio_bandwidth = if self.audio_total_ticks > 0 {
                self.audio_total_bytes * 8 * writer.timescale as u64 / self.audio_total_ticks
            } else {
                0
            };
            let audio_segment_timeline = self.generate_segment_timeline(&self.audio_segments);
            let audio_adaptation_id = if has_video_segments { 1 } else { 0 };
            let audio_representation_id = if has_video_segments { 1 } else { 0 };

            let audio_section = format!(
                "        <AdaptationSet id=\"{adapt_id}\" contentType=\"audio\" segmentAlignment=\"true\">\n            <Representation id=\"{rep_id}\" mimeType=\"audio/mp4\" codecs=\"{codec}\" bandwidth=\"{bandwidth}\" audioSamplingRate=\"{sample_rate}\" >\n                <SegmentTemplate timescale=\"{timescale}\" initialization=\"audio_init.m4s\" media=\"audio_seg_$Number%04d$.m4s\" startNumber=\"1\">\n{audio_timeline}\n                </SegmentTemplate>\n            </Representation>\n        </AdaptationSet>\n",
                adapt_id = audio_adaptation_id,
                rep_id = audio_representation_id,
                codec = writer.codec_string,
                bandwidth = audio_bandwidth,
                sample_rate = writer.sample_rate,
                timescale = writer.timescale,
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

        self.store_file("manifest.mpd", mpd_body.into_bytes())
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

// Add at bottom: helper to parse VP8 keyframe dimensions
fn parse_vp8_keyframe_dimensions(frame: &[u8]) -> Option<(u32, u32)> {
    // VP8 keyframe starts with uncompressed data chunk header:
    // Start code bytes 0x9D 0x01 0x2A followed by 2 bytes width, 2 bytes height (little-endian),
    // with 14-bit values and 2-bit scaling fields (ignored here).
    // We search within the first 64 bytes for start code to be safe against payload descriptor.
    let search_len = frame.len().min(64);
    let hay = &frame[..search_len];
    for i in 0..hay.len().saturating_sub(3) {
        if hay[i] == 0x9D && hay[i + 1] == 0x01 && hay[i + 2] == 0x2A {
            if i + 7 < hay.len() {
                let w_raw = u16::from_le_bytes([hay[i + 3], hay[i + 4]]);
                let h_raw = u16::from_le_bytes([hay[i + 5], hay[i + 6]]);
                let width = (w_raw & 0x3FFF) as u32; // lower 14 bits
                let height = (h_raw & 0x3FFF) as u32;
                if width > 0 && height > 0 {
                    return Some((width, height));
                }
            }
            break;
        }
    }
    None
}

// Add VP9 helper below VP8 helper
fn parse_vp9_keyframe_dimensions(frame: &[u8]) -> Option<(u32, u32)> {
    // VP9 keyframe contains sync code 0x49 0x83 0x42, followed by
    // little-endian width_minus_1 and height_minus_1 (16-bit each).
    let search_len = frame.len().min(128);
    let hay = &frame[..search_len];
    for i in 0..hay.len().saturating_sub(7) {
        if hay[i] == 0x49 && hay[i + 1] == 0x83 && hay[i + 2] == 0x42 {
            let w1 = u16::from_le_bytes([hay[i + 3], hay[i + 4]]) as u32;
            let h1 = u16::from_le_bytes([hay[i + 5], hay[i + 6]]) as u32;
            let w = w1 + 1;
            let h = h1 + 1;
            if w > 0 && h > 0 && w <= 8192 && h <= 8192 {
                return Some((w, h));
            }
            break;
        }
    }
    None
}
