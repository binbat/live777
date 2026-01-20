#[cfg(riscv_mode)]
use anyhow::anyhow;
#[cfg(riscv_mode)]
use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;
#[cfg(not(riscv_mode))]
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{error, info, trace, warn};
use webrtc::rtp::packet::Packet;
#[cfg(riscv_mode)]
use webrtc::rtp::{codecs::h264::H264Payloader, packetizer::Payloader};
use webrtc::track::track_local::{TrackLocalWriter, track_local_static_rtp::TrackLocalStaticRTP};
#[cfg(not(riscv_mode))]
use webrtc::util::Unmarshal;

#[cfg(riscv_mode)]
const DEFAULT_WIDTH: u32 = 1280;
#[cfg(riscv_mode)]
const DEFAULT_HEIGHT: u32 = 720;
#[cfg(riscv_mode)]
const DEFAULT_FPS: u32 = 30;
#[cfg(riscv_mode)]
const DEFAULT_RTSP_PORT: u16 = 8554;
#[cfg(riscv_mode)]
const DEFAULT_PROTOCOL: &str = "rtp";

pub async fn start(
    rtp_port: u16,
    track: Arc<TrackLocalStaticRTP>,
    shutdown_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    #[cfg(riscv_mode)]
    {
        riscv_mode(rtp_port, track, shutdown_rx).await
    }
    #[cfg(not(riscv_mode))]
    {
        normal_mode(rtp_port, track, shutdown_rx).await
    }
}

