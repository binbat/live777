//! XRGB8888 → I420 conversion (BT.601 limited range) via `yuvutils-rs`.

use anyhow::Result;
use yuvutils_rs::{
    BufferStoreMut, YuvConversionMode, YuvPlanarImageMut, YuvRange, YuvStandardMatrix,
    bgra_to_yuv420,
};

use super::ArgbFrame;

/// Borrowed I420 planes of one converted frame, tightly strided.
pub struct I420Planes<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub width: u32,
    pub height: u32,
}

/// Converts frames into a reused internal I420 buffer (no per-frame alloc).
pub struct I420Converter {
    width: u32,
    height: u32,
    yuv: Vec<u8>,
}

impl I420Converter {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            yuv: vec![0u8; (width * height * 3 / 2) as usize],
        }
    }

    /// Convert one XRGB8888 frame (little-endian B,G,R,X byte order, so the
    /// BGRA converter applies; the X byte is skipped).
    pub fn convert(&mut self, frame: &ArgbFrame) -> Result<()> {
        let y_len = (self.width * self.height) as usize;
        let c_len = y_len / 4;
        let (y_plane, uv) = self.yuv.split_at_mut(y_len);
        let (u_plane, v_plane) = uv.split_at_mut(c_len);

        let mut image = YuvPlanarImageMut {
            y_plane: BufferStoreMut::Borrowed(y_plane),
            y_stride: self.width,
            u_plane: BufferStoreMut::Borrowed(u_plane),
            u_stride: self.width / 2,
            v_plane: BufferStoreMut::Borrowed(v_plane),
            v_stride: self.width / 2,
            width: self.width,
            height: self.height,
        };
        bgra_to_yuv420(
            &mut image,
            &frame.data,
            frame.stride,
            YuvRange::Limited,
            YuvStandardMatrix::Bt601,
            YuvConversionMode::Balanced,
        )?;
        Ok(())
    }

    pub fn planes(&self) -> I420Planes<'_> {
        let y_len = (self.width * self.height) as usize;
        let c_len = y_len / 4;
        I420Planes {
            y: &self.yuv[..y_len],
            u: &self.yuv[y_len..y_len + c_len],
            v: &self.yuv[y_len + c_len..],
            width: self.width,
            height: self.height,
        }
    }
}
