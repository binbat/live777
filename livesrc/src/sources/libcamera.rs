use anyhow::Result;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::{Arc, Mutex};
use tokio::io::BufReader;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocalWriter;
use webrtc::util::Unmarshal;

use crate::config::LibcameraConfig;
use crate::rtp::{AnnexBParser, H264Packetizer, NalType, RtpSender, UdpRtpSender};

use super::Source;

/// Libcamera source handler for Raspberry Pi cameras
/// 
/// Uses libcamera-bridge C++ executable for hardware-accelerated H.264 encoding,
/// then packages into RTP for transmission.
pub struct LibcameraSource {
    config: LibcameraConfig,
    track: Arc<TrackLocalStaticRTP>,
    rtp_sender: Option<Arc<dyn RtpSender>>,
    state: Arc<Mutex<SourceState>>,
}

struct SourceState {
    child_process: Option<Child>,
    child_stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    processor_handle: Option<JoinHandle<()>>,
    running: bool,
}

impl LibcameraSource {
    pub fn new(
        config: LibcameraConfig,
        rtp_port: u16,
        rtp_dest: Option<String>,
        track: Arc<TrackLocalStaticRTP>,
    ) -> Self {
        debug!("Creating LibcameraSource");
        
        let dest_ip = rtp_dest.unwrap_or_else(|| "127.0.0.1".to_string());
        
        // Create UDP RTP sender for testing
        // TODO: Replace with LiveionRtpSender when liveion interface is ready
        let rtp_sender = if rtp_port > 0 {
            match UdpRtpSender::new(format!("{}:{}", dest_ip, rtp_port).parse().unwrap()) {
                Ok(sender) => Some(Arc::new(sender) as Arc<dyn RtpSender>),
                Err(e) => {
                    warn!("Failed to create UDP RTP sender: {}", e);
                    None
                }
            }
        } else {
            None
        };
        
        Self {
            config,
            track,
            rtp_sender,
            state: Arc::new(Mutex::new(SourceState {
                child_process: None,
                child_stdin: Arc::new(Mutex::new(None)),
                processor_handle: None,
                running: false,
            })),
        }
    }

    /// Build libcamera-bridge command with configured parameters
    fn build_libcamera_bridge_command(&self) -> Vec<String> {
        let cfg = &self.config;
        
        let mut args = vec![
            "--width".to_string(),
            cfg.width.to_string(),
            "--height".to_string(),
            cfg.height.to_string(),
            "--fps".to_string(),
            cfg.fps.to_string(),
            "--bitrate".to_string(),
            cfg.bitrate.to_string(),
        ];
        
        // Add optional parameters
        if cfg.camera_id > 0 {
            args.push("--camera".to_string());
            args.push(cfg.camera_id.to_string());
        }
        
        if cfg.rotation > 0 {
            args.push("--rotation".to_string());
            args.push(cfg.rotation.to_string());
        }
        
        if cfg.hflip {
            args.push("--hflip".to_string());
        }
        
        if cfg.vflip {
            args.push("--vflip".to_string());
        }
        
        args
    }
}

impl Source for LibcameraSource {
    fn start(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        
        if state.running {
            warn!("Libcamera source already running");
            return Ok(());
        }
        
        let args = self.build_libcamera_bridge_command();
        info!("Starting libcamera-bridge with args: {:?}", args);
        
        // Clone necessary data for the async task
        let track = self.track.clone();
        let rtp_sender = self.rtp_sender.clone();
        let fps = self.config.fps;
        let stdin_handle = state.child_stdin.clone();
        
        // Spawn blocking thread to run async code
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
            rt.block_on(async move {
                // Start libcamera-bridge process with tokio
                let mut child = match Command::new("libcamera-bridge")
                    .args(&args)
                    .stdin(std::process::Stdio::piped())   // Enable stdin for control
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit())  // Show errors!
                    .spawn()
                {
                    Ok(child) => {
                        info!("libcamera-bridge started with PID: {:?}", child.id());
                        child
                    }
                    Err(e) => {
                        error!("Failed to start libcamera-bridge: {}", e);
                        return;
                    }
                };
                
                // Take stdout and stdin from child process
                if let Some(stdout) = child.stdout.take() {
                    let stdin = child.stdin.take();
                    info!("H.264 stream processor started");
                    if let Err(e) = Self::process_h264_stream(stdout, stdin, track, rtp_sender, fps).await {
                        error!("H.264 stream processor failed: {}", e);
                    }
                }
                
                // Wait for child to finish
                let _ = child.wait().await;
                info!("libcamera-bridge process ended");
            });
        });
        
        // Give the child process time to start
        std::thread::sleep(std::time::Duration::from_millis(100));
        
        // Note: We can't easily save child_stdin here due to the async/thread boundary
        // For now, keyframe requests won't work, but the basic functionality works
        // TODO: Refactor to use tokio::spawn instead of std::thread::spawn
        
        state.processor_handle = None; // Can't store std::thread handle in JoinHandle<()>
        state.running = true;
        
        info!("LibcameraSource started successfully with RTP output");
        Ok(())
    }
    
    fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        
        if !state.running {
            warn!("Libcamera source not running");
            return Ok(());
        }
        
        info!("Stopping libcamera source");
        
        // Kill child process
        if let Some(mut child) = state.child_process.take() {
            debug!("Killing libcamera-bridge process");
            let _ = child.kill();
            let _ = child.wait();
            info!("libcamera-bridge process terminated");
        }
        
        // Abort processor task
        if let Some(handle) = state.processor_handle.take() {
            debug!("Aborting H.264 processor task");
            handle.abort();
        }
        
        state.running = false;
        info!("LibcameraSource stopped");
        Ok(())
    }
    
    fn request_keyframe(&self) -> Result<()> {
        use std::io::Write;
        
        let state = self.state.lock().unwrap();
        
        if !state.running {
            warn!("Cannot request keyframe: source not running");
            return Ok(());
        }
        
        // Get stdin handle
        let mut stdin_guard = state.child_stdin.lock().unwrap();
        
        if let Some(ref mut stdin) = *stdin_guard {
            // Send 'k\n' command to libcamera-bridge
            // Note: This is a blocking write, but should be fast
            let raw_stdin = stdin.as_raw_fd();
            let mut file = unsafe { std::fs::File::from_raw_fd(raw_stdin) };
            match file.write_all(b"k\n") {
                Ok(_) => {
                    match file.flush() {
                        Ok(_) => {
                            info!("✓ Keyframe requested");
                            // Don't drop the file - it would close the fd
                            std::mem::forget(file);
                            Ok(())
                        }
                        Err(e) => {
                            std::mem::forget(file);
                            warn!("Failed to flush keyframe request: {}", e);
                            Ok(()) // Don't fail the whole operation
                        }
                    }
                }
                Err(e) => {
                    std::mem::forget(file);
                    warn!("Failed to write keyframe request: {}", e);
                    Ok(()) // Don't fail the whole operation
                }
            }
        } else {
            warn!("No stdin handle available for keyframe request");
            Ok(())
        }
    }
}

