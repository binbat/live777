use anyhow::{Result, anyhow};
use rtsp_types::{Message, Method, Request, Response, StatusCode, Version};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, error, info, trace, warn};

use super::{Handler, ServerConfig, ServerSession};
use crate::sdp::parse_codecs_from_sdp;
use crate::tcp_stream::handle_tcp_stream;
use crate::types::{MediaInfo, SessionMode, TransportInfo};

pub struct RtspServerSession {
    handler: Handler,
    stream: TcpStream,
    addr: SocketAddr,
    mode: SessionMode,
}

impl RtspServerSession {
    pub fn new(
        stream: TcpStream,
        addr: SocketAddr,
        sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
        config: ServerConfig,
        sdp_content: Vec<u8>,
        mode: SessionMode,
    ) -> Self {
        let mut handler = Handler::new(addr, sessions, config);
        handler.set_sdp(sdp_content);

        Self {
            handler,
            stream,
            addr,
            mode,
        }
    }

    pub async fn handle_session(
        mut self,
        _use_tcp: bool,
    ) -> Result<(
        MediaInfo,
        Option<UnboundedSender<(u8, Vec<u8>)>>,
        Option<UnboundedReceiver<(u8, Vec<u8>)>>,
    )> {
        debug!(
            "Starting RTSP session: mode={:?}, addr={}",
            self.mode, self.addr
        );

        let mut video_channels: Option<(u8, u8)> = None;
        let mut audio_channels: Option<(u8, u8)> = None;
        let mut video_ports: Option<(u16, u16, u16, u16)> = None;
        let mut audio_ports: Option<(u16, u16, u16, u16)> = None;
        let mut actual_use_tcp = false;
        let mut media_started = false;

        loop {
            let request = self.read_request().await?;
            self.handler.update_cseq(&request);

            match request.method() {
                Method::Options => {
                    let response = self.handler.handle_options(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Describe => {
                    let response = self.handler.handle_describe(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Announce => {
                    let response = self.handler.handle_announce(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Setup => {
                    let transport_header = request
                        .header(&rtsp_types::headers::TRANSPORT)
                        .ok_or_else(|| anyhow!("Missing Transport header"))?;

                    let transport_str = transport_header.as_str();
                    info!("Client requested transport: {}", transport_str);

                    let client_wants_tcp =
                        transport_str.contains("TCP") || transport_str.contains("interleaved");

                    let uri = request
                        .request_uri()
                        .map(|u| u.to_string())
                        .unwrap_or_default();
                    let is_video = uri.contains("video")
                        || uri.contains("trackID=0")
                        || uri.contains("streamid=0")
                        || (video_channels.is_none() && video_ports.is_none());

                    if client_wants_tcp {
                        actual_use_tcp = true;
                        let (response, rtp_ch, rtcp_ch) = self.handle_setup_tcp(&request).await?;

                        if is_video {
                            video_channels = Some((rtp_ch, rtcp_ch));
                            info!("Video TCP channels: RTP={}, RTCP={}", rtp_ch, rtcp_ch);
                        } else {
                            audio_channels = Some((rtp_ch, rtcp_ch));
                            info!("Audio TCP channels: RTP={}, RTCP={}", rtp_ch, rtcp_ch);
                        }

                        self.send_response(&response).await?;
                    } else {
                        actual_use_tcp = false;
                        let (response, client_rtp, client_rtcp, server_rtp, server_rtcp) =
                            self.handle_setup_udp(&request).await?;

                        if is_video {
                            video_ports = Some((client_rtp, client_rtcp, server_rtp, server_rtcp));
                            info!(
                                "Video UDP ports: client={}:{}, server={}:{}",
                                client_rtp, client_rtcp, server_rtp, server_rtcp
                            );
                        } else {
                            audio_ports = Some((client_rtp, client_rtcp, server_rtp, server_rtcp));
                            info!(
                                "Audio UDP ports: client={}:{}, server={}:{}",
                                client_rtp, client_rtcp, server_rtp, server_rtcp
                            );
                        }

                        self.send_response(&response).await?;
                    }
                }
                Method::Play | Method::Record => {
                    let response = match self.mode {
                        SessionMode::Pull => self.handler.handle_play(&request).await?,
                        SessionMode::Push => self.handler.handle_record(&request).await?,
                    };
                    self.send_response(&response).await?;

                    let media_info = if actual_use_tcp {
                        self.build_media_info_tcp(video_channels, audio_channels)?
                    } else {
                        self.build_media_info_udp(video_ports, audio_ports)?
                    };

                    info!("MediaInfo: {:?}", media_info);
                    media_started = true;

                    if actual_use_tcp {
                        let (tx, rx) = self.start_tcp_data_transfer().await?;
                        return Ok((media_info, Some(tx), Some(rx)));
                    } else {
                        tokio::spawn(async move {
                            if let Err(e) = self.keep_control_connection().await {
                                error!("Error in RTSP control connection: {}", e);
                            }
                        });

                        return Ok((media_info, None, None));
                    }
                }
                Method::Teardown => {
                    let response = self.handler.handle_teardown(&request).await?;
                    self.send_response(&response).await?;

                    if media_started {
                        info!("Session terminated by TEARDOWN after media started");
                        return Err(anyhow!("Session terminated by TEARDOWN"));
                    } else {
                        info!("Session terminated by TEARDOWN before media started");
                        return Err(anyhow!("Session terminated by TEARDOWN"));
                    }
                }
                _ => {
                    warn!("Unsupported method: {:?}", request.method());
                }
            }
        }
    }

    async fn keep_control_connection(mut self) -> Result<()> {
        debug!("Keeping RTSP control connection alive for UDP mode");

        loop {
            match self.read_request().await {
                Ok(request) => {
                    self.handler.update_cseq(&request);

                    match request.method() {
                        Method::Teardown => {
                            debug!("Received TEARDOWN, closing session");
                            if let Ok(response) = self.handler.handle_teardown(&request).await {
                                let _ = self.send_response(&response).await;
                            }
                            break;
                        }
                        Method::GetParameter => {
                            trace!("Received GET_PARAMETER keep-alive");
                            let response = Response::builder(Version::V1_0, StatusCode::Ok)
                                .header(rtsp_types::headers::CSEQ, self.handler.cseq().to_string())
                                .empty();
                            let _ = self.send_response(&response.map_body(|_| vec![])).await;
                        }
                        Method::Options => {
                            trace!("Received OPTIONS keep-alive");
                            if let Ok(response) = self.handler.handle_options(&request).await {
                                let _ = self.send_response(&response).await;
                            }
                        }
                        _ => {
                            warn!(
                                "Unexpected method after RECORD/PLAY: {:?}",
                                request.method()
                            );
                        }
                    }
                }
                Err(e) => {
                    info!("RTSP control connection closed: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_setup_tcp(
        &mut self,
        request: &Request<Vec<u8>>,
    ) -> Result<(Response<Vec<u8>>, u8, u8)> {
        let transport_header = request
            .header(&rtsp_types::headers::TRANSPORT)
            .ok_or_else(|| anyhow!("Missing Transport header"))?;

        self.handler
            .handle_setup_tcp(transport_header.as_str())
            .await
    }

    async fn handle_setup_udp(
        &mut self,
        request: &Request<Vec<u8>>,
    ) -> Result<(Response<Vec<u8>>, u16, u16, u16, u16)> {
        let transport_header = request
            .header(&rtsp_types::headers::TRANSPORT)
            .ok_or_else(|| anyhow!("Missing Transport header"))?;

        self.handler
            .handle_setup_udp(transport_header.as_str())
            .await
    }

    async fn start_tcp_data_transfer(
        self,
    ) -> Result<(
        UnboundedSender<(u8, Vec<u8>)>,
        UnboundedReceiver<(u8, Vec<u8>)>,
    )> {
        let (data_from_stream_tx, data_from_stream_rx) = unbounded_channel();
        let (data_to_stream_tx, data_to_stream_rx) = unbounded_channel();

        tokio::spawn(async move {
            if let Err(e) = handle_tcp_stream(
                self.stream,
                self.mode,
                data_from_stream_tx,
                data_to_stream_rx,
            )
            .await
            {
                error!("TCP stream handler error: {}", e);
            }
        });

        match self.mode {
            SessionMode::Push => Ok((data_to_stream_tx, data_from_stream_rx)),
            SessionMode::Pull => Ok((data_to_stream_tx, data_from_stream_rx)),
        }
    }

    fn build_media_info_tcp(
        &self,
        video_channels: Option<(u8, u8)>,
        audio_channels: Option<(u8, u8)>,
    ) -> Result<MediaInfo> {
        let video_transport = video_channels.map(|(rtp, rtcp)| TransportInfo::Tcp {
            rtp_channel: rtp,
            rtcp_channel: rtcp,
        });

        let audio_transport = audio_channels.map(|(rtp, rtcp)| TransportInfo::Tcp {
            rtp_channel: rtp,
            rtcp_channel: rtcp,
        });

        let (video_codec, audio_codec) = self.parse_codecs()?;

        Ok(MediaInfo {
            video_transport,
            audio_transport,
            video_codec,
            audio_codec,
        })
    }

    fn build_media_info_udp(
        &self,
        video_ports: Option<(u16, u16, u16, u16)>,
        audio_ports: Option<(u16, u16, u16, u16)>,
    ) -> Result<MediaInfo> {
        let video_transport =
            video_ports.map(|(client_rtp, client_rtcp, server_rtp, server_rtcp)| {
                TransportInfo::Udp {
                    rtp_send_port: Some(client_rtp),
                    rtp_recv_port: Some(server_rtp),
                    rtcp_send_port: Some(client_rtcp),
                    rtcp_recv_port: Some(server_rtcp),
                    server_addr: Some(self.addr),
                }
            });

        let audio_transport =
            audio_ports.map(|(client_rtp, client_rtcp, server_rtp, server_rtcp)| {
                TransportInfo::Udp {
                    rtp_send_port: Some(client_rtp),
                    rtp_recv_port: Some(server_rtp),
                    rtcp_send_port: Some(client_rtcp),
                    rtcp_recv_port: Some(server_rtcp),
                    server_addr: Some(self.addr),
                }
            });

        let (video_codec, audio_codec) = self.parse_codecs()?;

        Ok(MediaInfo {
            video_transport,
            audio_transport,
            video_codec,
            audio_codec,
        })
    }

    fn parse_codecs(
        &self,
    ) -> Result<(
        Option<crate::types::VideoCodecParams>,
        Option<crate::types::AudioCodecParams>,
    )> {
        let sdp_bytes = self
            .handler
            .sdp_content()
            .ok_or_else(|| anyhow!("No SDP content"))?;

        let sdp = sdp_types::Session::parse(sdp_bytes)
            .map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;

        parse_codecs_from_sdp(&sdp)
    }

    async fn send_response(&mut self, response: &Response<Vec<u8>>) -> Result<()> {
        let mut buffer = Vec::new();
        response.write(&mut buffer)?;
        self.stream.write_all(&buffer).await?;
        trace!("Sent RTSP response to {}", self.addr);
        Ok(())
    }

    async fn read_request(&mut self) -> Result<Request<Vec<u8>>> {
        let mut buffer = Vec::with_capacity(8192);
        let mut temp_buf = vec![0u8; 4096];

        loop {
            let n = self.stream.read(&mut temp_buf).await?;
            if n == 0 {
                return Err(anyhow!("Connection closed"));
            }

            buffer.extend_from_slice(&temp_buf[..n]);
            trace!("Read {} bytes, total buffer: {} bytes", n, buffer.len());

            match Message::<Vec<u8>>::parse(&buffer) {
                Ok((Message::Request(request), consumed)) => {
                    trace!(
                        "Received RTSP request: {:?} from {}, consumed {} of {} bytes",
                        request.method(),
                        self.addr,
                        consumed,
                        buffer.len()
                    );
                    return Ok(request);
                }
                Err(rtsp_types::ParseError::Incomplete(needed)) => {
                    trace!(
                        "Incomplete RTSP message (current: {} bytes, needed: {:?}), reading more...",
                        buffer.len(),
                        needed
                    );
                    continue;
                }
                Err(e) => {
                    error!("Failed to parse RTSP request: {:?}", e);
                    error!(
                        "Buffer content (first 200 bytes): {:?}",
                        String::from_utf8_lossy(&buffer[..buffer.len().min(200)])
                    );
                    return Err(anyhow!("Failed to parse RTSP request: {:?}", e));
                }
                Ok(_) => {
                    return Err(anyhow!("Expected request, got response"));
                }
            }
        }
    }
}

pub async fn setup_rtsp_server_session(
    listen_addr: &str,
    sdp_content: Vec<u8>,
    mode: SessionMode,
    use_tcp: bool,
) -> Result<(
    MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    use tokio::net::TcpListener;

    info!(
        "Setting up RTSP server: addr={}, mode={:?}, tcp={}",
        listen_addr, mode, use_tcp
    );

    let listener = TcpListener::bind(listen_addr).await?;
    let local_addr = listener.local_addr()?;
    info!("RTSP server listening on {}", local_addr);

    let (socket, addr) = listener.accept().await?;
    info!("RTSP client connected from {}", addr);

    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let config = ServerConfig::default();

    let session = RtspServerSession::new(socket, addr, sessions, config, sdp_content, mode);
    session.handle_session(use_tcp).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_mode() {
        assert_ne!(SessionMode::Push, SessionMode::Pull);
    }
}
