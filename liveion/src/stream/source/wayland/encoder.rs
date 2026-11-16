//! Thin wrapper over the `openh264` crate: I420 in, Annex-B access units out.
//!
//! OpenH264 emits SPS/PPS in-band ahead of every IDR (any non-listing
//! SPS/PPS strategy), which is what WebRTC late joiners need. The safe API
//! has no runtime bitrate retune; [`H264Encoder::set_bitrate`] recreates the
//! encoder instead — sub-millisecond, and the fresh encoder's first frame is
//! an IDR with parameter sets, so subscribers recover immediately.
//!
//! Frame dimensions are not configured here: OpenH264 (re)initializes from
//! the input frame size on the first `encode` and on any size change.

use anyhow::Result;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile,
    RateControlMode, SpsPpsStrategy, UsageType, VuiConfig,
};
use openh264::formats::YUVSlices;
use openh264::{OpenH264API, Timestamp};

use super::convert::I420Planes;

/// One encoded access unit (Annex-B byte stream).
pub struct EncodedAu {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
}

pub struct H264Encoder {
    fps: u32,
    bitrate: u32,
    encoder: Encoder,
    /// OpenH264's ForceIntraFrame is only valid on an initialized encoder
    /// (the first `encode` initializes it); a keyframe request landing
    /// before that is a no-op because a fresh encoder starts with an IDR.
    started: bool,
}

impl H264Encoder {
    pub fn new(fps: u32, bitrate: u32) -> Result<Self> {
        let encoder = build_encoder(fps, bitrate)?;
        Ok(Self {
            fps,
            bitrate,
            encoder,
            started: false,
        })
    }

    pub fn request_keyframe(&mut self) {
        if self.started {
            self.encoder.force_intra_frame();
        }
    }

    /// Move to a new target bitrate. Already at the target leaves the
    /// running encoder untouched.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<()> {
        if bitrate == self.bitrate {
            return Ok(());
        }
        self.encoder = build_encoder(self.fps, bitrate)?;
        self.bitrate = bitrate;
        self.started = false;
        Ok(())
    }

    /// Encode one I420 frame. `Ok(None)` when the encoder produced no
    /// output for this frame (skip).
    pub fn encode(&mut self, planes: I420Planes<'_>, pts_us: u64) -> Result<Option<EncodedAu>> {
        let (width, height) = (planes.width as usize, planes.height as usize);
        let yuv = YUVSlices::new(
            (planes.y, planes.u, planes.v),
            (width, height),
            (width, width / 2, width / 2),
        );

        let stream = self
            .encoder
            .encode_at(&yuv, Timestamp::from_millis(pts_us / 1000))?;
        self.started = true;

        let frame_type = stream.frame_type();
        let data = stream.to_vec();
        if data.is_empty() || matches!(frame_type, FrameType::Skip | FrameType::Invalid) {
            return Ok(None);
        }

        Ok(Some(EncodedAu {
            data,
            is_keyframe: matches!(frame_type, FrameType::IDR | FrameType::I),
        }))
    }
}

fn build_encoder(fps: u32, bitrate: u32) -> Result<Encoder> {
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(bitrate))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        .usage_type(UsageType::ScreenContentRealTime)
        .profile(Profile::Baseline)
        .sps_pps_strategy(SpsPpsStrategy::IncreasingId)
        // A periodic IDR roughly every two seconds lets late joiners in even
        // when their startup PLI got lost.
        .intra_frame_period(IntraFramePeriod::from_num_frames(fps.saturating_mul(2)))
        .skip_frames(false)
        .vui(VuiConfig::bt601());

    Ok(Encoder::with_api_config(
        OpenH264API::from_source(),
        config,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray_frame(width: u32, height: u32, luma: u8) -> I420Planes<'static> {
        let y = vec![luma; (width * height) as usize];
        let c = vec![128u8; (width * height / 4) as usize];
        // Leak the buffers: test-only, keeps I420Planes' borrowed shape.
        let y: &'static [u8] = y.leak();
        let u: &'static [u8] = c.clone().leak();
        let v: &'static [u8] = c.leak();
        I420Planes {
            y,
            u,
            v,
            width,
            height,
        }
    }

    fn nal_types(au: &[u8]) -> Vec<u8> {
        let mut types = Vec::new();
        let mut pos = 0;
        while pos + 4 < au.len() {
            let start = if au[pos] == 0 && au[pos + 1] == 0 && au[pos + 2] == 0 && au[pos + 3] == 1
            {
                Some(4)
            } else if au[pos] == 0 && au[pos + 1] == 0 && au[pos + 2] == 1 {
                Some(3)
            } else {
                None
            };
            if let Some(len) = start {
                types.push(au[pos + len] & 0x1f);
                pos += len;
            } else {
                pos += 1;
            }
        }
        types
    }

    #[test]
    fn first_frame_is_idr_with_inband_params_and_periodic_idrs_follow() {
        let fps = 15;
        let mut enc = H264Encoder::new(fps, 500_000).unwrap();
        let mut idr_frames = Vec::new();
        for i in 0..fps * 3 {
            let au = enc
                .encode(gray_frame(320, 240, (i % 255) as u8), i as u64 * 66_666)
                .unwrap()
                .expect("frame must encode");
            if au.is_keyframe {
                idr_frames.push(i);
                let types = nal_types(&au.data);
                assert!(types.contains(&7), "IDR AU must carry SPS: {types:?}");
                assert!(types.contains(&8), "IDR AU must carry PPS: {types:?}");
                assert!(
                    types.contains(&5),
                    "IDR AU must carry an IDR slice: {types:?}"
                );
            }
        }
        assert_eq!(idr_frames.first(), Some(&0), "first frame must be an IDR");
        assert!(
            idr_frames.len() >= 2,
            "expected a periodic IDR within {} frames, got IDRs at {idr_frames:?}",
            fps * 3
        );
    }

    #[test]
    fn keyframe_request_forces_idr() {
        let mut enc = H264Encoder::new(15, 500_000).unwrap();
        for i in 0..10u32 {
            if i == 5 {
                enc.request_keyframe();
            }
            let au = enc
                .encode(gray_frame(160, 120, (i * 20) as u8), i as u64 * 66_666)
                .unwrap()
                .expect("frame must encode");
            if i == 5 {
                assert!(
                    au.is_keyframe,
                    "frame after request_keyframe must be an IDR"
                );
            }
        }
    }

    #[test]
    fn set_bitrate_recreates_encoder_with_fresh_idr() {
        let mut enc = H264Encoder::new(15, 500_000).unwrap();
        enc.encode(gray_frame(160, 120, 10), 0).unwrap();
        enc.encode(gray_frame(160, 120, 20), 66_666).unwrap();
        enc.set_bitrate(1_000_000).unwrap();
        let au = enc
            .encode(gray_frame(160, 120, 30), 133_333)
            .unwrap()
            .expect("frame must encode");
        assert!(au.is_keyframe, "recreated encoder must start with an IDR");
    }
}
