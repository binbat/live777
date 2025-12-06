use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace, warn};

use crate::types::SessionMode;

const MAX_BUFFER_SIZE: usize = 2 * 1024 * 1024;
const READ_BUFFER_SIZE: usize = 64 * 1024;
const INTERLEAVED_HEADER_SIZE: usize = 4;
const MAX_FRAME_SIZE: usize = 1024 * 1024;

pub async fn handle_tcp_stream(
    stream: TcpStream,
    mode: SessionMode,
    data_from_stream_tx: UnboundedSender<(u8, Vec<u8>)>,
    data_to_stream_rx: UnboundedReceiver<(u8, Vec<u8>)>,
) -> Result<()> {
    let (read_half, write_half) = tokio::io::split(stream);

    info!("Starting TCP interleaved stream handler (mode: {:?})", mode);

    let read_task = tokio::spawn(handle_read_stream(read_half, data_from_stream_tx));

    let write_task = tokio::spawn(handle_write_stream(write_half, data_to_stream_rx));

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

async fn handle_read_stream<R>(mut reader: R, tx: UnboundedSender<(u8, Vec<u8>)>) -> Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut buffer = Vec::with_capacity(READ_BUFFER_SIZE);
    let mut temp_buffer = vec![0u8; READ_BUFFER_SIZE];
    let mut total_frames = 0u64;
    let mut total_bytes = 0u64;

    loop {
        match reader.read(&mut temp_buffer).await {
            Ok(0) => {
                info!(
                    "TCP stream closed by peer (processed {} frames, {} bytes)",
                    total_frames, total_bytes
                );
                break;
            }
            Ok(n) => {
                trace!("Read {} bytes from TCP stream", n);

                if buffer.len() + n > MAX_BUFFER_SIZE {
                    warn!(
                        "Buffer size limit reached ({} + {} > {}), clearing buffer",
                        buffer.len(),
                        n,
                        MAX_BUFFER_SIZE
                    );
                    buffer.clear();

                    continue;
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

                    if let Err(e) = tx.send((channel, data)) {
                        error!("Failed to forward data from stream: {}", e);
                        return Err(e.into());
                    }

                    total_frames += 1;

                    buffer.drain(..consumed);
                }

                if !buffer.is_empty()
                    && buffer[0] != b'$'
                    && let Some(consumed) = try_parse_rtsp_message(&buffer)?
                {
                    debug!("Received RTSP control message, closing session");
                    buffer.drain(..consumed);
                    break;
                }

                if buffer.len() > MAX_BUFFER_SIZE / 2 {
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

    info!(
        "Read task completed: {} frames, {} bytes total",
        total_frames, total_bytes
    );
    Ok(())
}

fn parse_interleaved_frame(buffer: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>> {
    if buffer.len() < INTERLEAVED_HEADER_SIZE {
        return Ok(None);
    }

    if buffer[0] != b'$' {
        return Ok(None);
    }

    let channel = buffer[1];
    let length = u16::from_be_bytes([buffer[2], buffer[3]]) as usize;

    if length > MAX_FRAME_SIZE {
        warn!(
            "Interleaved frame too large: {} bytes (max: {}), skipping",
            length, MAX_FRAME_SIZE
        );

        return Ok(Some((channel, Vec::new(), INTERLEAVED_HEADER_SIZE)));
    }

    let total_size = INTERLEAVED_HEADER_SIZE + length;
    if buffer.len() < total_size {
        trace!(
            "Incomplete frame: have {} bytes, need {} bytes",
            buffer.len(),
            total_size
        );
        return Ok(None);
    }

    let data = buffer[INTERLEAVED_HEADER_SIZE..total_size].to_vec();

    Ok(Some((channel, data, total_size)))
}

fn try_parse_rtsp_message(buffer: &[u8]) -> Result<Option<usize>> {
    match rtsp_types::Message::<Vec<u8>>::parse(buffer) {
        Ok((msg, consumed)) => {
            if let rtsp_types::Message::Request(req) = msg
                && matches!(req.method(), rtsp_types::Method::Teardown)
            {
                info!("Received TEARDOWN in data stream");
                return Ok(Some(consumed));
            }

            Ok(Some(consumed))
        }
        Err(rtsp_types::ParseError::Incomplete(_)) => Ok(None),
        Err(e) => {
            warn!("Failed to parse RTSP message: {:?}, skipping byte", e);
            Ok(Some(1))
        }
    }
}

async fn handle_write_stream<W>(
    mut writer: W,
    mut rx: UnboundedReceiver<(u8, Vec<u8>)>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut total_frames = 0u64;
    let mut total_bytes = 0u64;

    while let Some((channel, data)) = rx.recv().await {
        if data.len() > MAX_FRAME_SIZE {
            error!(
                "Frame too large to send: {} bytes (max: {}), dropping",
                data.len(),
                MAX_FRAME_SIZE
            );
            continue;
        }

        let frame = build_interleaved_frame(channel, &data)?;

        trace!(
            "Sending interleaved frame: channel={}, length={}",
            channel,
            data.len()
        );

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

    let mut frame = Vec::with_capacity(INTERLEAVED_HEADER_SIZE + length);
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
    fn test_parse_oversized_frame() {
        const TEST_MAX_FRAME_SIZE: usize = 50000;

        let length = (TEST_MAX_FRAME_SIZE + 1) as u16;
        let data = vec![b'$', 0, (length >> 8) as u8, (length & 0xFF) as u8];

        let result = parse_interleaved_frame(&data).unwrap();
        if length as usize > MAX_FRAME_SIZE {
            assert!(result.is_some());
            let (channel, frame_data, consumed) = result.unwrap();
            assert_eq!(channel, 0);
            assert_eq!(frame_data.len(), 0);
            assert_eq!(consumed, 4);
        }
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
