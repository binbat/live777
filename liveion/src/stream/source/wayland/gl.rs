//! GL compositing for the Wayland compositor: GBM + EGL + smithay's
//! `GlesRenderer`, reading the composited canvas back to CPU memory for the
//! encode pipeline.
//!
//! Everything here is optional: [`GlPipeline::init`] returns `None` on any
//! failure (no DRM device, no EGL, no usable GLES) and the compositor stays
//! on the CPU blit path. The compositor owns the pipeline on its event-loop
//! thread — `GlesRenderer` is `!Send` by construction.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::allocator::{Fourcc, dmabuf::Dmabuf};
use smithay::backend::drm::DrmNode;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{
    Bind, Color32F, ExportMem, Frame, ImportDma, ImportMemWl, Offscreen, Renderer,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::SurfaceData;

/// DRM device candidates, in order. `LIVE777_WAYLAND_DRM_DEVICE` overrides.
pub fn device_candidates() -> Vec<PathBuf> {
    match std::env::var_os("LIVE777_WAYLAND_DRM_DEVICE") {
        Some(path) => vec![PathBuf::from(path)],
        None => vec![
            PathBuf::from("/dev/dri/renderD128"),
            PathBuf::from("/dev/dri/card0"),
        ],
    }
}

/// One composited-frame input: a client buffer imported as a texture plus
/// its position (canvas pixels, top-left origin).
pub struct DrawItem {
    pub texture: GlesTexture,
    pub pos: Point<i32, Physical>,
}

pub struct GlPipeline {
    renderer: GlesRenderer,
    /// Persistent offscreen render target, canvas-sized (recreated on
    /// source restart, never per frame).
    target: GlesTexture,
    size: Size<i32, Physical>,
    /// The DRM node the renderer runs on (dmabuf feedback main device).
    node: DrmNode,
}

impl GlPipeline {
    /// Bring up GBM + EGL + the GLES renderer and prove the full path
    /// (render + readback) with a probe frame. `None` — with a warning —
    /// when any candidate device fails.
    pub fn init(socket_name: &str, width: u32, height: u32) -> Option<Self> {
        for path in device_candidates() {
            match Self::try_init(&path, width, height) {
                Ok(pipeline) => {
                    tracing::info!(
                        "wayland compositor '{socket_name}': GL compositing active on {}",
                        path.display()
                    );
                    return Some(pipeline);
                }
                Err(e) => {
                    tracing::warn!(
                        "wayland compositor '{socket_name}': GL init on {} failed: {e:#}",
                        path.display()
                    );
                }
            }
        }
        tracing::warn!(
            "wayland compositor '{socket_name}': no usable GPU/EGL — staying on CPU compositing (SHM clients only)"
        );
        None
    }

    fn try_init(path: &PathBuf, width: u32, height: u32) -> Result<Self> {
        let node = DrmNode::from_path(path)
            .with_context(|| format!("{} is not a DRM node", path.display()))?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let device = GbmDevice::new(file).context("gbm_create_device failed")?;
        // Safety: the GBM device moves into the display and is not shared
        // with any other EGLDisplay creation.
        let display = unsafe { EGLDisplay::new(device) }.context("EGL display init failed")?;
        let context = EGLContext::new(&display).context("EGL context init failed")?;
        // Safety: the renderer is only ever used on the compositor's
        // event-loop thread (the GL context is never made current elsewhere).
        let mut renderer =
            unsafe { GlesRenderer::new(context) }.context("GLES renderer init failed")?;
        let size = Size::from((width as i32, height as i32));
        let target: GlesTexture = Offscreen::create_buffer(
            &mut renderer,
            Fourcc::Argb8888,
            Size::from((width as i32, height as i32)),
        )
        .context("offscreen render target allocation failed")?;

        let mut pipeline = Self {
            renderer,
            target,
            size,
            node,
        };
        // Prove render + readback now, so a half-working driver falls back
        // to CPU compositing at startup instead of failing per frame.
        let mut probe = vec![0u8; (width * height * 4) as usize];
        pipeline
            .composite(&mut probe, &[])
            .context("readback probe failed")?;
        Ok(pipeline)
    }

    /// The DRM node the renderer runs on.
    pub fn drm_node(&self) -> DrmNode {
        self.node
    }

    /// Dmabuf formats the renderer can import (for the dmabuf global's
    /// feedback and the import-time sanity filter).
    pub fn import_formats(&self) -> FormatSet {
        self.renderer.dmabuf_formats()
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> Result<GlesTexture> {
        self.renderer
            .import_dmabuf(dmabuf, None)
            .map_err(|e| anyhow::anyhow!("dmabuf import failed: {e}"))
    }

    pub fn import_shm(
        &mut self,
        buffer: &WlBuffer,
        surface: &SurfaceData,
        damage: &[Rectangle<i32, Buffer>],
    ) -> Result<GlesTexture> {
        self.renderer
            .import_shm_buffer(buffer, Some(surface), damage)
            .map_err(|e| anyhow::anyhow!("shm import failed: {e}"))
    }

    /// Composite all draw items onto the offscreen target and read the
    /// result back into `canvas` (top-down B,G,R,X rows, tight stride).
    pub fn composite(&mut self, canvas: &mut [u8], items: &[DrawItem]) -> Result<()> {
        let mut framebuffer = self
            .renderer
            .bind(&mut self.target)
            .context("failed to bind the render target")?;
        {
            // Flipped180 (vertical flip): GL readback yields bottom-up rows,
            // so rendering flipped stores the canvas top-down in memory.
            let mut frame = self
                .renderer
                .render(&mut framebuffer, self.size, Transform::Flipped180)
                .context("failed to begin a frame")?;
            let full = Rectangle::from_size(self.size);
            frame
                .clear(Color32F::TRANSPARENT, &[full])
                .context("clear failed")?;
            for item in items {
                frame
                    .render_texture_at(
                        &item.texture,
                        item.pos,
                        1,
                        1.0,
                        Transform::Normal,
                        &[full],
                        &[],
                        1.0,
                    )
                    .context("texture draw failed")?;
            }
            // finish() hands back a fence for the rendered frame; wait on it
            // before the readback so the pixels are complete.
            let sync = frame.finish().context("frame finish failed")?;
            self.renderer
                .wait(&sync)
                .context("frame fence wait failed")?;
        }

        let region = Rectangle::new(Point::from((0, 0)), Size::from((self.size.w, self.size.h)));
        let mapping = self
            .renderer
            .copy_framebuffer(&framebuffer, region, Fourcc::Argb8888)
            .context("framebuffer readback failed")?;
        let bytes = self
            .renderer
            .map_texture(&mapping)
            .context("readback mapping failed")?;
        anyhow::ensure!(
            bytes.len() == canvas.len(),
            "readback size mismatch: {} != {}",
            bytes.len(),
            canvas.len()
        );
        canvas.copy_from_slice(bytes);
        Ok(())
    }

    /// Drop cached client-buffer textures (a toplevel or wl_buffer went
    /// away), releasing the underlying dmabufs.
    pub fn cleanup_caches(&mut self) {
        let _ = self.renderer.cleanup_texture_cache();
    }
}
