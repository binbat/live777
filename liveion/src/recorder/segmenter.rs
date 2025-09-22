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
    seg_index: u32,
    // Decode timestamp of current segment start (in timescale units)
    seg_start_dts: u64,
    video_track_id: Option<u32>,

    // Audio track id (Opus)
    audio_track_id: Option<u32>,

    // All samples buffered for the current segment (already converted to AVCC length prefix)
    samples: Vec<Mp4Sample>,

    // audio samples buffered for current segment
    audio_samples: Vec<Mp4Sample>,

    // video info
    video_width: u32,
    video_height: u32,

    // codec string like "avc1.42E01E"
    video_codec: String,

    current_pts: u64,

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
            seg_index: 0,
            seg_start_dts: 0,
            video_track_id: None,

            audio_track_id: None,

            samples: Vec::new(),

            audio_samples: Vec::new(),

            video_width: 0,
            video_height: 0,
            video_codec: String::new(),
            current_pts: 0,
            frame_rate: 0,
            total_bytes: 0,
            total_ticks: 0,
            fmp4_writer: None,

            audio_writer: None,
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
        if is_idr && (self.current_pts - self.seg_start_dts >= self.seg_duration_ticks) {
            self.roll_segment().await?;
        }

        // After a possible roll, `self.current_pts` and `seg_start_dts` are intact
        // for the (potentially) new segment, so we can safely add the sample.
        let sample = Mp4Sample {
            start_time: self.current_pts,
            duration: dur,
            is_sync: is_idr,
            bytes: Bytes::from(avcc_payload),
        };
        self.samples.push(sample);
        self.current_pts += dur as u64;

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
        let sample = Mp4Sample {
            start_time: self.audio_current_pts,
            duration: duration_ticks,
            is_sync: true, // audio samples are always sync
            bytes: payload,
        };
        self.audio_samples.push(sample);
        self.audio_current_pts += duration_ticks as u64;

        // stats
        self.audio_total_bytes += size_bytes as u64;
        self.audio_total_ticks += duration_ticks as u64;
        Ok(())
    }

    /// Feed one VP8 frame (already reassembled by RTP parser)
    pub async fn push_vp8(
        &mut self,
        frame: Bytes,
        is_keyframe: bool,
        duration_ticks: u32,
    ) -> Result<()> {
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

        if is_keyframe && (self.current_pts - self.seg_start_dts >= self.seg_duration_ticks) {
            self.roll_segment().await?;
        }

        let sample = Mp4Sample {
            start_time: self.current_pts,
            duration: dur,
            is_sync: is_keyframe,
            bytes: frame,
        };
        self.samples.push(sample);
        self.current_pts += dur as u64;
        self.total_bytes += self
            .samples
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
        if self.video_track_id.is_none() {
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

        if is_keyframe && (self.current_pts - self.seg_start_dts >= self.seg_duration_ticks) {
            self.roll_segment().await?;
        }

        let size = frame.len();
        let sample = Mp4Sample {
            start_time: self.current_pts,
            duration: dur,
            is_sync: is_keyframe,
            bytes: frame,
        };
        self.samples.push(sample);
        self.current_pts += dur as u64;
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

    async fn init_writer(&mut self) -> Result<()> {
        // Get video width/height from adapter (only meaningful for H264 path)
        let (mut width, mut height) = (self.video_width, self.video_height);
        if width == 0 || height == 0 {
            if let Some(adapter) = self.video_adapter.as_ref() {
                let w = adapter.width();
                let h = adapter.height();
                if w != 0 && h != 0 {
                    width = w;
                    height = h;
                }
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
            width,
            height,
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

        tracing::info!("[segmenter] {} audio_init.m4s written", self.stream);
        Ok(())
    }

    async fn open_new_segment(&mut self) -> Result<()> {
        self.samples.clear();
        self.audio_samples.clear();
        self.seg_start_dts = self.current_pts;
        self.seg_index += 1;
        Ok(())
    }

    async fn roll_segment(&mut self) -> Result<()> {
        // Return immediately if not ready (no samples or tracks haven't been set up)
        if self.samples.is_empty() || self.video_track_id.is_none() {
            return Ok(());
        }

        let base_time = self.seg_start_dts;
        let segment_end_time = self.current_pts;
        let actual_duration = segment_end_time - base_time;

        let writer = self
            .fmp4_writer
            .as_ref()
            .expect("fmp4 writer not initialized");

        let fragment = writer.build_fragment(self.seg_index, base_time, &self.samples);
        let filename = format!("seg_{:04}.m4s", self.seg_index);
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

        // Write audio fragment if any samples are available
        if let (Some(writer), true) = (self.audio_writer.as_ref(), !self.audio_samples.is_empty()) {
            // Use the decode timestamp of the *first* audio sample in this fragment
            // for the tfdt base time. Using `audio_current_pts` (which points to the
            // *end* of the last pushed sample) caused a time‐shift in the generated
            // fragments leading to playback errors once an audio track was present.
            let audio_base_time = self
                .audio_samples
                .first()
                .map(|s| s.start_time)
                .unwrap_or(self.audio_current_pts);

            let fragment_a =
                writer.build_fragment(self.seg_index, audio_base_time, &self.audio_samples);
            let filename_a = format!("audio_seg_{:04}.m4s", self.seg_index);
            self.store_file(&filename_a, fragment_a)
                .await
                .map_err(|e| {
                    tracing::error!(
                        "[segmenter] failed to store audio segment {} for stream {}: {}",
                        filename_a,
                        self.stream,
                        e
                    );
                    e
                })?;

            // Record the audio segment - calculate duration based on audio samples
            let audio_duration = if let (Some(first), Some(last)) =
                (self.audio_samples.first(), self.audio_samples.last())
            {
                last.start_time + last.duration as u64 - first.start_time
            } else {
                0
            };

            self.audio_segments.push(SegmentInfo {
                start_time: audio_base_time,
                duration: audio_duration,
            });

            self.audio_samples.clear();
        }

        // Clear the cache and start the next segment
        self.open_new_segment().await?;

        // Update the MPD manifest
        self.write_manifest().await?;
        Ok(())
    }

    async fn write_manifest(&self) -> Result<()> {
        // Calculate total duration from actual segments
        let total_duration_ticks: u64 = self.segments.iter().map(|s| s.duration).sum();
        let total_duration_secs = total_duration_ticks as f64 / self.timescale as f64;
        let media_presentation_duration = format!("PT{total_duration_secs:.3}S");

        // Use the maximum actual segment duration for maxSegmentDuration
        let max_actual_duration = self
            .segments
            .iter()
            .map(|s| s.duration)
            .max()
            .unwrap_or(self.seg_duration_ticks);
        let max_segment_duration_secs = max_actual_duration as f64 / self.timescale as f64;
        let max_segment_duration = format!("PT{max_segment_duration_secs:.3}S");

        let min_buffer_time = if max_segment_duration_secs * 3.0 > 0.0 {
            format!("PT{:.3}S", max_segment_duration_secs * 3.0)
        } else {
            "PT1S".to_string()
        };

        // Estimate average video bitrate (bits per second)
        let video_bandwidth = if self.total_ticks > 0 {
            self.total_bytes * 8 * self.timescale as u64 / self.total_ticks
        } else {
            0
        };

        // Generate SegmentTimeline for video
        let video_segment_timeline = self.generate_segment_timeline(&self.segments);

        // Audio params if available
        let audio_section = if let Some(ref a_writer) = self.audio_writer {
            let audio_bandwidth = if self.audio_total_ticks > 0 {
                self.audio_total_bytes * 8 * a_writer.timescale as u64 / self.audio_total_ticks
            } else {
                0
            };

            // Generate SegmentTimeline for audio
            let audio_segment_timeline = self.generate_segment_timeline(&self.audio_segments);

            format!(
                "        <AdaptationSet id=\"1\" contentType=\"audio\" segmentAlignment=\"true\">\n            <Representation id=\"1\" mimeType=\"audio/mp4\" codecs=\"{}\" bandwidth=\"{}\" audioSamplingRate=\"{}\" >\n                <SegmentTemplate timescale=\"{}\" initialization=\"audio_init.m4s\" media=\"audio_seg_$Number%04d$.m4s\" startNumber=\"1\">\n{}\n                </SegmentTemplate>\n            </Representation>\n        </AdaptationSet>\n",
                a_writer.codec_string,
                audio_bandwidth,
                a_writer.sample_rate,
                a_writer.timescale,
                audio_segment_timeline
            )
        } else {
            String::new()
        };

        // Use computed frame_rate, default to 30 if unavailable
        let fps_val = if self.frame_rate > 0 {
            self.frame_rate
        } else {
            30
        };

        // Calculate pixel aspect ratio (PAR) based on video dimension
        let par_str = if self.video_width > 0 && self.video_height > 0 {
            let mut w = self.video_width;
            let mut h = self.video_height;
            // gcd
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

        // Build MPD with SegmentTimeline instead of fixed duration
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
    <ProgramInformation/>\n    <ServiceDescription id=\"0\"/>\n    <Period id=\"0\" start=\"PT0.0S\">\n        <AdaptationSet id=\"0\" contentType=\"video\" startWithSAP=\"1\" segmentAlignment=\"true\" bitstreamSwitching=\"true\" frameRate=\"{fps}/1\" maxWidth=\"{width}\" maxHeight=\"{height}\" par=\"{par}\">\n            <Representation id=\"0\" mimeType=\"video/mp4\" codecs=\"{codec}\" bandwidth=\"{bandwidth}\" width=\"{width}\" height=\"{height}\" sar=\"1:1\">\n                <SegmentTemplate timescale=\"{timescale}\" initialization=\"init.m4s\" media=\"seg_$Number%04d$.m4s\" startNumber=\"1\">\n{video_timeline}\n                </SegmentTemplate>\n            </Representation>\n        </AdaptationSet>\n        {audio_section}\n    </Period>\n</MPD>\n",
            media_duration = media_presentation_duration,
            max_seg_dur = max_segment_duration,
            min_buf = min_buffer_time,
            width = self.video_width,
            height = self.video_height,
            timescale = self.timescale,
            codec = self.video_codec,
            fps = fps_val,
            bandwidth = video_bandwidth,
            par = par_str,
            video_timeline = video_segment_timeline,
            audio_section = audio_section,
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
