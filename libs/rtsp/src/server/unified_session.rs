use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::RwLock;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use super::{Handler, ServerConfig, ServerSession};
use crate::channels::InterleavedData;
use crate::constants::{media_type, net, track};
use crate::sdp::parse_codecs_from_sdp;
use crate::tcp_stream::handle_tcp_stream;
use crate::types::{MediaInfo, SessionMode, TransportInfo};
use crate::{Message, Method, Request, Response};

#[derive(Debug, Clone)]
pub struct PortUpdate {
    pub connection_id: u32,
    pub media_info: MediaInfo,
}

/// Endpoint handed to the application handler when an RTSP PLAY/RECORD
/// session has been negotiated.
pub enum SessionEndpoint {
    /// PUSH mode: the server will forward incoming RTP data to this receiver.
    Push(UnboundedReceiver<InterleavedData>),
    /// PULL mode: the server expects the application to send RTP data on this
    /// sender.
    Pull(UnboundedSender<InterleavedData>),
}

/// Application-provided handler for per-path RTSP sessions.
///
/// A single server can multiplex many streams by URL path: the first path
/// segment of the request URI is treated as the stream identifier and passed
/// to each callback.
#[async_trait::async_trait]
pub trait SessionHandler: Send + Sync + 'static {
    /// Called when a PUSH client sends ANNOUNCE. `path` is the first URL path
    /// segment (the stream identifier). The handler may inspect the SDP and
    /// prepare tracks.
    async fn on_announce(&self, path: String, sdp: Vec<u8>) -> Result<()>;

    /// Called when a PULL client sends DESCRIBE. The handler should return the
    /// SDP describing the stream identified by `path`.
    async fn on_describe(&self, path: String) -> Result<Vec<u8>>;

    /// Called when the RTSP session has been established (after PLAY/RECORD).
    /// The server provides a [`SessionEndpoint`] matching the session mode.
    async fn on_session(
        &self,
        path: String,
        mode: SessionMode,
        media_info: MediaInfo,
        endpoint: SessionEndpoint,
    ) -> Result<()>;
}

enum ServerSide {
    Push(UnboundedSender<InterleavedData>),
    Pull(UnboundedReceiver<InterleavedData>),
}

pub struct RtspServerSession<H: SessionHandler> {
    handler: Handler,
    app_handler: Arc<H>,
    stream: TcpStream,
    addr: SocketAddr,
    mode: SessionMode,
}

impl<H: SessionHandler> RtspServerSession<H> {
    pub fn new(
        stream: TcpStream,
        addr: SocketAddr,
        sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
        config: ServerConfig,
        mode: SessionMode,
        app_handler: Arc<H>,
    ) -> Self {
        Self {
            handler: Handler::new(addr, sessions, config),
            app_handler,
            stream,
            addr,
            mode,
        }
    }

