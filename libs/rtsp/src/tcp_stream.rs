use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::constants::buffer;
use crate::types::SessionMode;

pub type InterleavedFrame = (u8, Vec<u8>);

#[async_trait::async_trait]
pub(crate) trait InterleavedSender: Send + Sync {
    async fn send_interleaved(
        &self,
        data: InterleavedFrame,
    ) -> Result<(), SendError<InterleavedFrame>>;
}

#[async_trait::async_trait]
impl InterleavedSender for Sender<InterleavedFrame> {
    async fn send_interleaved(
        &self,
        data: InterleavedFrame,
    ) -> Result<(), SendError<InterleavedFrame>> {
        self.send(data).await
    }
}

#[async_trait::async_trait]
impl InterleavedSender for UnboundedSender<InterleavedFrame> {
    async fn send_interleaved(
        &self,
        data: InterleavedFrame,
    ) -> Result<(), SendError<InterleavedFrame>> {
        self.send(data)
    }
}

#[async_trait::async_trait]
pub(crate) trait InterleavedReceiver: Send {
    async fn recv_interleaved(&mut self) -> Option<InterleavedFrame>;
}

#[async_trait::async_trait]
impl InterleavedReceiver for Receiver<InterleavedFrame> {
    async fn recv_interleaved(&mut self) -> Option<InterleavedFrame> {
        self.recv().await
    }
}

#[async_trait::async_trait]
impl InterleavedReceiver for UnboundedReceiver<InterleavedFrame> {
    async fn recv_interleaved(&mut self) -> Option<InterleavedFrame> {
        self.recv().await
    }
}

pub(crate) async fn handle_tcp_stream<S, R>(
    stream: TcpStream,
    mode: SessionMode,
    data_from_stream_tx: S,
    data_to_stream_rx: R,
    cancel: CancellationToken,
    // When true, drop incoming even-channel (RTP) frames. This is used on the
    // server side in Pull mode where the client should only send RTCP on odd
    // channels, so unexpected RTP echo must not fill the bounded RTCP channel.
    drop_incoming_even: bool,
) -> Result<()>
where
    S: InterleavedSender + 'static,
    R: InterleavedReceiver + 'static,
{
    let (read_half, write_half) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(write_half));

    info!("Starting TCP interleaved stream handler (mode: {:?})", mode);

    let read_task = tokio::spawn(handle_read_stream(
        read_half,
        writer.clone(),
        data_from_stream_tx,
        cancel.clone(),
        drop_incoming_even,
    ));

    let write_task = tokio::spawn(handle_write_stream(writer, data_to_stream_rx));

    let (read_result, write_result) = tokio::join!(read_task, write_task);

    match read_result {
        Ok(Ok(())) => debug!("Read task completed successfully"),
        Ok(Err(e)) => error!("Read task failed: {}", e),
        Err(e) => error!("Read task panicked: {}", e),
    }

    match write_result {
        Ok(Ok(())) => debug!("Write task completed successfully"),
        Ok(Err(e)) => error!("Write task failed: {}", e),
        Err(e) => error!("Write task panicked: {}", e),
    }

    info!("TCP interleaved stream handler stopped");
    Ok(())
}

