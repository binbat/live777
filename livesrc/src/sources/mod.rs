use anyhow::Result;

/// Trait that all source handlers must implement.
pub trait Source {
    /// Start the source (e.g., launch ffmpeg, open V4L2 device, connect to RTSP, etc.)
    fn start(&self) -> Result<()>;
    /// Stop the source and clean up resources.
    fn stop(&self) -> Result<()>;
    /// Request a keyframe (IDR) from the source.
    /// This is critical for real-time streaming to ensure new subscribers get video immediately.
    fn request_keyframe(&self) -> Result<()> {
        // Default implementation does nothing (for sources that don't support it)
        Ok(())
    }
}

// ---------- Real implementations ----------

pub mod libcamera;
pub use libcamera::LibcameraSource;

pub mod rpicam;
pub use rpicam::RpicamSource;

// ---------- Placeholder implementations ----------

pub struct V4l2Source;
impl Source for V4l2Source {
    fn start(&self) -> Result<()> { Ok(()) }
    fn stop(&self) -> Result<()> { Ok(()) }
}

pub struct WhipSource;
impl Source for WhipSource {
    fn start(&self) -> Result<()> { Ok(()) }
    fn stop(&self) -> Result<()> { Ok(()) }
}

pub struct RtspSource;
impl Source for RtspSource {
    fn start(&self) -> Result<()> { Ok(()) }
    fn stop(&self) -> Result<()> { Ok(()) }
}

pub struct PublisherSource;
impl Source for PublisherSource {
    fn start(&self) -> Result<()> { Ok(()) }
    fn stop(&self) -> Result<()> { Ok(()) }
}
