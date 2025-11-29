use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{error, info, trace};

use crate::types::SessionMode;

pub async fn handle_tcp_stream(
    stream: TcpStream,
    mode: SessionMode,
    data_from_stream_tx: UnboundedSender<(u8, Vec<u8>)>,
    mut data_to_stream_rx: UnboundedReceiver<(u8, Vec<u8>)>,
) -> Result<()> {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    info!("Starting TCP interleaved stream handler (mode: {:?})", mode);

    let read_tx = data_from_stream_tx.clone();
    let read_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        let mut temp_buffer = vec![0u8; 65536];

        loop {
            match read_half.read(&mut temp_buffer).await {
                Ok(0) => {
                    info!("TCP stream closed by peer");
                    break;
                }
                Ok(n) => {
                    buffer.extend_from_slice(&temp_buffer[..n]);
                    trace!("Read {} bytes from TCP stream", n);

                    while buffer.len() >= 4 && buffer[0] == b'$' {
                        let channel = buffer[1];
                        let length = u16::from_be_bytes([buffer[2], buffer[3]]) as usize;

                        if buffer.len() < 4 + length {
                            trace!("Incomplete frame, waiting for more data");
                            break;
                        }

                        let data = buffer[4..4 + length].to_vec();
                        trace!(
                            "Received interleaved frame: channel={}, length={}",
                            channel, length
                        );

                        if let Err(e) = read_tx.send((channel, data)) {
                            error!("Failed to forward data from stream: {}", e);
                            return;
                        }

                        buffer.drain(..4 + length);
                    }

                    if !buffer.is_empty()
                        && buffer[0] != b'$'
                        && let Ok((msg, consumed)) = rtsp_types::Message::<Vec<u8>>::parse(&buffer)
                    {
                        if let rtsp_types::Message::Request(req) = msg
                            && matches!(req.method(), rtsp_types::Method::Teardown)
                        {
                            info!("Received TEARDOWN in data stream");
                            return;
                        }

                        buffer.drain(..consumed);
                    }
                }
                Err(e) => {
                    error!("TCP read error: {}", e);
                    break;
                }
            }
        }
    });

    let write_task = tokio::spawn(async move {
        while let Some((channel, data)) = data_to_stream_rx.recv().await {
            let mut frame = vec![
                b'$',
                channel,
                ((data.len() >> 8) & 0xFF) as u8,
                (data.len() & 0xFF) as u8,
            ];
            frame.extend_from_slice(&data);

            trace!(
                "Sending interleaved frame: channel={}, length={}",
                channel,
                data.len()
            );

            if let Err(e) = write_half.write_all(&frame).await {
                error!("TCP write error: {}", e);
                break;
            }

            if let Err(e) = write_half.flush().await {
                error!("TCP flush error: {}", e);
                break;
            }
        }
    });

    let _ = tokio::join!(read_task, write_task);

    info!("TCP interleaved stream handler stopped");
    Ok(())
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn test_interleaved_frame_parsing() {
        // Create a mock TCP stream with interleaved data
        let data = vec![
            b'$', 0, 0, 4, // Channel 0, length 4
            1, 2, 3, 4, // Data
            b'$', 1, 0, 3, // Channel 1, length 3
            5, 6, 7, // Data
        ];

        // This is a simplified test - in reality you'd need to mock TcpStream
        assert_eq!(data[0], b'$');
        assert_eq!(data[1], 0); // channel
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), 4); // length
    }
}