#[cfg(riscv_mode)]
async fn riscv_mode(
    rtp_port: u16,
    track: Arc<TrackLocalStaticRTP>,
    shutdown_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    info!("RISCV mode initialized with protocol: {}", DEFAULT_PROTOCOL);

    match DEFAULT_PROTOCOL {
        "rtsp" => rtsp_mode(rtp_port, track, shutdown_rx).await,
        "rtp" => rtp_encode_mode(rtp_port, track, shutdown_rx).await,
        _ => Err(anyhow!("Unknown protocol: {}", DEFAULT_PROTOCOL)),
    }
}
#[cfg(riscv_mode)]
async fn rtsp_mode(
    rtp_port: u16,
    track: Arc<TrackLocalStaticRTP>,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    use milkv_libs::rtsp::{RtspParams, RtspServer};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::UdpSocket;
    use tokio::process::Command;
    use webrtc::util::Unmarshal;

    info!("=== RTSP Mode (Internal Server) Starting ===");
    info!(
        "RTSP server config: {}x{} @ {} fps, port {}",
        DEFAULT_WIDTH, DEFAULT_HEIGHT, DEFAULT_FPS, DEFAULT_RTSP_PORT
    );

    let params = RtspParams::new()
        .port(DEFAULT_RTSP_PORT)
        .resolution(DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .framerate(DEFAULT_FPS)
        .codec("h264")
        .vb_blocks(32)
        .vb_bind(true);

    info!(
        "Starting internal RTSP server on port {}...",
        DEFAULT_RTSP_PORT
    );

    let rtsp_server =
        RtspServer::start(params).map_err(|e| anyhow!("Failed to start RTSP server: {}", e))?;

    info!("âœ?Internal RTSP server created");

    if !rtsp_server.wait_running(5000) {
        let err = rtsp_server.last_error();
        return Err(anyhow!("RTSP server failed to start: {}", err));
    }

    let rtsp_url = format!("rtsp://127.0.0.1:{}/h264", DEFAULT_RTSP_PORT);
    info!("âœ?RTSP server running at: {}", rtsp_url);

    info!("Starting ffmpeg client to pull RTSP stream...");

    let mut ffmpeg_child = Command::new("ffmpeg")
        .args(&[
            "-hide_banner",
            "-loglevel",
            "warning",
            "-rtsp_transport",
            "tcp",
            "-use_wallclock_as_timestamps",
            "1",
            "-i",
            &rtsp_url,
            "-c:v",
            "copy",
            "-an",
            "-f",
            "rtp",
            "-payload_type",
            "96",
            &format!("rtp://127.0.0.1:{}?pkt_size=1200", rtp_port),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("Failed to start ffmpeg: {}", e))?;

    info!(
        "âœ?FFmpeg client started (PID: {})",
        ffmpeg_child.id().unwrap_or(0)
    );

    let stderr = ffmpeg_child.stderr.take().unwrap();
    let stderr_reader = BufReader::new(stderr);
    let mut stderr_lines = stderr_reader.lines();

    let ffmpeg_monitor = tokio::spawn(async move {
        while let Ok(Some(line)) = stderr_lines.next_line().await {
            if !line.is_empty() {
                if line.contains("error") || line.contains("Error") {
                    error!("FFmpeg: {}", line);
                } else {
                    warn!("FFmpeg: {}", line);
                }
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(1500)).await;
    let socket = UdpSocket::bind(format!("127.0.0.1:{}", rtp_port))
        .await
        .map_err(|e| anyhow!("Failed to bind UDP on port {}: {}", rtp_port, e))?;

    info!("âœ?RTP receiver listening on 127.0.0.1:{}", rtp_port);
    info!("=== RTSP Mode Running ===");

    let mut buffer = [0u8; 4096];
    let mut packet_count = 0u64;
    let mut last_packet_time = tokio::time::Instant::now();
    const TIMEOUT_DURATION: Duration = Duration::from_secs(10);

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Received shutdown signal");
                break;
            }
            result = socket.recv_from(&mut buffer) => {
                match result {
                    Ok((size, _)) => {
                        packet_count += 1;
                        last_packet_time = tokio::time::Instant::now();

                        if packet_count % 300 == 0 {
                            trace!("Processed {} RTP packets from RTSP stream", packet_count);
                        }

                        match Packet::unmarshal(&mut &buffer[..size]) {
                            Ok(rtp_packet) => {
                                if let Err(e) = track.write_rtp(&rtp_packet).await {
                                    error!("Failed to write RTP packet: {}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Failed to unmarshal RTP packet (size={}): {}", size, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("UDP recv error: {}", e);

                        match ffmpeg_child.try_wait() {
                            Ok(Some(status)) => {
                                error!("FFmpeg exited with status: {:?}", status);
                                break;
                            }
                            Ok(None) => {
                                sleep(Duration::from_millis(100)).await;
                            }
                            Err(e) => {
                                error!("Failed to check ffmpeg status: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
            _ = sleep(Duration::from_secs(2)) => {

                if last_packet_time.elapsed() > TIMEOUT_DURATION {
                    error!("No RTP packets received for {} seconds", TIMEOUT_DURATION.as_secs());

                    if !rtsp_server.is_running() {
                        error!("RTSP server stopped: {}", rtsp_server.last_error());
                        break;
                    }

                    match ffmpeg_child.try_wait() {
                        Ok(Some(status)) => {
                            error!("FFmpeg exited: {:?}", status);
                            break;
                        }
                        Ok(None) => {
                            warn!("FFmpeg still running but no data received");
                        }
                        Err(e) => {
                            error!("Failed to check ffmpeg: {}", e);
                            break;
                        }
                    }
                }
            }
        }
    }

    info!("Shutting down RTSP mode...");

    let _ = ffmpeg_child.kill().await;
    let _ = ffmpeg_child.wait().await;
    ffmpeg_monitor.abort();

    if let Err(e) = rtsp_server.stop() {
        warn!("Failed to stop RTSP server gracefully: {}", e);
    }

    info!(
        "=== RTSP Mode Stopped (processed {} packets) ===",
        packet_count
    );
    Ok(())
}
#[cfg(riscv_mode)]
async fn rtp_encode_mode(
    _rtp_port: u16,
    track: Arc<TrackLocalStaticRTP>,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    use milkv_libs::{TDL_RTSP_Params, stream::StreamHandle};
    use std::ffi::CString;
    use std::sync::Mutex;

    const POLL_INTERVAL: Duration = Duration::from_millis(33);
    const POLL_TIMEOUT_MS: i32 = 100;

    info!("Starting RTP encode mode (direct encoding)");
    info!(
        "RTP encode config: {}x{} @ {} fps",
        DEFAULT_WIDTH, DEFAULT_HEIGHT, DEFAULT_FPS
    );

    let stream_handle = {
        let codec_cstring = CString::new("h264").unwrap();
        let params = TDL_RTSP_Params {
            rtsp_port: 0,
            enc_width: DEFAULT_WIDTH,
            enc_height: DEFAULT_HEIGHT,
            framerate: DEFAULT_FPS,
            vb_blk_count: 8,
            vb_bind: 0,
            codec: codec_cstring.as_ptr(),
            ring_capacity: 64,
        };

        let handle = StreamHandle::start_encode_only(&params)
            .map_err(|e| anyhow!("Failed to start encoding: {}", e))?;
        Arc::new(Mutex::new(handle))
    };

    let mut sequence_number: u16 = rand::random();
    let ssrc: u32 = rand::random();
    let mut payloader = H264Payloader::default();

    info!("âœ?RTP encode mode started (SSRC: 0x{:08X})", ssrc);

    let mut frame_count = 0u64;
    let mut keyframe_count = 0u64;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("RTP encode mode shutting down");
                let handle = stream_handle.lock().unwrap();
                handle.stop();
                break;
            }
            _ = sleep(POLL_INTERVAL) => {

                let frame_result = {
                    let handle = stream_handle.lock().unwrap();
                    handle.get_encoded_frame(POLL_TIMEOUT_MS)
                };

                match frame_result {
                    Ok(Some((frame, pts, is_key))) => {
                        frame_count += 1;
                        if is_key {
                            keyframe_count += 1;
                            trace!("Got keyframe #{}: size={}, pts={}", keyframe_count, frame.len(), pts);
                        }

                        if frame_count % 300 == 0 {
                            info!("Encoded {} frames ({} keyframes)", frame_count, keyframe_count);
                        }

                        if let Err(e) = send_rtp(
                            &track,
                            &frame,
                            &mut sequence_number,
                            pts as u32,
                            ssrc,
                            &mut payloader,
                        )
                        .await
                        {
                            error!("Failed to send RTP: {}", e);
                            break;
                        }
                    }
                    Ok(None) => {

                        continue;
                    }
                    Err(e) => {
                        error!("Failed to get frame from encoder: {}", e);
                        if e.contains("Handle stopped") || e.contains("invalid state") {
                            break;
                        }
                        sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }

    info!(
        "RTP encode mode stopped (total frames: {}, keyframes: {})",
        frame_count, keyframe_count
    );
    Ok(())
}

#[cfg(riscv_mode)]
async fn send_rtp(
    track: &Arc<TrackLocalStaticRTP>,
    h264_data: &[u8],
    sequence_number: &mut u16,
    timestamp: u32,
    ssrc: u32,
    payloader: &mut H264Payloader,
) -> anyhow::Result<()> {
    const RTP_MTU: usize = 1200;
    const H264_PAYLOAD_TYPE: u8 = 96;

    let frame_bytes = Bytes::from(h264_data.to_vec());
    match payloader.payload(RTP_MTU, &frame_bytes) {
        Ok(payloads) => {
            let num_payloads = payloads.len();
            for (i, payload) in payloads.into_iter().enumerate() {
                let packet = Packet {
                    header: webrtc::rtp::header::Header {
                        version: 2,
                        padding: false,
                        extension: false,
                        marker: i == num_payloads - 1,
                        payload_type: H264_PAYLOAD_TYPE,
                        sequence_number: *sequence_number,
                        timestamp,
                        ssrc,
                        csrc: vec![],
                        ..Default::default()
                    },
                    payload,
                };
                track.write_rtp(&packet).await?;
                *sequence_number = sequence_number.wrapping_add(1);
            }
            Ok(())
        }
        Err(e) => Err(anyhow!("Failed to payload: {}", e)),
    }
}

#[cfg(not(riscv_mode))]
async fn normal_mode(
    rtp_port: u16,
    track: Arc<TrackLocalStaticRTP>,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(format!("0.0.0.0:{}", rtp_port)).await?;
    info!(port = rtp_port, "RTP receiver listening on UDP");

    let mut buffer = [0u8; 2048];
    let mut packet_count = 0u64;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(port = rtp_port, "RTP receiver shutting down");
                break;
            }
            result = socket.recv_from(&mut buffer) => {
                match result {
                    Ok((size, _)) => {
                        packet_count += 1;
                        if packet_count.is_multiple_of(1000){
                            trace!("Processed {} RTP packets", packet_count);
                        }

                        match Packet::unmarshal(&mut &buffer[..size]) {
                            Ok(rtp_packet) => {
                                if let Err(e) = track.write_rtp(&rtp_packet).await {
                                    error!("Failed to write RTP packet: {}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!("Failed to unmarshal RTP packet (size={}): {}", size, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("UDP recv error: {}", e);
                        sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }

    Ok(())
}
