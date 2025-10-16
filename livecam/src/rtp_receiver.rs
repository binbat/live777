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

pub async fn start(
    rtp_port: u16,
    track: Arc<TrackLocalStaticRTP>,
    shutdown_rx: mpsc::Receiver<()>,
    _payload_type: u8,
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
    _rtp_port: u16,
    track: Arc<TrackLocalStaticRTP>,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    use crate::ffi::TDL_RTSP_Params;
    use crate::stream::StreamHandle;
    use std::ffi::CString;
    use std::sync::Mutex;

    const POLL_INTERVAL: Duration = Duration::from_millis(33);
    const POLL_TIMEOUT_MS: u32 = 100;
    const RTP_MTU: usize = 1200;
    const H264_PAYLOAD_TYPE: u8 = 96;

    let stream_handle = {
        let codec_cstring = CString::new("h264").unwrap();
        let params = TDL_RTSP_Params {
            rtsp_port: 0,
            enc_width: 1280,
            enc_height: 720,
            framerate: 30,
            vb_blk_count: 8,
            vb_bind: 0,
            codec: codec_cstring.as_ptr(),
            ring_capacity: 32,
        };

        let handle = StreamHandle::start_encode_only(&params)
            .map_err(|e| anyhow!("Failed to start stream encoding: {}", e))?;
        Arc::new(Mutex::new(handle))
    };

    let mut sequence_number: u16 = rand::random();
    let ssrc: u32 = rand::random();
    let mut payloader = H264Payloader::default();

    info!("stream receiver started.");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("stream receiver shutting down.");
                let handle = stream_handle.lock().unwrap();
                handle.stop();
                break;
            }
            _ = sleep(POLL_INTERVAL) => {
                let frame_result = {
                    let handle = stream_handle.lock().unwrap();
                    handle.get_encoded_frame(POLL_TIMEOUT_MS as i32)
                };

                match frame_result {
                    Ok(Some((frame, pts, _is_key))) => {
                        if let Err(e) = send_rtp(
                            &track,
                            &frame,
                            &mut sequence_number,
                            pts as u32,
                            ssrc,
                            &mut payloader,
                        ).await {
                            error!("Failed to send frame as RTP: {}", e);
                        }
                    }
                    Ok(None) => {
                        continue;
                    }
                    Err(e) => {
                        error!("Failed to get frame from device: {}", e);
                        if e.contains("Handle stopped") {
                            break;
                        }
                        sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }

    info!("stream receiver stopped.");
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
            trace!("Packaged frame into {} RTP packets", num_payloads);
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
    info!(port = rtp_port, "RTP receiver listening on UDP.");
    let mut buffer = [0u8; 2048];
    let mut packet_count = 0u64;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(port = rtp_port, "RTP receiver shutting down.");
                break;
            }
            result = socket.recv_from(&mut buffer) => {
                match result {
                    Ok((size, _)) => {
                        packet_count += 1;
                        if packet_count.is_multiple_of(100) {
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