async fn handle_read_stream<R, S>(
    mut reader: R,
    writer: Arc<Mutex<WriteHalf<TcpStream>>>,
    tx: S,
    cancel: CancellationToken,
    drop_incoming_even: bool,
) -> Result<()>
where
    R: AsyncReadExt + Unpin,
    S: InterleavedSender,
{
    let mut buffer = Vec::with_capacity(buffer::TCP_READ_BUFFER_SIZE);
    let mut temp_buffer = vec![0u8; buffer::TCP_READ_BUFFER_SIZE];
    let mut total_frames = 0u64;
    let mut total_bytes = 0u64;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("TCP stream handler cancelled");
                break;
            }
            result = reader.read(&mut temp_buffer) => {
                match result {
                    Ok(0) => {
                        info!(
                            "TCP stream closed by peer (processed {} frames, {} bytes)",
                            total_frames, total_bytes
                        );
                        break;
                    }
                    Ok(n) => {
                        trace!("Read {} bytes from TCP stream", n);

                        if buffer.len() + n > buffer::MAX_BUFFER_SIZE {
                            return Err(anyhow!(
                                "TCP read buffer limit exceeded ({} + {} > {}); closing connection",
                                buffer.len(),
                                n,
                                buffer::MAX_BUFFER_SIZE
                            ));
                        }

                        buffer.extend_from_slice(&temp_buffer[..n]);
                        total_bytes += n as u64;

                        while let Some((channel, data, consumed)) = parse_interleaved_frame(&buffer)? {
                            trace!(
                                "Parsed interleaved frame: channel={}, length={}, consumed={}",
                                channel,
                                data.len(),
                                consumed
                            );

                            // When acting as a Pull server, the client should only send
                            // RTCP on odd channels. Drop unexpected even-channel (RTP)
                            // frames before they can fill the bounded RTCP channel and
                            // stall the session.
                            if drop_incoming_even && channel % 2 == 0 {
                                trace!("Dropping unexpected incoming RTP frame on channel {}", channel);
                                buffer.drain(..consumed);
                                continue;
                            }

                            if let Err(e) = tx.send_interleaved((channel, data)).await {
                                error!("Failed to forward data from stream: {}", e);
                                return Err(e.into());
                            }

                            total_frames += 1;

                            buffer.drain(..consumed);
                        }

                        if !buffer.is_empty() && buffer[0] != b'$' {
                            match try_parse_rtsp_message(&buffer)? {
                                Some((consumed, true, _)) => {
                                    info!("Received TEARDOWN in data stream, closing session");
                                    buffer.drain(..consumed);
                                    break;
                                }
                                Some((consumed, false, Some(cseq))) => {
                                    debug!(
                                        "Responding to keep-alive RTSP message in data stream ({} bytes)",
                                        consumed
                                    );
                                    buffer.drain(..consumed);
                                    let response = build_keep_alive_response(cseq);
                                    let mut writer = writer.lock().await;
                                    if let Err(e) = writer.write_all(&response).await {
                                        error!("TCP keep-alive write error: {}", e);
                                        return Err(e.into());
                                    }
                                    if let Err(e) = writer.flush().await {
                                        error!("TCP keep-alive flush error: {}", e);
                                        return Err(e.into());
                                    }
                                }
                                Some((consumed, false, None)) => {
                                    debug!(
                                        "Skipping non-keep-alive RTSP message in data stream ({} bytes)",
                                        consumed
                                    );
                                    buffer.drain(..consumed);
                                }
                                None => {}
                            }
                        }

                        if buffer.len() > buffer::MAX_BUFFER_SIZE / 2 {
                            warn!(
                                "Buffer size is large: {} bytes (may indicate slow consumer)",
                                buffer.len()
                            );
                        }
                    }
                    Err(e) => {
                        error!("TCP read error: {}", e);
                        return Err(e.into());
                    }
                }
            }
        }
    }

    info!(
        "Read task completed: {} frames, {} bytes total",
        total_frames, total_bytes
    );
    Ok(())
}

