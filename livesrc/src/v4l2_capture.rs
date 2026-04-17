#[cfg(feature = "v4l2")]
use v4l::buffer::Type;
#[cfg(feature = "v4l2")]
use v4l::io::mmap::Stream;
#[cfg(feature = "v4l2")]
use v4l::io::traits::CaptureStream;
#[cfg(feature = "v4l2")]
use v4l::video::Capture;
#[cfg(feature = "v4l2")]
use v4l::{Device, FourCC};

use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::V4l2Config;

#[cfg(feature = "v4l2")]
pub struct V4l2Capture<'a> {
    device: Arc<Device>,
    stream: Option<Stream<'a>>,
    config: V4l2Config,
}

#[cfg(feature = "v4l2")]
impl<'a> V4l2Capture<'a> {
    pub fn new(device_path: &str, config: V4l2Config) -> anyhow::Result<Self> {
        info!("Opening V4L2 device: {}", device_path);
        
        let device = Device::with_path(device_path)?;
        
        // Query device capabilities
        let caps = device.query_caps()?;
        info!(
            "Device: {} (driver: {}, bus: {})",
            caps.card, caps.driver, caps.bus
        );
        
        // Set format
        let fourcc = match config.format.as_str() {
            "H264" => FourCC::new(b"H264"),
            "H265" | "HEVC" => FourCC::new(b"HEVC"),
            _ => anyhow::bail!("Unsupported format: {}", config.format),
        };
        
        let mut fmt = device.format()?;
        fmt.width = config.width;
        fmt.height = config.height;
        fmt.fourcc = fourcc;
        device.set_format(&fmt)?;
        
        info!(
            "Format set: {}x{} {} ({})",
            fmt.width, fmt.height, config.format, fourcc
        );
        
        // Set frame rate
        let mut params = device.params()?;
        params.interval = v4l::Fraction::new(1, config.fps);
        device.set_params(&params)?;
        
        info!("Frame rate set: {} fps", config.fps);
        
        Ok(Self {
            device: Arc::new(device),
            stream: None,
            config,
        })
    }
    
    pub fn start(&mut self) -> anyhow::Result<()> {
        info!("Starting V4L2 capture stream");
        
        // Create mmap stream with 4 buffers
        let stream = Stream::with_buffers(&self.device, Type::VideoCapture, 4)?;
        self.stream = Some(stream);
        
        info!("V4L2 capture stream started with 4 mmap buffers");
        Ok(())
    }
    
    pub async fn capture_loop(
        &mut self,
        frame_tx: mpsc::Sender<Vec<u8>>,
        mut shutdown_rx: mpsc::Receiver<()>,
    ) -> anyhow::Result<()> {
        let stream = self.stream.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Stream not started"))?;
        
        info!("Starting V4L2 capture loop");
        let mut frame_count = 0u64;
        
        loop {
            // Check for shutdown signal (non-blocking)
            if shutdown_rx.try_recv().is_ok() {
                info!("V4L2 capture loop shutdown requested");
                break;
            }
            
            // Capture frame (blocking, but we'll use tokio::task::spawn_blocking)
            let (buf, _meta) = match stream.next() {
                Ok(frame) => frame,
                Err(e) => {
                    error!("Failed to capture frame: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }
            };
            
            frame_count += 1;
            
            // Copy frame data
            let frame_data = buf.to_vec();
            let frame_size = frame_data.len();
            
            if frame_count % 30 == 0 {
                debug!(
                    "Captured frame #{}: {} bytes",
                    frame_count, frame_size
                );
            }
            
            // Send frame to RTP packetizer
            if let Err(e) = frame_tx.send(frame_data).await {
                error!("Failed to send frame to RTP packetizer: {}", e);
                break;
            }
            
            // Yield to allow other tasks to run
            tokio::task::yield_now().await;
        }
        
        info!("V4L2 capture loop ended. Total frames: {}", frame_count);
        Ok(())
    }
    
    pub fn stop(&mut self) {
        info!("Stopping V4L2 capture");
        self.stream = None;
    }
    
    #[allow(dead_code)]
    pub fn request_keyframe(&self) -> anyhow::Result<()> {
        info!("Requesting IDR keyframe");
        
        // Try to force keyframe using V4L2 control
        // V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME
        const V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME: u32 = 0x009909e6;
        
        use v4l::control::Control;
        use v4l::control::Value;
        
        let ctrl = Control {
            id: V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME,
            value: Value::Integer(1),
        };
        
        match self.device.set_control(ctrl) {
            Ok(_) => {
                debug!("IDR keyframe requested successfully");
                Ok(())
            }
            Err(e) => {
                warn!("Failed to request keyframe: {}. Device may not support this control.", e);
                Ok(()) // Don't fail, just warn
            }
        }
    }
}

#[cfg(feature = "v4l2")]
impl<'a> Drop for V4l2Capture<'a> {
    fn drop(&mut self) {
        self.stop();
    }
}

// Stub implementation when v4l2 feature is not enabled
#[cfg(not(feature = "v4l2"))]
pub struct V4l2Capture;

#[cfg(not(feature = "v4l2"))]
impl V4l2Capture {
    pub fn new(_device_path: &str, _config: crate::config::V4l2Config) -> anyhow::Result<Self> {
        anyhow::bail!("V4L2 support not enabled. Rebuild with --features v4l2")
    }
}