    pub async fn handle_session(mut self, _use_tcp: bool) -> Result<MediaInfo> {
        debug!(
            "Starting RTSP session: mode={:?}, addr={}",
            self.mode, self.addr
        );

        let mut path: Option<String> = None;
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
                    let p = request_path(&request)?.unwrap_or_default();
                    let sdp = self.app_handler.on_describe(p.clone()).await?;
                    self.handler.set_sdp(sdp);
                    path = Some(p);
                    let response = self.handler.handle_describe(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Announce => {
                    let p = request_path(&request)?.unwrap_or_default();
                    let sdp = request.body().to_vec();
                    self.app_handler.on_announce(p.clone(), sdp.clone()).await?;
                    self.handler.set_sdp(sdp);
                    path = Some(p);
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

                    let is_video = if uri.contains(media_type::VIDEO)
                        || uri.contains(track::VIDEO_TRACK_ID)
                        || uri.contains(track::VIDEO_STREAM_ID)
                    {
                        true
                    } else if uri.contains(media_type::AUDIO)
                        || uri.contains(track::AUDIO_TRACK_ID)
                        || uri.contains(track::AUDIO_STREAM_ID)
                    {
                        false
                    } else {
                        let sdp_bytes = self
                            .handler
                            .sdp_content()
                            .ok_or_else(|| anyhow!("No SDP"))?;
                        let sdp = sdp_types::Session::parse(sdp_bytes)?;

                        let has_video = sdp.medias.iter().any(|m| m.media == media_type::VIDEO);
                        let has_audio = sdp.medias.iter().any(|m| m.media == media_type::AUDIO);

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

                    let p = path.clone().unwrap_or_default();
                    if actual_use_tcp {
                        self.start_tcp_data_transfer(p, media_info.clone()).await?;
                    } else {
                        self.start_udp_data_transfer(p, media_info.clone()).await?;
                    }
                    return Ok(media_info);
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

    async fn start_tcp_data_transfer(self, path: String, media_info: MediaInfo) -> Result<()> {
        let (data_from_stream_tx, mut data_from_stream_rx) = unbounded_channel::<InterleavedData>();
        let (data_to_stream_tx, data_to_stream_rx) = unbounded_channel::<InterleavedData>();

        let (endpoint, server_side) = match self.mode {
            SessionMode::Push => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Push(rx), ServerSide::Push(tx))
            }
            SessionMode::Pull => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Pull(tx), ServerSide::Pull(rx))
            }
        };

        self.app_handler
            .on_session(path, self.mode, media_info, endpoint)
            .await?;

        let stream = self.stream;
        let mode = self.mode;
        tokio::spawn(async move {
            if let Err(e) =
                handle_tcp_stream(stream, mode, data_from_stream_tx, data_to_stream_rx).await
            {
                error!("TCP stream handler error: {}", e);
            }
        });

        match server_side {
            ServerSide::Push(tx) => {
                tokio::spawn(async move {
                    while let Some(data) = data_from_stream_rx.recv().await {
                        if tx.send(data).is_err() {
                            break;
                        }
                    }
                });
                // Nothing to send back to a PUSH client.
                drop(data_to_stream_tx);
            }
            ServerSide::Pull(mut rx) => {
                tokio::spawn(async move {
                    while let Some(data) = rx.recv().await {
                        if data_to_stream_tx.send(data).is_err() {
                            break;
                        }
                    }
                });
                // Drain incoming RTCP/TEARDOWN frames so the read half stays alive.
                tokio::spawn(
                    async move { while let Some(_data) = data_from_stream_rx.recv().await {} },
                );
            }
        }

        Ok(())
    }

    async fn start_udp_data_transfer(self, path: String, media_info: MediaInfo) -> Result<()> {
        let (endpoint, server_side) = match self.mode {
            SessionMode::Push => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Push(rx), ServerSide::Push(tx))
            }
            SessionMode::Pull => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Pull(tx), ServerSide::Pull(rx))
            }
        };

        self.app_handler
            .on_session(path, self.mode, media_info.clone(), endpoint)
            .await?;

        let client_addr = self.addr;
        let mode = self.mode;
        tokio::spawn(async move {
            if let Err(e) = run_udp_transfer(mode, client_addr, media_info, server_side).await {
                error!("UDP transfer error: {}", e);
            }
        });

        Ok(())
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

        let codecs = parse_codecs_from_sdp(&sdp)?;
        info!(
            "RTSP parsed codecs: video={:?}, audio={:?}",
            codecs.0, codecs.1
        );
        Ok(codecs)
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

fn request_path(request: &Request<Vec<u8>>) -> Result<Option<String>> {
    let uri = request
        .request_uri()
        .ok_or_else(|| anyhow!("Missing request URI"))?;
    let mut segments = uri
        .path_segments()
        .ok_or_else(|| anyhow!("Invalid request URI"))?;
    Ok(segments.next().map(|s| s.to_string()))
}

