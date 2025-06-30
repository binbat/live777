use std::io::{Cursor, Write, Seek};
use std::time::{Duration, Instant};

use anyhow::Result;
use bytes::Bytes;
use mp4::{Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, AvcConfig, TrackType, MediaConfig};
use opendal::Operator;

/// Default duration of each segment in seconds
const DEFAULT_SEG_DURATION: u64 = 2;

pub struct Segmenter {
    op: Operator,
    stream: String,
    path_prefix: String,
    timescale: u32,
    duration_per_seg: Duration,

    // fragment index
    seg_index: u32,
    started_at: Instant,

    // Writer and related fields (presence means writer has been initialized)
    writer: Option<Mp4Writer<Cursor<Vec<u8>>>>,
    video_track_id: Option<u32>,

    // codec config collection before init
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,

    current_pts: u64,
}

impl Segmenter {
    pub async fn new(op: Operator, stream: String, root_prefix: String) -> Result<Self> {
        Ok(Self {
            op,
            stream: stream.clone(),
            path_prefix: root_prefix,
            timescale: 90_000,
            duration_per_seg: Duration::from_secs(DEFAULT_SEG_DURATION),
            seg_index: 0,
            started_at: Instant::now(),
            writer: None,
            video_track_id: None,
            sps: None,
            pps: None,
            current_pts: 0,
        })
    }

    /// Feed one H.264 NALU (Annex-B format with start code)
    pub async fn push_h264(&mut self, nalu: Bytes, is_idr: bool) -> Result<()> {
        // Collect SPS/PPS
        let nal_type = nalu[4] & 0x1F; // assume 4-byte start code
        match nal_type {
            7 => self.sps = Some(nalu.slice(4..).to_vec()),
            8 => self.pps = Some(nalu.slice(4..).to_vec()),
            _ => {}
        }

        // Initialize writer when both SPS and PPS are ready and writer is None
        if self.writer.is_none() && self.sps.is_some() && self.pps.is_some() {
            self.init_writer().await?;
            // init.m4s is generated inside init_writer
        }

        // Skip if writer is not yet ready (waiting for full codec information)
        let Some(writer) = self.writer.as_mut() else { return Ok(()); };
        let track_id = self.video_track_id.unwrap();

        // Build Mp4Sample
        let sample = Mp4Sample {
            start_time: self.current_pts,
            duration: 3_000, // assuming 30 fps
            rendering_offset: 0,
            is_sync: is_idr,
            bytes: nalu.into(),
        };

        writer.write_sample(track_id, &sample)?;
        self.current_pts += sample.duration as u64;

        // Rotate segment when duration threshold reached
        if self.started_at.elapsed() >= self.duration_per_seg {
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

        // TrackConfig
        let avc_config = AvcConfig {
            width: 0,
            height: 0,
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

        let init_bytes = writer.into_writer().into_inner();
        self.store_file("init.m4s", init_bytes).await?;

        // reopen writer for segments
        self.open_new_segment().await?;
        Ok(())
    }

    async fn open_new_segment(&mut self) -> Result<()> {
        self.started_at = Instant::now();
        self.seg_index += 1;
        self.current_pts = 0;

        let cursor = Cursor::new(Vec::new());
        let mut writer = Mp4Writer::write_start(cursor, &Mp4Config {
            major_brand: "isom".parse().unwrap(),
            minor_version: 0,
            compatible_brands: vec!["isom".parse().unwrap()],
            timescale: self.timescale,
        })?;

        // reuse track config
        let avc_config = AvcConfig {
            width: 0,
            height: 0,
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
        self.video_track_id = Some(1);
        self.writer = Some(writer);
        Ok(())
    }

    async fn roll_segment(&mut self) -> Result<()> {
        // flush current writer
        if let Some(mut writer) = self.writer.take() {
            writer.write_end()?;
            let mut cursor = writer.into_writer();
            cursor.seek(std::io::SeekFrom::Start(0))?;
            let data = cursor.into_inner();
            let filename = format!("seg_{:04}.m4s", self.seg_index);
            self.store_file(&filename, data).await?;
        }
        // open next segment
        self.open_new_segment().await?;
        Ok(())
    }

    async fn store_file(&self, name: &str, data: Vec<u8>) -> Result<()> {
        let path = format!("{}/{}", self.path_prefix, name);
        let mut w = self.op.writer_with(&path).await?;
        w.write(data).await?;
        w.close().await?;
        Ok(())
    }
} 