fn parse_interleaved_frame(buffer: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>> {
    if buffer.len() < buffer::INTERLEAVED_HEADER_SIZE {
        return Ok(None);
    }

    if buffer[0] != b'$' {
        return Ok(None);
    }

    let channel = buffer[1];
    // The interleaved header length field is 16-bit, so `length` is naturally
    // bounded by `u16::MAX` which equals `MAX_FRAME_SIZE`.
    let length = u16::from_be_bytes([buffer[2], buffer[3]]) as usize;

    let total_size = buffer::INTERLEAVED_HEADER_SIZE + length;
    if buffer.len() < total_size {
        trace!(
            "Incomplete frame: have {} bytes, need {} bytes",
            buffer.len(),
            total_size
        );
        return Ok(None);
    }

    let data = buffer[buffer::INTERLEAVED_HEADER_SIZE..total_size].to_vec();

    Ok(Some((channel, data, total_size)))
}

/// Parse an RTSP message from the buffer. Returns `Ok(Some((consumed, is_teardown, cseq)))`
/// where `is_teardown` is true only for TEARDOWN requests and `cseq` is the request CSeq
/// when the message is an OPTIONS or GET_PARAMETER keep-alive request.
fn try_parse_rtsp_message(buffer: &[u8]) -> Result<Option<(usize, bool, Option<u32>)>> {
    match rtsp_types::Message::<Vec<u8>>::parse(buffer) {
        Ok((msg, consumed)) => {
            if let rtsp_types::Message::Request(req) = msg {
                if matches!(req.method(), rtsp_types::Method::Teardown) {
                    return Ok(Some((consumed, true, None)));
                }
                let is_keep_alive = matches!(
                    req.method(),
                    rtsp_types::Method::Options | rtsp_types::Method::GetParameter
                );
                let cseq = if is_keep_alive {
                    req.header(&rtsp_types::headers::CSEQ)
                        .and_then(|h| h.as_str().parse().ok())
                } else {
                    None
                };
                return Ok(Some((consumed, false, cseq)));
            }

            Ok(Some((consumed, false, None)))
        }
        Err(rtsp_types::ParseError::Incomplete(_)) => Ok(None),
        Err(e) => {
            // Parse failed — the buffer starts with non-'$' bytes but is not a
            // valid RTSP message. Scan forward to the next '$' (interleaved
            // frame) or "RTSP/" (next RTSP message) so we don't desync the
            // interleaved parser.
            let skip = buffer
                .iter()
                .position(|&b| b == b'$')
                .or_else(|| buffer.windows(5).position(|w| w == b"RTSP/"))
                .unwrap_or(buffer.len());
            let skip = if skip == 0 { 1 } else { skip };
            warn!(
                "Failed to parse RTSP message: {:?}, skipping {} bytes to next sync point",
                e, skip
            );
            Ok(Some((skip, false, None)))
        }
    }
}

fn build_keep_alive_response(cseq: u32) -> Vec<u8> {
    format!(
        "RTSP/1.0 200 OK\r\nCSeq: {}\r\nPublic: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, ANNOUNCE, RECORD, GET_PARAMETER\r\nContent-Length: 0\r\n\r\n",
        cseq
    )
    .into_bytes()
}

async fn handle_write_stream<R>(writer: Arc<Mutex<WriteHalf<TcpStream>>>, mut rx: R) -> Result<()>
where
    R: InterleavedReceiver,
{
    let mut total_frames = 0u64;
    let mut total_bytes = 0u64;

    while let Some((channel, data)) = rx.recv_interleaved().await {
        if data.len() > buffer::MAX_FRAME_SIZE {
            error!(
                "Frame too large to send: {} bytes (max: {}), dropping",
                data.len(),
                buffer::MAX_FRAME_SIZE
            );
            continue;
        }

        let frame = build_interleaved_frame(channel, &data)?;

        trace!(
            "Sending interleaved frame: channel={}, length={}",
            channel,
            data.len()
        );

        let mut writer = writer.lock().await;
        if let Err(e) = writer.write_all(&frame).await {
            error!("TCP write error: {}", e);
            return Err(e.into());
        }

        if let Err(e) = writer.flush().await {
            error!("TCP flush error: {}", e);
            return Err(e.into());
        }

        total_frames += 1;
        total_bytes += frame.len() as u64;

        if total_frames.is_multiple_of(1000) {
            debug!("Sent {} frames, {} bytes total", total_frames, total_bytes);
        }
    }

    info!(
        "Write task completed: {} frames, {} bytes total",
        total_frames, total_bytes
    );
    Ok(())
}

fn build_interleaved_frame(channel: u8, data: &[u8]) -> Result<Vec<u8>> {
    let length = data.len();

    if length > u16::MAX as usize {
        return Err(anyhow::anyhow!(
            "Data too large for interleaved frame: {} bytes",
            length
        ));
    }

    let mut frame = Vec::with_capacity(buffer::INTERLEAVED_HEADER_SIZE + length);
    frame.push(b'$');
    frame.push(channel);
    frame.extend_from_slice(&(length as u16).to_be_bytes());
    frame.extend_from_slice(data);

    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_complete_frame() {
        let data = vec![
            b'$', 0, 0, 4, // Channel 0, length 4
            1, 2, 3, 4, // Data
        ];

        let result = parse_interleaved_frame(&data).unwrap();
        assert!(result.is_some());

        let (channel, frame_data, consumed) = result.unwrap();
        assert_eq!(channel, 0);
        assert_eq!(frame_data, vec![1, 2, 3, 4]);
        assert_eq!(consumed, 8);
    }

    #[test]
    fn test_parse_incomplete_frame() {
        let data = vec![
            b'$', 0, 0, 4, // Channel 0, length 4
            1, 2, // Only 2 bytes of data
        ];

        let result = parse_interleaved_frame(&data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_max_length_frame_header() {
        // The interleaved length field is 16-bit; verify a frame header at the
        // upper boundary is recognized as incomplete when payload is missing.
        let length = buffer::MAX_FRAME_SIZE as u16;
        let data = vec![b'$', 0, (length >> 8) as u8, (length & 0xFF) as u8];

        let result = parse_interleaved_frame(&data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_build_interleaved_frame() {
        let data = vec![1, 2, 3, 4];
        let frame = build_interleaved_frame(0, &data).unwrap();

        assert_eq!(frame.len(), 8);
        assert_eq!(frame[0], b'$');
        assert_eq!(frame[1], 0);
        assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 4);
        assert_eq!(&frame[4..], &data);
    }

    #[test]
    fn test_build_frame_too_large() {
        let data = vec![0u8; (u16::MAX as usize) + 1];
        let result = build_interleaved_frame(0, &data);
        assert!(result.is_err());
    }
}