async fn run_udp_transfer(
    mode: SessionMode,
    client_addr: SocketAddr,
    media_info: MediaInfo,
    server_side: ServerSide,
) -> Result<()> {
    match mode {
        SessionMode::Push => {
            let ServerSide::Push(tx) = server_side else {
                return Err(anyhow!("Unexpected server side for push"));
            };

            // Forward incoming video RTP to the handler.
            if let Some(TransportInfo::Udp {
                rtp_recv_port: Some(port),
                ..
            }) = media_info.video_transport
            {
                let socket = bind_udp(&client_addr, port).await?;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 2048];
                    while let Ok((n, _)) = socket.recv_from(&mut buf).await {
                        if tx.send((0, buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                });
            }

            // Forward incoming audio RTP to the handler.
            if let Some(TransportInfo::Udp {
                rtp_recv_port: Some(port),
                ..
            }) = media_info.audio_transport
            {
                let socket = bind_udp(&client_addr, port).await?;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 2048];
                    while let Ok((n, _)) = socket.recv_from(&mut buf).await {
                        if tx.send((2, buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                });
            }

            drain_rtcp_ports(&client_addr, &media_info).await?;
        }
        SessionMode::Pull => {
            let ServerSide::Pull(mut rx) = server_side else {
                return Err(anyhow!("Unexpected server side for pull"));
            };

            let send_socket = UdpSocket::bind(net::bind_any_for(&client_addr)).await?;

            let mut channel_map: HashMap<u8, u16> = HashMap::new();
            if let Some(TransportInfo::Udp {
                rtp_send_port: Some(port),
                ..
            }) = media_info.video_transport
            {
                channel_map.insert(0, port);
            }
            if let Some(TransportInfo::Udp {
                rtp_send_port: Some(port),
                ..
            }) = media_info.audio_transport
            {
                channel_map.insert(2, port);
            }

            tokio::spawn(async move {
                while let Some((channel, data)) = rx.recv().await {
                    if let Some(&port) = channel_map.get(&channel) {
                        let dest = SocketAddr::new(client_addr.ip(), port);
                        if send_socket.send_to(&data, dest).await.is_err() {
                            break;
                        }
                    }
                }
            });

            drain_rtcp_ports(&client_addr, &media_info).await?;
        }
    }

    Ok(())
}

async fn drain_rtcp_ports(client_addr: &SocketAddr, media_info: &MediaInfo) -> Result<()> {
    let transports = [
        media_info.video_transport.as_ref(),
        media_info.audio_transport.as_ref(),
    ];
    for transport in transports.into_iter().flatten() {
        if let TransportInfo::Udp {
            rtcp_recv_port: Some(port),
            ..
        } = transport
        {
            let socket = bind_udp(client_addr, *port).await?;
            tokio::spawn(async move {
                let mut buf = vec![0u8; 2048];
                while socket.recv_from(&mut buf).await.is_ok() {}
            });
        }
    }
    Ok(())
}

async fn bind_udp(addr: &SocketAddr, port: u16) -> Result<UdpSocket> {
    let bind_addr = net::bind_addr_for(addr, port);
    UdpSocket::bind(&bind_addr)
        .await
        .map_err(|e| anyhow!("Failed to bind UDP socket {}: {}", bind_addr, e))
}

/// Start a single-port RTSP server that multiplexes sessions by URL path.
///
/// The server runs until `cancel` is cancelled. The application logic for each
/// stream is supplied by `handler`.
pub async fn setup_rtsp_server_with_handler<H>(
    listen_addr: &str,
    mode: SessionMode,
    use_tcp: bool,
    handler: H,
    cancel: CancellationToken,
) -> Result<()>
where
    H: SessionHandler,
{
    info!(
        "Setting up RTSP server: addr={}, mode={:?}, tcp={}",
        listen_addr, mode, use_tcp
    );

    let listener = TcpListener::bind(listen_addr).await?;
    let local_addr = listener.local_addr()?;
    info!("RTSP server listening on {}", local_addr);

    let handler = Arc::new(handler);
    let mut connection_count = 0u32;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("RTSP server on {} shutting down", local_addr);
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((socket, addr)) => {
                        connection_count += 1;
                        let conn_id = connection_count;
                        info!("RTSP client #{} connected from {}", conn_id, addr);

                        let sessions = Arc::new(RwLock::new(HashMap::new()));
                        let config = ServerConfig::default();
                        let session = RtspServerSession::new(
                            socket,
                            addr,
                            sessions,
                            config,
                            mode,
                            handler.clone(),
                        );

                        tokio::spawn(async move {
                            match session.handle_session(use_tcp).await {
                                Ok(media_info) => {
                                    info!(
                                        "Connection #{} session established: {:?}",
                                        conn_id, media_info
                                    );
                                }
                                Err(e) => {
                                    warn!("Connection #{} error: {}", conn_id, e);
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
        }
    }

    Ok(())
}