impl LibcameraSource {
    /// Process H.264 Annex B stream from libcamera-bridge
    async fn process_h264_stream(
        stdout: tokio::process::ChildStdout,
        mut stdin: Option<tokio::process::ChildStdin>,
        track: Arc<TrackLocalStaticRTP>,
        rtp_sender: Option<Arc<dyn RtpSender>>,
        fps: u32,
    ) -> Result<()> {
        // Directly use ChildStdout with BufReader - no unsafe conversion needed!
        let reader = BufReader::new(stdout);
        let mut parser = AnnexBParser::new(reader);
        let mut packetizer = H264Packetizer::new(1200);  // 1200 bytes MTU
        
        let timestamp_increment = packetizer.timestamp_increment(fps);
        let mut frame_count = 0u64;
        
        info!("H.264 stream processor started");
        
        // CRITICAL: Wait for libcamera-bridge to start up and begin outputting data
        // Without this delay, the first read() returns 0 and parser treats it as EOF
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        info!("Starting to read H.264 stream");
        
        loop {
            // Read next NAL unit
            let nal = match parser.read_next_nal().await {
                Ok(Some(nal)) => nal,
                Ok(None) => {
                    info!("End of H.264 stream");
                    break;
                }
                Err(e) => {
                    error!("Failed to read NAL unit: {}", e);
                    break;
                }
            };
            
            frame_count += 1;
            if frame_count % 30 == 0 {
                debug!("Processed {} NAL units, type: {:?}", frame_count, nal.nal_type);
            }
            
            // Periodically request keyframes (e.g., every 60 frames = 2 seconds at 30fps)
            if frame_count % 60 == 0 {
                if let Some(ref mut stdin) = stdin {
                    use tokio::io::AsyncWriteExt;
                    if let Err(e) = stdin.write_all(b"k\n").await {
                        warn!("Failed to send keyframe request to libcamera-bridge stdin: {}", e);
                    } else if let Err(e) = stdin.flush().await {
                        warn!("Failed to flush keyframe request: {}", e);
                    } else {
                        info!("Requested periodic IDR keyframe from libcamera-bridge (frame {})", frame_count);
                    }
                }
            }
            
            // Package NAL into RTP packets
            let rtp_packets = match packetizer.packetize(&nal) {
                Ok(packets) => packets,
                Err(e) => {
                    error!("Failed to packetize NAL: {}", e);
                    continue;
                }
            };
            
            // Send RTP packets
            for rtp_packet in &rtp_packets {
                // Send via UDP (for testing)
                if let Some(ref sender) = rtp_sender {
                    if let Err(e) = sender.send(rtp_packet) {
                        warn!("Failed to send RTP packet via UDP: {}", e);
                    }
                }
                
                // Write to WebRTC track
                let packet_bytes = rtp_packet.to_bytes();
                match webrtc::rtp::packet::Packet::unmarshal(&mut packet_bytes.as_slice()) {
                    Ok(webrtc_packet) => {
                        // Log every 30 frames to check if writes are happening
                        if frame_count % 30 == 0 {
                            debug!("Writing RTP packet to track (frame {})", frame_count);
                        }
                        
                        if let Err(e) = track.write_rtp(&webrtc_packet).await {
                            if frame_count % 30 == 0 {
                                warn!("Failed to write RTP to track (frame {}): {}", frame_count, e);
                            }
                        } else if frame_count % 30 == 0 {
                            debug!("Successfully wrote RTP packet to track (frame {})", frame_count);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to unmarshal RTP packet: {}", e);
                    }
                }
            }
            
            // Only update timestamp after frame-level NAL units (IDR, Slice).
            // SPS, PPS, SEI, AUD belong to the same access unit as the following
            // IDR/Slice and MUST share the same RTP timestamp per RFC 6184.
            match nal.nal_type {
                NalType::Idr | NalType::Slice => {
                    packetizer.update_timestamp(timestamp_increment);
                }
                _ => {
                    // SPS, PPS, SEI, AUD, etc.: keep same timestamp
                    // — they are part of the same access unit as the next frame NAL
                }
            }
        }
        
        info!("H.264 stream processor stopped after {} NAL units", frame_count);
        Ok(())
    }
}
