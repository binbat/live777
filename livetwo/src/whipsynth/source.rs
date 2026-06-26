use std::time::Duration;

use anyhow::Result;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::source::{AudioCodec, FrameGenerator, FrameGeneratorConfig, MediaFrame, VideoCodec};

/// Result of asking a [`MediaSource`] for the next frame.
pub enum SourceFrame {
    /// A media frame is available.
    Frame(MediaFrame),
    /// No frame produced this iteration (e.g. encoder is buffering), keep polling.
    Empty,
    /// The source has reached its configured duration or been exhausted.
    End,
}

/// Abstraction over an encoded-frame source for the WHIP publisher.
///
/// A source is single-consumer and `Send` so it can be driven from a blocking
/// encoder thread while the publisher consumes frames on an async task.
pub trait MediaSource: Send {
    /// Produce the next encoded media frame.
    fn next_frame(&mut self) -> Result<SourceFrame>;

    /// Return the configured video codec.
    fn video_codec(&self) -> Option<VideoCodec>;

    /// Return the configured audio codec.
    fn audio_codec(&self) -> Option<AudioCodec>;
}

impl MediaSource for FrameGenerator {
    fn next_frame(&mut self) -> Result<SourceFrame> {
        self.next_frame()
    }

    fn video_codec(&self) -> Option<VideoCodec> {
        self.video_codec()
    }

    fn audio_codec(&self) -> Option<AudioCodec> {
        self.audio_codec()
    }
}

/// Handle returned by [`spawn_source`] that can be used to stop the source
/// thread and wait for it to finish.
pub struct SourceHandle {
    ct: CancellationToken,
    join_handle: Option<JoinHandle<Result<()>>>,
}

impl SourceHandle {
    pub fn new(ct: CancellationToken, join_handle: JoinHandle<Result<()>>) -> Self {
        Self {
            ct,
            join_handle: Some(join_handle),
        }
    }

    /// Signal the source to stop.
    pub fn cancel(&self) {
        self.ct.cancel();
    }

    /// Stop the source and wait for the background task to finish.
    pub async fn stop(mut self) -> Result<()> {
        self.ct.cancel();
        if let Some(handle) = self.join_handle.take() {
            handle.await??;
        }
        Ok(())
    }
}

/// Spawn an rsmpeg frame generator on a dedicated blocking task.
///
/// Frames are sent on the returned channel. The task stops when the
/// cancellation token is triggered or the generator is exhausted.
pub fn spawn_rsmpeg_source(
    config: FrameGeneratorConfig,
    ct: CancellationToken,
) -> Result<(
    tokio::sync::mpsc::UnboundedReceiver<MediaFrame>,
    SourceHandle,
)> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<MediaFrame>();

    let task_ct = ct.clone();
    let handle = tokio::task::spawn_blocking(move || {
        let mut generator = FrameGenerator::new(&config)?;
        loop {
            if task_ct.is_cancelled() {
                break;
            }
            match generator.next_frame() {
                Ok(SourceFrame::Frame(frame)) => {
                    if tx.send(frame).is_err() {
                        break;
                    }
                }
                Ok(SourceFrame::Empty) => {
                    // Encoder is buffering; yield briefly and try again.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(SourceFrame::End) => break,
                Err(e) => {
                    tracing::error!(error = ?e, "frame generator error");
                    return Err(e);
                }
            }
        }
        let _ = generator.flush();
        Ok(())
    });

    Ok((rx, SourceHandle::new(ct, handle)))
}

/// Build a frame generator config from publisher parameters.
pub fn frame_generator_config(
    video_codec: VideoCodec,
    audio_codec: Option<AudioCodec>,
    width: u32,
    height: u32,
    fps: u32,
    duration: Option<Duration>,
) -> FrameGeneratorConfig {
    FrameGeneratorConfig {
        video_codec,
        audio_codec,
        width,
        height,
        fps,
        duration,
    }
}
