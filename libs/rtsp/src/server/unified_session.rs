use anyhow::{Result, anyhow};
use rtsp_types::{Message, Method, Request, Response, StatusCode, Version};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tracing::{debug, error, info, trace, warn};

use super::{Handler, ServerConfig, ServerSession};
use crate::channels::{InterleavedChannel, InterleavedData};
use crate::sdp::parse_codecs_from_sdp;
use crate::tcp_stream::handle_tcp_stream;
use crate::types::{MediaInfo, SessionMode, TransportInfo};

#[derive(Debug, Clone)]
pub struct PortUpdate {
    pub connection_id: u32,
    pub media_info: MediaInfo,
}

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
    ) -> Result<(MediaInfo, Option<InterleavedChannel>)> {
        debug!(
            "Starting RTSP session: mode={:?}, addr={}",
            self.mode, self.addr
        );

        let mut video_channels: Option<(u8, u8)> = None;
        let mut audio_channels: Option<(u8, u8)> = None;
        let mut video_ports: Option<(u16, u16, u16, u16)> = None;
        let mut audio_ports: Option<(u16, u16, u16, u16)> = None;
        let mut actual_use_tcp = false;

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
                    debug!("Client requested transport: {}", transport_str);

                    let client_wants_tcp =
                        transport_str.contains("TCP") || transport_str.contains("interleaved");

                    let uri = request
                        .request_uri()
                        .map(|u| u.to_string())
                        .unwrap_or_default();

                    let is_video = if uri.contains("video")
                        || uri.contains("trackID=0")
                        || uri.contains("streamid=0")
                    {
                        true
                    } else if uri.contains("audio")
                        || uri.contains("trackID=1")
                        || uri.contains("streamid=1")
                    {
                        false
                    } else {
                        let sdp_bytes = self
                            .handler
                            .sdp_content()
                            .ok_or_else(|| anyhow!("No SDP"))?;
                        let sdp = sdp_types::Session::parse(sdp_bytes)?;

                        let has_video = sdp.medias.iter().any(|m| m.media == "video");
                        let has_audio = sdp.medias.iter().any(|m| m.media == "audio");

                        if has_video && !has_audio {
                            true
                        } else if !has_video && has_audio {
                            false
                        } else if has_video && has_audio {
                            video_channels.is_none() && video_ports.is_none()
                        } else {
                            true
                        }
                    };

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
                            debug!(
                                "Video UDP ports: client={}:{}, server={}:{}",
                                client_rtp, client_rtcp, server_rtp, server_rtcp
                            );
                        } else {
                            audio_ports = Some((client_rtp, client_rtcp, server_rtp, server_rtcp));
                            debug!(
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

                    if actual_use_tcp {
                        let channels = self.start_tcp_data_transfer().await?;
                        return Ok((media_info, Some(channels)));
                    } else {
                        tokio::spawn(async move {
                            if let Err(e) = self.keep_control_connection().await {
                                error!("Error in RTSP control connection: {}", e);
                            }
                        });

                        return Ok((media_info, None));
                    }
                }
                Method::Teardown => {
                    let response = self.handler.handle_teardown(&request).await?;
                    self.send_response(&response).await?;
                }
                _ => {
                    warn!("Unsupported method: {:?}", request.method());
                }
            }
        }
    }

    async fn keep_control_connection(mut self) -> Result<()> {
        debug!("Keeping RTSP control connection alive");

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

    async fn start_tcp_data_transfer(self) -> Result<InterleavedChannel> {
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

        Ok(match self.mode {
            SessionMode::Push => (data_to_stream_tx, data_from_stream_rx),
            SessionMode::Pull => (data_to_stream_tx, data_from_stream_rx),
        })
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
    Option<InterleavedChannel>,
    UnboundedReceiver<PortUpdate>,
)> {
    use tokio::net::TcpListener;

    info!(
        "Setting up RTSP server: addr={}, mode={:?}, tcp={}",
        listen_addr, mode, use_tcp
    );

    let listener = TcpListener::bind(listen_addr).await?;
    let local_addr = listener.local_addr()?;
    info!("RTSP server listening on {}", local_addr);

    let sdp_content = Arc::new(sdp_content);

    let (broadcast_tx, _) = broadcast::channel::<InterleavedData>(100);
    let broadcast_tx = Arc::new(broadcast_tx);

    let (main_data_to_webrtc_tx, main_data_to_webrtc_rx) = unbounded_channel::<InterleavedData>();
    let (main_data_from_webrtc_tx, mut main_data_from_webrtc_rx) =
        unbounded_channel::<InterleavedData>();

    let (port_update_tx, port_update_rx) = unbounded_channel::<PortUpdate>();
    let port_update_tx = Arc::new(port_update_tx);

    let broadcast_tx_clone = broadcast_tx.clone();
    tokio::spawn(async move {
        while let Some(data) = main_data_from_webrtc_rx.recv().await {
            let _ = broadcast_tx_clone.send(data);
        }
    });

    let main_data_to_webrtc_tx = Arc::new(main_data_to_webrtc_tx);

    let (media_info_tx, mut media_info_rx) = unbounded_channel::<MediaInfo>();
    let media_info_tx = Arc::new(media_info_tx);

    let mut connection_count = 0u32;

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((socket, addr)) => {
                    connection_count += 1;
                    let conn_id = connection_count;
                    info!("RTSP client #{} connected from {}", conn_id, addr);

                    let sessions = Arc::new(RwLock::new(HashMap::new()));
                    let config = ServerConfig::default();
                    let sdp_clone = (*sdp_content).clone();

                    let session =
                        RtspServerSession::new(socket, addr, sessions, config, sdp_clone, mode);

                    let media_info_tx_clone = media_info_tx.clone();
                    let main_data_to_webrtc_tx_clone = main_data_to_webrtc_tx.clone();
                    let broadcast_rx = broadcast_tx.subscribe();
                    let port_update_tx_clone = port_update_tx.clone();

                    tokio::spawn(async move {
                        match session.handle_session(use_tcp).await {
                            Ok((media_info, channels)) => {
                                info!("Connection #{} session established successfully", conn_id);

                                let _ = port_update_tx_clone.send(PortUpdate {
                                    connection_id: conn_id,
                                    media_info: media_info.clone(),
                                });

                                if conn_id == 1 {
                                    let _ = media_info_tx_clone.send(media_info);
                                }

                                if let Some((conn_tx, mut conn_rx)) = channels {
                                    let tx_clone = main_data_to_webrtc_tx_clone.clone();
                                    tokio::spawn(async move {
                                        info!("Connection #{} RTP receiver started", conn_id);
                                        while let Some(data) = conn_rx.recv().await {
                                            if tx_clone.send(data).is_err() {
                                                warn!(
                                                    "Connection #{} WebRTC channel closed",
                                                    conn_id
                                                );
                                                break;
                                            }
                                        }
                                        info!("Connection #{} RTP receiver stopped", conn_id);
                                    });

                                    let mut broadcast_rx = broadcast_rx;
                                    tokio::spawn(async move {
                                        info!("Connection #{} RTCP forwarder started", conn_id);
                                        loop {
                                            match broadcast_rx.recv().await {
                                                Ok(data) => {
                                                    if conn_tx.send(data).is_err() {
                                                        warn!(
                                                            "Connection #{} RTSP channel closed",
                                                            conn_id
                                                        );
                                                        break;
                                                    }
                                                }
                                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                                    warn!(
                                                        "Connection #{} lagged by {} messages",
                                                        conn_id, n
                                                    );
                                                }
                                                Err(broadcast::error::RecvError::Closed) => {
                                                    info!(
                                                        "Broadcast channel closed for connection #{}",
                                                        conn_id
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                        info!("Connection #{} RTCP forwarder stopped", conn_id);
                                    });
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Connection #{} error: {}, waiting for reconnection...",
                                    conn_id, e
                                );
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }
    });

    let media_info = media_info_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("Failed to receive media info from first connection"))?;

    let uses_tcp = media_info
        .video_transport
        .as_ref()
        .is_some_and(|t| matches!(t, TransportInfo::Tcp { .. }))
        || media_info
            .audio_transport
            .as_ref()
            .is_some_and(|t| matches!(t, TransportInfo::Tcp { .. }));

    if uses_tcp {
        Ok((
            media_info,
            Some((main_data_from_webrtc_tx, main_data_to_webrtc_rx)),
            port_update_rx,
        ))
    } else {
        Ok((media_info, None, port_update_rx))
    }
}
