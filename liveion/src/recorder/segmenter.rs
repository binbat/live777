use std::io::Cursor;
use std::time::Duration;

use anyhow::Result;
use byteorder::{BigEndian, ByteOrder};
use bytes::Bytes;
use h264_reader::{
    nal::sps::SeqParameterSet,
    rbsp::{decode_nal, BitReader},
};
use mp4::{AvcConfig, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType};
use opendal::Operator;
use tracing::{debug, info};

/// Default duration of each segment in seconds
const DEFAULT_SEG_DURATION: u64 = 10;

pub struct Segmenter {
    op: Operator,
    stream: String,
    path_prefix: String,
    timescale: u32,
    duration_per_seg: Duration,
    // Length of each segment (in timescale units) for fast comparison
    seg_duration_ticks: u64,

    // fragment index
    seg_index: u32,
    // Decode timestamp of current segment start (in timescale units)
    seg_start_dts: u64,
    video_track_id: Option<u32>,

    // All samples buffered for the current segment (already converted to AVCC length prefix)
    samples: Vec<Mp4Sample>,

    // codec config collection before init
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,

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
}

impl Segmenter {
    pub async fn new(op: Operator, stream: String, root_prefix: String) -> Result<Self> {
        Ok(Self {
            op,
            stream: stream.clone(),
            path_prefix: root_prefix,
            timescale: 90_000,
            duration_per_seg: Duration::from_secs(DEFAULT_SEG_DURATION),
            seg_duration_ticks: 90_000u64 * DEFAULT_SEG_DURATION,
            seg_index: 0,
            seg_start_dts: 0,
            video_track_id: None,
            samples: Vec::new(),
            sps: None,
            pps: None,
            video_width: 0,
            video_height: 0,
            video_codec: String::new(),
            current_pts: 0,
            frame_rate: 0,
            total_bytes: 0,
            total_ticks: 0,
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
        let mut offset = 0usize;
        let mut avcc_payload = Vec::<u8>::new();

        let bytes = frame.as_ref();
        while offset + 3 < bytes.len() {
            // Find start code 0x000001 or 0x00000001
            let (start_code_len, start_pos) = if bytes[offset..].starts_with(&[0, 0, 1]) {
                (3, offset)
            } else if bytes[offset..].starts_with(&[0, 0, 0, 1]) {
                (4, offset)
            } else {
                offset += 1;
                continue;
            };

            // Locate the next start code to get the current NALU range
            let mut next = start_pos + start_code_len;
            while next + 3 < bytes.len()
                && !bytes[next..].starts_with(&[0, 0, 1])
                && !bytes[next..].starts_with(&[0, 0, 0, 1])
            {
                next += 1;
            }

            // If we reached near the end without finding another start code, include the trailing bytes
            if next + 3 >= bytes.len() {
                next = bytes.len();
            }

            let nalu = &bytes[start_pos..next];

            // Strip the start code and get the NALU type
            let header_idx = if nalu.starts_with(&[0, 0, 0, 1]) {
                4
            } else {
                3
            };
            if nalu.len() <= header_idx {
                offset = next;
                continue;
            }
            let nal_type = nalu[header_idx] & 0x1F;

            // Collect SPS/PPS
            match nal_type {
                7 => self.sps = Some(nalu[header_idx..].to_vec()),
                8 => self.pps = Some(nalu[header_idx..].to_vec()),
                _ => {}
            }

            // Convert to AVCC and append to frame payload
            avcc_payload.extend_from_slice(&nalu_to_avcc(&Bytes::copy_from_slice(nalu)));

            // Detect IDR frame automatically if any NALU type is 5 (IDR slice)
            if nal_type == 5 {
                is_idr = true;
            }

            offset = next;
        }

        // Use provided duration, fallback to 3000 ticks if it looks invalid (e.g. 0)
        let dur = if duration_ticks == 0 {
            3_000
        } else {
            duration_ticks
        } as u32;

        let sample = Mp4Sample {
            start_time: self.current_pts,
            duration: dur,
            rendering_offset: 0,
            is_sync: is_idr,
            bytes: avcc_payload.into(),
        };
        self.samples.push(sample);
        self.current_pts += dur as u64;

        // Update dynamic statistics
        self.total_bytes += frame.len() as u64;
        self.total_ticks += dur as u64;

        // Dynamically update nominal FPS: prefer the highest observed value
        let fps_cur = (self.timescale / dur) as u32;
        if self.frame_rate == 0 || fps_cur > self.frame_rate {
            self.frame_rate = fps_cur;
        }

        // Now that we (likely) have frame rate, generate init segment if not yet written
        if self.video_track_id.is_none() && self.sps.is_some() && self.pps.is_some() {
            self.init_writer().await?;
            // init_writer will call open_new_segment(), clear samples, so ensure it's called before samples are enqueued
        }

        // Check if we need to roll the segment: IDR + duration met
        if is_idr && (self.current_pts - self.seg_start_dts >= self.seg_duration_ticks) {
            self.roll_segment().await?;
        }

        Ok(())
    }

    async fn init_writer(&mut self) -> Result<()> {
        let cursor = Cursor::new(Vec::new());
        let mp4_cfg = Mp4Config {
            major_brand: "isom".parse().unwrap(),
            minor_version: 512,
            compatible_brands: vec![
                "isom".parse().unwrap(),
                "iso2".parse().unwrap(),
                "avc1".parse().unwrap(),
                "mp41".parse().unwrap(),
            ],
            timescale: self.timescale,
        };
        let mut writer = Mp4Writer::write_start(cursor, &mp4_cfg)?;

        // Parse SPS to get width/height if possible
        let (mut width, mut height) = (0u32, 0u32);
        if let Ok(rbsp) = decode_nal(&self.sps.clone().unwrap()) {
            if let Ok(sps) = SeqParameterSet::from_bits(BitReader::new(&rbsp[..])) {
                if let Ok((w, h)) = sps.pixel_dimensions() {
                    width = w;
                    height = h;
                    info!(
                        "[segmenter] {} SPS parsed width={} height={}",
                        self.stream, width, height
                    );
                }
            }
        }

        // Parse profile/level and construct codec string avc1.PPCCLL
        if self.video_codec.is_empty() {
            if let Some(sps_bytes) = &self.sps {
                if sps_bytes.len() >= 4 {
                    let profile_idc = sps_bytes[1];
                    let constraints = sps_bytes[2];
                    let level_idc = sps_bytes[3];
                    self.video_codec = format!(
                        "avc1.{:02x}{:02x}{:02x}",
                        profile_idc, constraints, level_idc
                    );
                } else {
                    self.video_codec = "avc1".to_string();
                }
            } else {
                self.video_codec = "avc1".to_string();
            }
        }

        // Save to member fields for generating the MPD
        self.video_width = width;
        self.video_height = height;

        // TrackConfig
        let avc_config = AvcConfig {
            width: width as u16,
            height: height as u16,
            seq_param_set: self.sps.clone().unwrap(),
            pic_param_set: self.pps.clone().unwrap(),
        };
        let track_cfg = TrackConfig {
            track_type: TrackType::Video,
            timescale: self.timescale,
            language: "und".into(),
            media_conf: MediaConfig::AvcConfig(avc_config),
        };
        writer.add_track(&track_cfg)?;
        // video track id is 1 (first) according to implementation
        self.video_track_id = Some(1);
        writer.write_end()?;

        let track_id = self.video_track_id.unwrap_or(1);
        let init_bytes = inject_mvex(writer.into_writer().into_inner(), track_id);
        self.store_file("init.m4s", init_bytes).await?;
        info!("[segmenter] {} init.m4s written", self.stream);

        // Generate or update the MPD manifest
        self.write_manifest().await?;

        // Start a new segment: reset timers and caches
        self.open_new_segment().await?;
        Ok(())
    }

    async fn open_new_segment(&mut self) -> Result<()> {
        // Mp4Writer is no longer created; we buffer samples and let roll_segment build moof+mdat
        self.samples.clear();
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
        let track_id = self.video_track_id.unwrap_or(1);

        let fragment = build_fragment(track_id, self.seg_index, base_time, &self.samples);
        let filename = format!("seg_{:04}.m4s", self.seg_index);
        self.store_file(&filename, fragment).await?;
        info!("[segmenter] {} {} written", self.stream, filename);

        // Clear the cache and start the next segment
        self.open_new_segment().await?;

        // Update the MPD manifest
        self.write_manifest().await?;
        Ok(())
    }

    async fn write_manifest(&self) -> Result<()> {
        // Compute total media duration (completed segments) in seconds
        let seg_count = if self.seg_index > 0 {
            self.seg_index - 1
        } else {
            0
        } as u64;
        let seg_duration_ticks = (self.timescale as u64) * self.duration_per_seg.as_secs();
        let total_duration_ticks = seg_duration_ticks * seg_count;

        // Represent duration in ISO8601 PT format with second-level precision
        let total_duration_secs = total_duration_ticks as f64 / self.timescale as f64;
        let media_presentation_duration = format!("PT{:.3}S", total_duration_secs);
        let max_segment_duration = format!("PT{}S", self.duration_per_seg.as_secs());
        let min_buffer_time = if self.duration_per_seg.as_secs() * 3 > 0 {
            format!("PT{}S", self.duration_per_seg.as_secs() * 3)
        } else {
            "PT1S".to_string()
        };

        // Estimate average bitrate (bits per second) if we have stats
        let bandwidth = if self.total_ticks > 0 {
            (self.total_bytes * 8 * self.timescale as u64 / self.total_ticks) as u64
        } else {
            0
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

        // Duration of each segment in timescale units
        let seg_duration_ticks = self.timescale as u64 * self.duration_per_seg.as_secs();

        // Build MPD with fixed-duration SegmentTemplate (no SegmentTimeline)
        let mpd = format!(
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
    <ProgramInformation/>\n    <ServiceDescription id=\"0\"/>\n    <Period id=\"0\" start=\"PT0.0S\">\n        <AdaptationSet id=\"0\" contentType=\"video\" startWithSAP=\"1\" segmentAlignment=\"true\" bitstreamSwitching=\"true\" frameRate=\"{fps}/1\" maxWidth=\"{width}\" maxHeight=\"{height}\" par=\"{par}\">\n            <Representation id=\"0\" mimeType=\"video/mp4\" codecs=\"{codec}\" bandwidth=\"{bandwidth}\" width=\"{width}\" height=\"{height}\" sar=\"1:1\">\n                <SegmentTemplate timescale=\"{timescale}\" initialization=\"init.m4s\" media=\"seg_$Number%04d$.m4s\" duration=\"{seg_duration}\" startNumber=\"1\" />\n            </Representation>\n        </AdaptationSet>\n    </Period>\n</MPD>\n",
            media_duration = media_presentation_duration,
            max_seg_dur = max_segment_duration,
            min_buf = min_buffer_time,
            width = self.video_width,
            height = self.video_height,
            timescale = self.timescale,
            seg_duration = seg_duration_ticks,
            codec = self.video_codec,
            fps = fps_val,
            bandwidth = bandwidth,
            par = par_str,
        );

        self.store_file("manifest.mpd", mpd.into_bytes()).await
    }

    async fn store_file(&self, name: &str, data: Vec<u8>) -> Result<()> {
        let path = format!("{}/{}", self.path_prefix, name);
        let mut w = self.op.writer_with(&path).await?;
        w.write(data).await?;
        w.close().await?;
        debug!("[segmenter] stored file {}", path);
        Ok(())
    }
}

// ===== Utility: insert mvex/trex boxes into fMP4 to convert it into fragmented MP4 =====
/// Create mvex+trex boxes based on TrackID (total 40 bytes)
fn build_mvex(track_id: u32) -> Vec<u8> {
    let trex_size: u32 = 32; // fixed size
    let mvex_size: u32 = 8 + trex_size; // mvex header + trex

    let mut buf = Vec::with_capacity(mvex_size as usize);
    buf.extend_from_slice(&mvex_size.to_be_bytes()); // mvex size
    buf.extend_from_slice(b"mvex");

    // trex box
    buf.extend_from_slice(&trex_size.to_be_bytes()); // trex size
    buf.extend_from_slice(b"trex");
    buf.extend_from_slice(&[0u8; 4]); // version(0)+flags(0)
    buf.extend_from_slice(&track_id.to_be_bytes()); // track_ID
    buf.extend_from_slice(&1u32.to_be_bytes()); // default_sample_description_index
    buf.extend_from_slice(&0u32.to_be_bytes()); // default_sample_duration
    buf.extend_from_slice(&0u32.to_be_bytes()); // default_sample_size
                                                // Mark default samples as non-sync so that we must explicitly set the sync flag for IDR samples in the fragment
                                                // sample_depends_on = 1 (depends on other frame), sample_is_non_sync = 1
    let default_flags: u32 = 0x0101_0000;
    buf.extend_from_slice(&default_flags.to_be_bytes()); // default_sample_flags
    buf
}

/// inject mvex into mp4 data's moov box, and update moov size
fn inject_mvex(mut data: Vec<u8>, track_id: u32) -> Vec<u8> {
    let mvex = build_mvex(track_id);

    let mut offset = 0usize;
    while offset + 8 <= data.len() {
        let size = BigEndian::read_u32(&data[offset..offset + 4]) as usize;
        let box_type = &data[offset + 4..offset + 8];
        if box_type == b"moov" {
            let insert_pos = offset + size; // moov end
            let new_size = size + mvex.len();
            // update moov size
            BigEndian::write_u32(&mut data[offset..offset + 4], new_size as u32);
            // insert mvex
            data.splice(insert_pos..insert_pos, mvex.iter().cloned());
            break;
        }
        if size == 0 {
            // Avoid infinite loop
            break;
        }
        offset += size;
    }
    data
}

// ===== Convert AnnexB NALU to AVCC (length prefix) =====
fn nalu_to_avcc(nalu: &Bytes) -> Vec<u8> {
    // Skip the 3/4-byte start code
    let offset = if nalu.len() >= 4 && &nalu[..4] == &[0, 0, 0, 1][..] {
        4
    } else if nalu.len() >= 3 && &nalu[..3] == &[0, 0, 1][..] {
        3
    } else {
        0
    };
    let payload = &nalu[offset..];
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

// ===== build moof + mdat fragment =====
fn build_fragment(
    track_id: u32,
    seq_number: u32,
    base_time: u64,
    samples: &[Mp4Sample],
) -> Vec<u8> {
    use byteorder::ByteOrder;

    let total_data: usize = samples.iter().map(|s| s.bytes.len()).sum();

    // ========= styp =========
    let mut fragment: Vec<u8> = Vec::with_capacity(1024 + total_data);
    const STYP_SIZE: u32 = 24;
    fragment.extend_from_slice(&STYP_SIZE.to_be_bytes());
    fragment.extend_from_slice(b"styp");
    fragment.extend_from_slice(b"msdh");
    fragment.extend_from_slice(&0u32.to_be_bytes()); // minor version
    fragment.extend_from_slice(b"msdh");
    fragment.extend_from_slice(b"dash");

    // ========= moof =========
    let moof_start = fragment.len();
    fragment.extend_from_slice(&[0u8; 8]); // placeholder for moof size+type

    // ---- mfhd ----
    fragment.extend_from_slice(&16u32.to_be_bytes());
    fragment.extend_from_slice(b"mfhd");
    fragment.extend_from_slice(&0u32.to_be_bytes());
    fragment.extend_from_slice(&seq_number.to_be_bytes());

    // ---- traf ----
    let traf_start = fragment.len();
    fragment.extend_from_slice(&[0u8; 8]); // placeholder traf header

    // tfhd
    let tfhd_flags: u32 = 0x000200; // default-base-is-moof
    fragment.extend_from_slice(&16u32.to_be_bytes());
    fragment.extend_from_slice(b"tfhd");
    fragment.extend_from_slice(&tfhd_flags.to_be_bytes());
    fragment.extend_from_slice(&track_id.to_be_bytes());

    // tfdt (version 1)
    fragment.extend_from_slice(&20u32.to_be_bytes());
    fragment.extend_from_slice(b"tfdt");
    fragment.extend_from_slice(&0x01000000u32.to_be_bytes());
    fragment.extend_from_slice(&base_time.to_be_bytes());

    // trun
    let sample_count = samples.len() as u32;
    // trun flags: data-offset present | sample-duration present | sample-size present | sample-flags present
    let trun_flags: u32 = 0x000001 | 0x000100 | 0x000200 | 0x000400;
    let trun_start = fragment.len();
    fragment.extend_from_slice(&[0u8; 4]); // placeholder size
    fragment.extend_from_slice(b"trun");
    fragment.extend_from_slice(&trun_flags.to_be_bytes());
    fragment.extend_from_slice(&sample_count.to_be_bytes());
    let data_offset_pos = fragment.len();
    fragment.extend_from_slice(&[0u8; 4]); // data offset placeholder

    for s in samples {
        fragment.extend_from_slice(&(s.duration as u32).to_be_bytes());
        fragment.extend_from_slice(&(s.bytes.len() as u32).to_be_bytes());
        // Sample flags: mark sync vs non-sync samples
        let flags: u32 = if s.is_sync {
            // sample_depends_on = 2 (no dependencies, key frame)
            0x0200_0000
        } else {
            // sample_depends_on = 1, sample_is_non_sync = 1
            0x0101_0000
        };
        fragment.extend_from_slice(&flags.to_be_bytes());
    }

    // patch sizes
    let trun_size = (fragment.len() - trun_start) as u32;
    BigEndian::write_u32(&mut fragment[trun_start..trun_start + 4], trun_size);

    let traf_size = (fragment.len() - traf_start) as u32;
    BigEndian::write_u32(&mut fragment[traf_start..traf_start + 4], traf_size);
    fragment[traf_start + 4..traf_start + 8].copy_from_slice(b"traf");

    let moof_size = (fragment.len() - moof_start) as u32;
    BigEndian::write_u32(&mut fragment[moof_start..moof_start + 4], moof_size);
    fragment[moof_start + 4..moof_start + 8].copy_from_slice(b"moof");

    // data-offset: distance from moof start to first byte of mdat payload
    let data_offset_val = moof_size + 8; // mdat header
    BigEndian::write_u32(
        &mut fragment[data_offset_pos..data_offset_pos + 4],
        data_offset_val,
    );

    // ========= mdat =========
    let mdat_size = (8 + total_data) as u32;
    fragment.extend_from_slice(&mdat_size.to_be_bytes());
    fragment.extend_from_slice(b"mdat");
    for s in samples {
        fragment.extend_from_slice(s.bytes.as_ref());
    }

    fragment
}
