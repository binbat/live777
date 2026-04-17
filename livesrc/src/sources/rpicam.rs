use anyhow::Result;
use std::sync::{Arc, Mutex};
use tokio::io::BufReader;
use tokio::process::Command;
use tracing::{debug, error, info, warn};
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocalWriter;
use webrtc::util::Unmarshal;

use crate::config::LibcameraConfig;
use crate::rtp::{AnnexBParser, H264Packetizer, NalType, RtpSender, UdpRtpSender};

use super::Source;

/// RpicamSource: uses `rpicam-vid` (the official Raspberry Pi tool) to capture
/// and hardware-encode H.264 in one step. Much simpler than the custom
/// libcamera-bridge approach — no C++ code needed.
///
/// Pipeline:
///   rpicam-vid --codec h264 --inline -t 0 -o - (stdout)
///       → AnnexBParser → H264Packetizer → RTP → UDP
pub struct RpicamSource {
    config: LibcameraConfig,
    track: Arc<TrackLocalStaticRTP>,
    rtp_sender: Option<Arc<dyn RtpSender>>,
    state: Arc<Mutex<RpicamState>>,
}

struct RpicamState {
    running: bool,
}

impl RpicamSource {
    pub fn new(
        config: LibcameraConfig,
        rtp_port: u16,
        rtp_dest: Option<String>,
        track: Arc<TrackLocalStaticRTP>,
    ) -> Self {
        debug!("Creating RpicamSource (rpicam-vid backend)");

        let dest_ip = rtp_dest.unwrap_or_else(|| "127.0.0.1".to_string());

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
            state: Arc::new(Mutex::new(RpicamState { running: false })),
        }
    }

    /// Build the rpicam-vid command line arguments.
    fn build_command_args(&self) -> Vec<String> {
        let cfg = &self.config;

        let mut args = vec![
            // Output H.264 Annex-B to stdout
            "--codec".to_string(),
            "h264".to_string(),
            // Inline SPS/PPS headers (critical for stream joining)
            "--inline".to_string(),
            // Run forever
            "-t".to_string(),
            "0".to_string(),
            // Output to stdout
            "-o".to_string(),
            "-".to_string(),
            // Resolution
            "--width".to_string(),
            cfg.width.to_string(),
            "--height".to_string(),
            cfg.height.to_string(),
            // Frame rate
            "--framerate".to_string(),
            cfg.fps.to_string(),
            // Bitrate (rpicam-vid uses bits per second)
            "--bitrate".to_string(),
            cfg.bitrate.to_string(),
            // H.264 profile: baseline for lowest latency
            "--profile".to_string(),
            "baseline".to_string(),
            // H.264 level
            "--level".to_string(),
            "4".to_string(),
            // Intra-refresh period (GOP size): one IDR every N frames
            // 30 frames = 1 IDR per second at 30fps
            "--intra".to_string(),
            "30".to_string(),
            // Flush output after every frame for lowest latency
            "--flush".to_string(),
            // Disable preview window (headless)
            "-n".to_string(),
        ];

        // Camera selection
        if cfg.camera_id > 0 {
            args.push("--camera".to_string());
            args.push(cfg.camera_id.to_string());
        }

        // Rotation
        if cfg.rotation > 0 {
            args.push("--rotation".to_string());
            args.push(cfg.rotation.to_string());
        }

        // Flip
        if cfg.hflip {
            args.push("--hflip".to_string());
        }
        if cfg.vflip {
            args.push("--vflip".to_string());
        }

        args
    }

    /// Process the H.264 Annex-B stream from rpicam-vid stdout.
    /// This is the core loop: read NALs → packetize → send RTP.
    async fn process_h264_stream(
        stdout: tokio::process::ChildStdout,
        track: Arc<TrackLocalStaticRTP>,
        rtp_sender: Option<Arc<dyn RtpSender>>,
        fps: u32,
    ) -> Result<()> {
        let reader = BufReader::new(stdout);
        let mut parser = AnnexBParser::new(reader);
        let mut packetizer = H264Packetizer::new(1200); // 1200 bytes MTU

        let timestamp_increment = packetizer.timestamp_increment(fps);
        let mut frame_count = 0u64;

        info!("[rpicam] H.264 stream processor started");

        // Give rpicam-vid a moment to initialize the camera hardware
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        info!("[rpicam] Starting to read H.264 stream from rpicam-vid stdout");

        loop {
            // Read next NAL unit from Annex-B stream
            let nal = match parser.read_next_nal().await {
                Ok(Some(nal)) => nal,
                Ok(None) => {
                    info!("[rpicam] End of H.264 stream (rpicam-vid exited?)");
                    break;
                }
                Err(e) => {
                    error!("[rpicam] Failed to read NAL unit: {}", e);
                    break;
                }
            };

            frame_count += 1;
            if frame_count % 30 == 0 {
                debug!(
                    "[rpicam] Processed {} NAL units, latest type: {:?}",
                    frame_count, nal.nal_type
                );
            }

            // Packetize NAL into RTP packets
            let rtp_packets = match packetizer.packetize(&nal) {
                Ok(packets) => packets,
                Err(e) => {
                    error!("[rpicam] Failed to packetize NAL: {}", e);
                    continue;
                }
            };

            // Send each RTP packet
            for rtp_packet in &rtp_packets {
                // Send via UDP to liveion
                if let Some(ref sender) = rtp_sender {
                    if let Err(e) = sender.send(rtp_packet) {
                        warn!("[rpicam] Failed to send RTP packet via UDP: {}", e);
                    }
                }

                // Also write to WebRTC track (for direct WHEP subscribers)
                let packet_bytes = rtp_packet.to_bytes();
                match webrtc::rtp::packet::Packet::unmarshal(&mut packet_bytes.as_slice()) {
                    Ok(webrtc_packet) => {
                        if let Err(e) = track.write_rtp(&webrtc_packet).await {
                            if frame_count % 30 == 0 {
                                warn!(
                                    "[rpicam] Failed to write RTP to track (frame {}): {}",
                                    frame_count, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!("[rpicam] Failed to unmarshal RTP packet: {}", e);
                    }
                }
            }

            // Only advance timestamp after VCL NAL units (IDR, Slice).
            // SPS/PPS/SEI belong to the same access unit → same timestamp.
            match nal.nal_type {
                NalType::Idr | NalType::Slice => {
                    packetizer.update_timestamp(timestamp_increment);
                }
                _ => {
                    // SPS, PPS, SEI, AUD: keep same timestamp
                }
            }
        }

        info!(
            "[rpicam] H.264 stream processor stopped after {} NAL units",
            frame_count
        );
        Ok(())
    }
}

impl Source for RpicamSource {
    fn start(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        if state.running {
            warn!("[rpicam] Source already running");
            return Ok(());
        }

        let args = self.build_command_args();
        info!("[rpicam] Starting rpicam-vid with args: {:?}", args);

        let track = self.track.clone();
        let rtp_sender = self.rtp_sender.clone();
        let fps = self.config.fps;

        // Spawn a dedicated thread with its own tokio runtime
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
            rt.block_on(async move {
                let mut child = match Command::new("rpicam-vid")
                    .args(&args)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit())
                    .spawn()
                {
                    Ok(child) => {
                        info!("[rpicam] rpicam-vid started with PID: {:?}", child.id());
                        child
                    }
                    Err(e) => {
                        error!("[rpicam] Failed to start rpicam-vid: {}", e);
                        error!("[rpicam] Make sure rpicam-vid is installed (sudo apt install rpicam-apps)");
                        return;
                    }
                };

                if let Some(stdout) = child.stdout.take() {
                    if let Err(e) =
                        Self::process_h264_stream(stdout, track, rtp_sender, fps).await
                    {
                        error!("[rpicam] H.264 stream processor failed: {}", e);
                    }
                }

                let _ = child.wait().await;
                info!("[rpicam] rpicam-vid process ended");
            });
        });

        std::thread::sleep(std::time::Duration::from_millis(100));
        state.running = true;

        info!("[rpicam] RpicamSource started successfully");
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        if !state.running {
            warn!("[rpicam] Source not running");
            return Ok(());
        }

        info!("[rpicam] Stopping rpicam source");
        // rpicam-vid will be killed when the thread/runtime is dropped
        state.running = false;
        info!("[rpicam] RpicamSource stopped");
        Ok(())
    }

    fn request_keyframe(&self) -> Result<()> {
        // rpicam-vid does not support on-demand keyframe requests via stdin.
        // Keyframes are generated periodically based on the --intra parameter.
        debug!("[rpicam] Keyframe request ignored (rpicam-vid uses fixed --intra interval)");
        Ok(())
    }
}
