use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::RwLock;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use super::{Handler, ServerConfig, ServerSession};
use crate::channels::InterleavedData;
use crate::constants::{media_type, net, udp_route};
use crate::sdp::parse_codecs_from_sdp;
use crate::tcp_stream::handle_tcp_stream;
use crate::types::{MediaInfo, SessionMode, TransportInfo};
use crate::{Message, Method, Request, Response, StatusCode, Version, headers};

#[derive(Debug, Clone)]
pub struct PortUpdate {
    pub connection_id: u32,
    pub media_info: MediaInfo,
}

/// Endpoint handed to the application handler when an RTSP PLAY/RECORD
/// session has been negotiated.
pub enum SessionEndpoint {
    /// PUSH mode: the server will forward incoming RTP/RTCP data to `rx`, and
    /// the application can send RTP/RTCP data back to the client on `tx`.
    Push(Receiver<InterleavedData>, Sender<InterleavedData>),
    /// PULL mode: the server expects the application to send RTP/RTCP data on
    /// the sender, and forwards RTCP received from the pull client on the
    /// receiver.
    Pull(Sender<InterleavedData>, Receiver<InterleavedData>),
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
    /// The server provides a [`SessionEndpoint`] matching the session mode and
    /// a session-scoped [`CancellationToken`] that fires on TEARDOWN or server
    /// shutdown, allowing the handler to stop spawned tasks cleanly.
    async fn on_session(
        &self,
        path: String,
        mode: SessionMode,
        media_info: MediaInfo,
        endpoint: SessionEndpoint,
        cancel: CancellationToken,
    ) -> Result<()>;

    /// Called for every incoming RTCP packet (UDP RTCP port or TCP interleaved
    /// odd channel). The default implementation ignores it.
    async fn on_rtcp(&self, _path: String, _data: Vec<u8>) -> Result<()> {
        Ok(())
    }
}

enum ServerSide {
    Push(Sender<InterleavedData>),
    Pull(Receiver<InterleavedData>, Sender<InterleavedData>),
}

/// Result of a fully handled RTSP session.
pub(crate) enum SessionResult {
    /// PLAY/RECORD was sent and data transfer has started.
    Established(Box<MediaInfo>),
    /// TEARDOWN was received before data transfer began.
    Teardown,
}

pub struct RtspServerSession<H: SessionHandler> {
    handler: Handler,
    app_handler: Arc<H>,
    stream: TcpStream,
    addr: SocketAddr,
    local_addr: SocketAddr,
    mode: SessionMode,
    read_buffer: Vec<u8>,
    video_udp_sockets: Option<(UdpSocket, UdpSocket)>,
    audio_udp_sockets: Option<(UdpSocket, UdpSocket)>,
    /// Server-level cancellation token. Made a child of this so that server
    /// shutdown propagates into all session data tasks.
    cancel: CancellationToken,
}

impl<H: SessionHandler> RtspServerSession<H> {
    pub fn new(
        stream: TcpStream,
        addr: SocketAddr,
        local_addr: SocketAddr,
        sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
        config: ServerConfig,
        mode: SessionMode,
        app_handler: Arc<H>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            handler: Handler::new(addr, sessions, config),
            app_handler,
            stream,
            addr,
            local_addr,
            mode,
            read_buffer: Vec::with_capacity(8192),
            video_udp_sockets: None,
            audio_udp_sockets: None,
            cancel,
        }
    }

    pub(crate) async fn handle_session(mut self, guard: ConnectionGuard) -> Result<SessionResult> {
        debug!(
            "Starting RTSP session: mode={:?}, addr={}",
            self.mode, self.addr
        );

        let mut session_mode = self.mode;
        let mut path: Option<String> = None;
        let mut video_channels: Option<(u8, u8)> = None;
        let mut audio_channels: Option<(u8, u8)> = None;
        let mut video_ports: Option<(u16, u16, u16, u16)> = None;
        let mut audio_ports: Option<(u16, u16, u16, u16)> = None;
        let mut established_transport: Option<bool> = None;
        let session_cancel = self.cancel.child_token();

        loop {
            let request = self.read_request().await?;
            self.handler.update_cseq(&request);
            self.handler.update_activity().await;

            match request.method() {
                Method::Options => {
                    let response = self.handler.handle_options(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Describe => {
                    if session_mode == SessionMode::Push {
                        self.send_method_not_allowed(&request).await?;
                        return Err(anyhow!("DESCRIBE is not supported on a push session"));
                    }
                    if let Err(response) = self.handler.check_auth(&request) {
                        self.send_response(&response).await?;
                        continue;
                    }
                    if session_mode == SessionMode::Mixed {
                        session_mode = SessionMode::Pull;
                    }

                    let p = request_path(&request)?;
                    let sdp = self.app_handler.on_describe(p.clone()).await?;
                    self.handler.set_sdp(sdp);
                    path = Some(p);
                    let response = self.handler.handle_describe(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Announce => {
                    if session_mode == SessionMode::Pull {
                        self.send_method_not_allowed(&request).await?;
                        return Err(anyhow!("ANNOUNCE is not supported on a pull session"));
                    }
                    if let Err(response) = self.handler.check_auth(&request) {
                        self.send_response(&response).await?;
                        continue;
                    }
                    if session_mode == SessionMode::Mixed {
                        session_mode = SessionMode::Push;
                    }

                    let p = request_path(&request)?;
                    let sdp = request.body().to_vec();
                    self.app_handler.on_announce(p.clone(), sdp.clone()).await?;
                    self.handler.set_sdp(sdp);
                    path = Some(p);
                    let response = self.handler.handle_announce(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Setup => {
                    if let Err(response) = self.handler.check_auth(&request) {
                        self.send_response(&response).await?;
                        return Err(anyhow!("RTSP SETUP authentication required"));
                    }

                    let transport_header = match request.header(&rtsp_types::headers::TRANSPORT) {
                        Some(h) => h,
                        None => {
                            warn!("SETUP missing Transport header from {}", self.addr);
                            let response = Response::builder(Version::V1_0, StatusCode::BadRequest)
                                .header(headers::CSEQ, self.handler.cseq().to_string())
                                .empty();
                            self.send_response(&response.map_body(|_| vec![])).await?;
                            return Err(anyhow!("Missing Transport header"));
                        }
                    };

                    let transport_str = transport_header.as_str();
                    debug!("Client requested transport: {}", transport_str);

                    let transport_lower = transport_str.to_ascii_lowercase();
                    let client_wants_tcp =
                        transport_lower.contains("tcp") || transport_lower.contains("interleaved");

                    if let Some(prev) = established_transport {
                        if prev != client_wants_tcp {
                            warn!(
                                "Rejecting SETUP for {}: mixed TCP/UDP transport is not supported",
                                self.addr
                            );
                            let response =
                                Response::builder(Version::V1_0, StatusCode::UnsupportedTransport)
                                    .header(headers::CSEQ, self.handler.cseq().to_string())
                                    .empty();
                            self.send_response(&response.map_body(|_| vec![])).await?;
                            return Err(anyhow!("Mixed TCP/UDP transport is not supported"));
                        }
                    } else {
                        established_transport = Some(client_wants_tcp);
                    }

                    let uri = request
                        .request_uri()
                        .map(|u| u.to_string())
                        .unwrap_or_default();

                    let is_video = {
                        let sdp = self.handler.parsed_sdp().ok_or_else(|| anyhow!("No SDP"))?;
                        match resolve_setup_media_kind(
                            &uri,
                            sdp,
                            video_channels,
                            video_ports,
                            audio_channels,
                            audio_ports,
                        ) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!("SETUP media resolution failed for {}: {}", self.addr, e);
                                let response =
                                    Response::builder(Version::V1_0, StatusCode::BadRequest)
                                        .header(headers::CSEQ, self.handler.cseq().to_string())
                                        .empty();
                                self.send_response(&response.map_body(|_| vec![])).await?;
                                return Err(e);
                            }
                        }
                    };

                    let already_setup = if is_video {
                        video_channels.is_some() || video_ports.is_some()
                    } else {
                        audio_channels.is_some() || audio_ports.is_some()
                    };
                    if already_setup {
                        warn!(
                            "Duplicate SETUP for {} media from {}",
                            if is_video { "video" } else { "audio" },
                            self.addr
                        );
                        let response =
                            Response::builder(Version::V1_0, StatusCode::MethodNotValidInThisState)
                                .header(headers::CSEQ, self.handler.cseq().to_string())
                                .empty();
                        self.send_response(&response.map_body(|_| vec![])).await?;
                        return Err(anyhow!("Duplicate SETUP for the same media"));
                    }

                    if client_wants_tcp {
                        let (response, rtp_ch, rtcp_ch) = self.handle_setup_tcp(&request).await?;

                        if is_video {
                            video_channels = Some((rtp_ch, rtcp_ch));
                            debug!("Video TCP channels: RTP={}, RTCP={}", rtp_ch, rtcp_ch);
                        } else {
                            audio_channels = Some((rtp_ch, rtcp_ch));
                            debug!("Audio TCP channels: RTP={}, RTCP={}", rtp_ch, rtcp_ch);
                        }

                        self.send_response(&response).await?;
                    } else {
                        let (response, client_rtp, client_rtcp, server_rtp, server_rtcp) =
                            self.handle_setup_udp(&request).await?;

                        if let Some(sockets) = self.handler.take_udp_sockets() {
                            if is_video {
                                self.video_udp_sockets = Some(sockets);
                            } else {
                                self.audio_udp_sockets = Some(sockets);
                            }
                        }

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
                    if session_mode == SessionMode::Mixed {
                        return Err(anyhow!("Session mode must be resolved before PLAY/RECORD"));
                    }

                    let has_transport = video_channels.is_some()
                        || video_ports.is_some()
                        || audio_channels.is_some()
                        || audio_ports.is_some();
                    if !has_transport {
                        warn!(
                            "PLAY/RECORD received from {} without prior SETUP",
                            self.addr
                        );
                        let response =
                            Response::builder(Version::V1_0, StatusCode::MethodNotValidInThisState)
                                .header(headers::CSEQ, self.handler.cseq().to_string())
                                .empty();
                        self.send_response(&response.map_body(|_| vec![])).await?;
                        return Err(anyhow!("PLAY/RECORD without SETUP"));
                    }

                    let response = match session_mode {
                        SessionMode::Pull => self.handler.handle_play(&request).await?,
                        SessionMode::Push => self.handler.handle_record(&request).await?,
                        SessionMode::Mixed => {
                            return Err(anyhow!(
                                "Session mode must be resolved before PLAY/RECORD"
                            ));
                        }
                    };
                    self.send_response(&response).await?;

                    let use_tcp = established_transport.unwrap_or(false);
                    let mut media_info = if use_tcp {
                        self.build_media_info_tcp(video_channels, audio_channels)?
                    } else {
                        self.build_media_info_udp(video_ports, audio_ports)?
                    };

                    // Audio-only streams may have transport assigned to
                    // `video_transport` when the SETUP resolver can't match the
                    // control URL to a media section. Normalize so downstream
                    // consumers always find the transport on `audio_transport`.
                    media_info.normalize_audio_only();

                    info!("MediaInfo: {:?}", media_info);

                    let p = path.clone().unwrap_or_default();
                    if use_tcp {
                        self.start_tcp_data_transfer(
                            p,
                            media_info.clone(),
                            session_mode,
                            session_cancel.child_token(),
                            guard,
                        )
                        .await?;
                    } else {
                        self.start_udp_data_transfer(
                            p,
                            media_info.clone(),
                            session_mode,
                            session_cancel.child_token(),
                            guard,
                        )
                        .await?;
                    }
                    return Ok(SessionResult::Established(Box::new(media_info)));
                }
                Method::Teardown => {
                    let response = self.handler.handle_teardown(&request).await?;
                    self.send_response(&response).await?;
                    session_cancel.cancel();
                    break Ok(SessionResult::Teardown);
                }
                _ => {
                    warn!("Unsupported method: {:?}", request.method());
                    self.send_method_not_allowed(&request).await?;
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

    async fn start_tcp_data_transfer(
        self,
        path: String,
        media_info: MediaInfo,
        session_mode: SessionMode,
        cancel: CancellationToken,
        guard: ConnectionGuard,
    ) -> Result<()> {
        use crate::channels::DEFAULT_CHANNEL_CAPACITY as DATA_CHANNEL_CAPACITY;
        let (data_from_stream_tx, mut data_from_stream_rx) =
            channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
        let (data_to_stream_tx, data_to_stream_rx) =
            channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);

        let (endpoint, server_side) = match session_mode {
            SessionMode::Push => {
                let (tx, rx) = channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
                (
                    SessionEndpoint::Push(rx, data_to_stream_tx.clone()),
                    ServerSide::Push(tx),
                )
            }
            SessionMode::Pull => {
                let (tx, rx) = channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
                let (rtcp_tx, rtcp_rx) = channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
                (
                    SessionEndpoint::Pull(tx, rtcp_rx),
                    ServerSide::Pull(rx, rtcp_tx),
                )
            }
            SessionMode::Mixed => return Err(anyhow!("session mode must be resolved")),
        };

        self.app_handler
            .on_session(
                path.clone(),
                session_mode,
                media_info,
                endpoint,
                cancel.clone(),
            )
            .await?;

        let stream = self.stream;
        tokio::spawn(async move {
            let _guard = guard;
            if let Err(e) = handle_tcp_stream(
                stream,
                session_mode,
                data_from_stream_tx,
                data_to_stream_rx,
                cancel,
                session_mode == SessionMode::Pull,
            )
            .await
            {
                error!("TCP stream handler error: {}", e);
            }
        });

        match server_side {
            ServerSide::Push(tx) => {
                tokio::spawn(async move {
                    while let Some(data) = data_from_stream_rx.recv().await {
                        if tx.send(data).await.is_err() {
                            break;
                        }
                    }
                });
                // `data_to_stream_tx` was handed to the application handler via
                // `SessionEndpoint::Push` so it can send RTCP feedback to the
                // push client when needed.
            }
            ServerSide::Pull(mut rx, rtcp_tx) => {
                tokio::spawn(async move {
                    while let Some(data) = rx.recv().await {
                        if data_to_stream_tx.send(data).await.is_err() {
                            break;
                        }
                    }
                });
                // Handle incoming RTCP frames so the read half stays alive. In
                // Pull mode the client should only send RTCP on odd channels;
                // even-channel RTP is filtered inside handle_tcp_stream to avoid
                // filling this bounded channel and stalling the session.
                tokio::spawn(async move {
                    while let Some((channel, data)) = data_from_stream_rx.recv().await {
                        if channel % 2 != 0 && rtcp_tx.send((channel, data)).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }

        Ok(())
    }

    async fn start_udp_data_transfer(
        mut self,
        path: String,
        media_info: MediaInfo,
        session_mode: SessionMode,
        cancel: CancellationToken,
        guard: ConnectionGuard,
    ) -> Result<()> {
        use crate::channels::DEFAULT_CHANNEL_CAPACITY as DATA_CHANNEL_CAPACITY;
        let (data_to_stream_tx, data_to_stream_rx) =
            channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
        let (endpoint, server_side) = match session_mode {
            SessionMode::Push => {
                let (tx, rx) = channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
                (
                    SessionEndpoint::Push(rx, data_to_stream_tx),
                    ServerSide::Push(tx),
                )
            }
            SessionMode::Pull => {
                let (tx, rx) = channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
                let (rtcp_tx, rtcp_rx) = channel::<InterleavedData>(DATA_CHANNEL_CAPACITY);
                (
                    SessionEndpoint::Pull(tx, rtcp_rx),
                    ServerSide::Pull(rx, rtcp_tx),
                )
            }
            SessionMode::Mixed => return Err(anyhow!("session mode must be resolved")),
        };

        // Capture session state before `self` is consumed so the UDP control
        // connection can keep the session alive.
        let sessions = self.handler.sessions();
        let session_id = self.handler.session_id().cloned();

        let app_handler = self.app_handler.clone();
        self.app_handler
            .on_session(
                path.clone(),
                session_mode,
                media_info.clone(),
                endpoint,
                cancel.clone(),
            )
            .await?;

        let client_addr = self.addr;
        let local_addr = self.local_addr;
        let video_sockets = self.video_udp_sockets.take();
        let audio_sockets = self.audio_udp_sockets.take();
        let run_cancel = cancel.clone();
        tokio::spawn(async move {
            if let Err(e) = run_udp_transfer(
                session_mode,
                client_addr,
                local_addr,
                media_info,
                server_side,
                data_to_stream_rx,
                app_handler,
                path,
                video_sockets,
                audio_sockets,
                run_cancel.clone(),
            )
            .await
            {
                error!("UDP transfer error: {}", e);
                run_cancel.cancel();
                return;
            }

            // run_udp_transfer starts the UDP data-plane tasks and then
            // returns. Keep this task alive so the session token is not
            // cancelled until TEARDOWN, control EOF, or server shutdown.
            run_cancel.cancelled().await;
        });

        // Keep the RTSP control connection alive for UDP sessions so that
        // clients (e.g. ffmpeg) do not see an unexpected EOF before TEARDOWN.
        // Minimal RTSP message handling: respond to OPTIONS/GET_PARAMETER and
        // honour TEARDOWN so clients can close cleanly.
        let stream = self.stream;
        let control_cancel = cancel.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let (mut read_half, mut write_half) = tokio::io::split(stream);
            let mut buffer = Vec::with_capacity(4096);

            loop {
                let message = tokio::select! {
                    _ = control_cancel.cancelled() => {
                        debug!("UDP control connection cancelled");
                        break;
                    }
                    result = read_rtsp_message(&mut read_half, &mut buffer) => result,
                };

                match message {
                    Ok(Message::Request(request)) => {
                        let cseq = request
                            .header(&headers::CSEQ)
                            .map(|h| h.as_str().to_string())
                            .unwrap_or_else(|| "0".to_string());

                        let response = match request.method() {
                            Method::Options => Response::builder(Version::V1_0, StatusCode::Ok)
                                .header(headers::CSEQ, cseq.as_str())
                                .header(
                                    headers::PUBLIC,
                                    "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, ANNOUNCE, RECORD, GET_PARAMETER",
                                )
                                .empty(),
                            _ => Response::builder(Version::V1_0, StatusCode::Ok)
                                .header(headers::CSEQ, cseq.as_str())
                                .empty(),
                        }
                        .map_body(|_| vec![]);

                        let mut out = Vec::new();
                        if response.write(&mut out).is_err() {
                            break;
                        }
                        if write_half.write_all(&out).await.is_err() {
                            break;
                        }

                        // Any RTSP request on the control connection means the
                        // session is still active.
                        if let Some(ref id) = session_id {
                            update_session_activity(sessions.clone(), id.clone()).await;
                        }

                        if request.method() == Method::Teardown {
                            break;
                        }
                    }
                    Ok(_) => break,
                    Err(_) => break,
                }
            }
            let _ = write_half.shutdown().await;
            control_cancel.cancel();
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
        let sdp = self
            .handler
            .parsed_sdp()
            .ok_or_else(|| anyhow!("No SDP content"))?;

        let codecs = parse_codecs_from_sdp(sdp)?;
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

    async fn send_method_not_allowed(&mut self, _request: &Request<Vec<u8>>) -> Result<()> {
        let response = Response::builder(Version::V1_0, StatusCode::MethodNotAllowed)
            .header(headers::CSEQ, self.handler.cseq().to_string())
            .header(
                headers::ALLOW,
                "OPTIONS, DESCRIBE, ANNOUNCE, SETUP, PLAY, RECORD, TEARDOWN",
            )
            .empty();
        self.send_response(&response.map_body(|_| vec![])).await
    }

    async fn read_request(&mut self) -> Result<Request<Vec<u8>>> {
        let message = read_rtsp_message(&mut self.stream, &mut self.read_buffer).await?;
        match message {
            Message::Request(request) => {
                trace!(
                    "Received RTSP request: {:?} from {}, buffer {} bytes",
                    request.method(),
                    self.addr,
                    self.read_buffer.len()
                );
                Ok(request)
            }
            _ => Err(anyhow!("Expected request, got response")),
        }
    }
}

async fn update_session_activity(
    sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
    session_id: String,
) {
    let mut sessions = sessions.write().await;
    if let Some(session) = sessions.get_mut(&session_id) {
        session.update_activity();
    }
}

/// Read a complete RTSP message from `reader`, accumulating into `buffer`.
/// Consumed bytes are drained from `buffer` before the message is returned.
pub(crate) async fn read_rtsp_message<R>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
) -> Result<Message<Vec<u8>>>
where
    R: AsyncReadExt + Unpin,
{
    let mut temp_buf = [0u8; 4096];

    loop {
        match Message::<Vec<u8>>::parse(buffer) {
            Ok((message, consumed)) => {
                buffer.drain(..consumed);
                return Ok(message);
            }
            Err(rtsp_types::ParseError::Incomplete(_)) => {}
            Err(e) => return Err(anyhow!("Failed to parse RTSP message: {:?}", e)),
        }

        if buffer.len() >= crate::constants::buffer::MAX_BUFFER_SIZE {
            return Err(anyhow!(
                "RTSP message buffer limit exceeded ({} bytes); closing connection",
                buffer.len()
            ));
        }

        let n = reader.read(&mut temp_buf).await?;
        if n == 0 {
            return Err(anyhow!("Connection closed"));
        }
        buffer.extend_from_slice(&temp_buf[..n]);
    }
}

fn request_path(request: &Request<Vec<u8>>) -> Result<String> {
    let uri = request
        .request_uri()
        .ok_or_else(|| anyhow!("Missing request URI"))?;
    let mut segments = uri
        .path_segments()
        .ok_or_else(|| anyhow!("Invalid request URI"))?;
    Ok(segments.next().map(|s| s.to_string()).unwrap_or_default())
}

/// Resolve whether a SETUP request targets the video media.
///
/// Resolution order:
/// 1. Match against each media's `a=control` attribute.
/// 2. Explicit `video` / `audio` keywords in the URI.
/// 3. `streamid=N` / `trackID=N` refers to the N-th media in the SDP.
/// 4. Fallback based on whether video or audio has already been set up.
///
/// Returns an error when the target cannot be disambiguated (e.g. both media
/// are present and neither has been set up yet).
fn resolve_setup_media_kind(
    uri: &str,
    sdp: &sdp_types::Session,
    video_channels: Option<(u8, u8)>,
    video_ports: Option<(u16, u16, u16, u16)>,
    audio_channels: Option<(u8, u8)>,
    audio_ports: Option<(u16, u16, u16, u16)>,
) -> Result<bool> {
    let uri_lower = uri.to_lowercase();

    // 1. Match against control attributes.
    for media in &sdp.medias {
        let is_video = media.media == media_type::VIDEO;
        let matched = media.attributes.iter().any(|a| {
            a.attribute == "control"
                && a.value.as_ref().is_some_and(|control| {
                    let control_lower = control.to_lowercase();
                    if control_lower == "*" {
                        return false;
                    }
                    // Require the control value to match the end of the URI as a
                    // complete segment, avoiding false positives like control
                    // "track1" matching URI ending in "track10".
                    uri_lower == control_lower
                        || uri_lower.ends_with(&format!("/{}", control_lower))
                })
        });
        if matched {
            return Ok(is_video);
        }
    }

    // 2. Explicit video/audio keywords in the final path segment only, so a
    // stream ID like "myvideo" does not confuse audio-only streams.
    if let Some(segment) = last_path_segment(&uri_lower) {
        if segment == media_type::VIDEO {
            return Ok(true);
        }
        if segment == media_type::AUDIO {
            return Ok(false);
        }
    }

    // 3. streamid=N / trackID=N
    if let Some(index) = parse_track_index(uri)
        && let Some(media) = sdp.medias.get(index)
    {
        return Ok(media.media == media_type::VIDEO);
    }

    // 4. Fallback based on already-negotiated transports.
    let has_video = sdp.medias.iter().any(|m| m.media == media_type::VIDEO);
    let has_audio = sdp.medias.iter().any(|m| m.media == media_type::AUDIO);
    match (has_video, has_audio) {
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        (true, true) => {
            let video_setup = video_channels.is_some() || video_ports.is_some();
            let audio_setup = audio_channels.is_some() || audio_ports.is_some();
            match (video_setup, audio_setup) {
                (false, true) => Ok(true),  // audio already set up, this must be video
                (true, false) => Ok(false), // video already set up, this must be audio
                (true, true) => Err(anyhow!(
                    "Both video and audio transports are already configured"
                )),
                (false, false) => Err(anyhow!(
                    "Ambiguous SETUP target; supply a=control, streamid or trackID"
                )),
            }
        }
        (false, false) => Ok(true),
    }
}

fn last_path_segment(uri: &str) -> Option<&str> {
    uri.rsplit('/')
        .next()
        .map(|s| s.split('?').next().unwrap_or(s))
}

fn parse_track_index(uri: &str) -> Option<usize> {
    let uri_lower = uri.to_lowercase();
    for prefix in ["streamid=", "trackid="] {
        if let Some(start) = uri_lower.find(prefix) {
            let rest = &uri_lower[start + prefix.len()..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if end > 0 {
                return rest[..end].parse().ok();
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn run_udp_transfer<H: SessionHandler>(
    mode: SessionMode,
    client_addr: SocketAddr,
    local_addr: SocketAddr,
    media_info: MediaInfo,
    server_side: ServerSide,
    mut data_to_stream_rx: Receiver<InterleavedData>,
    app_handler: Arc<H>,
    path: String,
    video_sockets: Option<(UdpSocket, UdpSocket)>,
    audio_sockets: Option<(UdpSocket, UdpSocket)>,
    cancel: CancellationToken,
) -> Result<()> {
    let (video_rtp, video_rtcp) = video_sockets.map_or((None, None), |(r, c)| (Some(r), Some(c)));
    let (audio_rtp, audio_rtcp) = audio_sockets.map_or((None, None), |(r, c)| (Some(r), Some(c)));

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
                let socket = socket_or_bind(video_rtp, &client_addr, port).await?;
                spawn_udp_recv(socket, udp_route::VIDEO_RTP, tx.clone(), cancel.clone());
            }

            // Forward incoming audio RTP to the handler.
            if let Some(TransportInfo::Udp {
                rtp_recv_port: Some(port),
                ..
            }) = media_info.audio_transport
            {
                let socket = socket_or_bind(audio_rtp, &client_addr, port).await?;
                spawn_udp_recv(socket, udp_route::AUDIO_RTP, tx.clone(), cancel.clone());
            }

            // Forward outgoing RTP/RTCP back to the push client.
            // Bind to the local IP selected for the RTSP control connection so
            // UDP packets use the same server-facing interface on multi-homed
            // hosts. Each session binds one ephemeral send socket. For
            // deployments with many concurrent sessions, consider sharing
            // `Arc<UdpSocket>` across sessions (a single socket can `send_to`
            // any destination).
            let send_socket = UdpSocket::bind(net::bind_on_ip(local_addr.ip())).await?;

            let video_rtp_send = media_info.video_transport.as_ref().and_then(|t| {
                if let TransportInfo::Udp {
                    rtp_send_port: Some(port),
                    ..
                } = t
                {
                    Some(*port)
                } else {
                    None
                }
            });
            let video_rtcp_send = media_info.video_transport.as_ref().and_then(|t| {
                if let TransportInfo::Udp {
                    rtcp_send_port: Some(port),
                    ..
                } = t
                {
                    Some(*port)
                } else {
                    None
                }
            });
            let audio_rtp_send = media_info.audio_transport.as_ref().and_then(|t| {
                if let TransportInfo::Udp {
                    rtp_send_port: Some(port),
                    ..
                } = t
                {
                    Some(*port)
                } else {
                    None
                }
            });
            let audio_rtcp_send = media_info.audio_transport.as_ref().and_then(|t| {
                if let TransportInfo::Udp {
                    rtcp_send_port: Some(port),
                    ..
                } = t
                {
                    Some(*port)
                } else {
                    None
                }
            });

            let send_cancel = cancel.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = send_cancel.cancelled() => break,
                        maybe_frame = data_to_stream_rx.recv() => {
                            match maybe_frame {
                                Some((channel, data)) => {
                                    let send_port = match channel {
                                        udp_route::VIDEO_RTP => video_rtp_send,
                                        udp_route::VIDEO_RTCP => video_rtcp_send,
                                        udp_route::AUDIO_RTP => audio_rtp_send,
                                        udp_route::AUDIO_RTCP => audio_rtcp_send,
                                        _ => None,
                                    };
                                    if let Some(port) = send_port {
                                        let dest = SocketAddr::new(client_addr.ip(), port);
                                        if send_socket.send_to(&data, dest).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                None => break,
                            }
                        }
                    }
                }
            });

            spawn_rtcp_drain(video_rtcp, audio_rtcp, app_handler, path, cancel.clone());
        }
        SessionMode::Pull => {
            let ServerSide::Pull(mut rx, rtcp_tx) = server_side else {
                return Err(anyhow!("Unexpected server side for pull"));
            };

            // Bind to the local IP selected for the RTSP control connection
            // for correct source address selection on multi-homed hosts.
            let send_socket = UdpSocket::bind(net::bind_on_ip(local_addr.ip())).await?;

            let mut channel_map: HashMap<u8, u16> = HashMap::new();
            if let Some(TransportInfo::Udp {
                rtp_send_port: Some(port),
                rtcp_send_port: Some(rtcp_port),
                ..
            }) = media_info.video_transport
            {
                channel_map.insert(udp_route::VIDEO_RTP, port);
                channel_map.insert(udp_route::VIDEO_RTCP, rtcp_port);
            }
            if let Some(TransportInfo::Udp {
                rtp_send_port: Some(port),
                rtcp_send_port: Some(rtcp_port),
                ..
            }) = media_info.audio_transport
            {
                channel_map.insert(udp_route::AUDIO_RTP, port);
                channel_map.insert(udp_route::AUDIO_RTCP, rtcp_port);
            }

            let send_cancel = cancel.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = send_cancel.cancelled() => break,
                        maybe_frame = rx.recv() => {
                            match maybe_frame {
                                Some((channel, data)) => {
                                    if let Some(&port) = channel_map.get(&channel) {
                                        let dest = SocketAddr::new(client_addr.ip(), port);
                                        if send_socket.send_to(&data, dest).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                None => break,
                            }
                        }
                    }
                }
            });

            spawn_rtcp_channel_drain(video_rtcp, audio_rtcp, rtcp_tx, cancel.clone());
        }
        SessionMode::Mixed => return Err(anyhow!("session mode must be resolved")),
    }

    Ok(())
}

/// Spawn RTCP receiver tasks for active UDP sockets. Each task forwards
/// incoming RTCP packets to `app_handler.on_rtcp()`. Tasks exit when `cancel`
/// is cancelled (e.g. on TEARDOWN or control connection close).
fn spawn_rtcp_drain<H: SessionHandler>(
    video_rtcp: Option<UdpSocket>,
    audio_rtcp: Option<UdpSocket>,
    app_handler: Arc<H>,
    path: String,
    cancel: CancellationToken,
) {
    for socket in [video_rtcp, audio_rtcp].into_iter().flatten() {
        let app_handler = app_handler.clone();
        let path = path.clone();
        let rtcp_cancel = cancel.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                tokio::select! {
                    _ = rtcp_cancel.cancelled() => break,
                    result = socket.recv_from(&mut buf) => {
                        match result {
                            Ok((n, _)) => {
                                let data = buf[..n].to_vec();
                                if app_handler.on_rtcp(path.clone(), data).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }
}

/// Spawn RTCP receiver tasks for pull-mode UDP sockets and forward incoming
/// RTCP packets through the per-session endpoint channel.
fn spawn_rtcp_channel_drain(
    video_rtcp: Option<UdpSocket>,
    audio_rtcp: Option<UdpSocket>,
    tx: Sender<InterleavedData>,
    cancel: CancellationToken,
) {
    for (channel, socket) in [
        (udp_route::VIDEO_RTCP, video_rtcp),
        (udp_route::AUDIO_RTCP, audio_rtcp),
    ]
    .into_iter()
    .filter_map(|(channel, socket)| socket.map(|socket| (channel, socket)))
    {
        let tx = tx.clone();
        let rtcp_cancel = cancel.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                tokio::select! {
                    _ = rtcp_cancel.cancelled() => break,
                    result = socket.recv_from(&mut buf) => {
                        match result {
                            Ok((n, _)) => {
                                if tx.send((channel, buf[..n].to_vec())).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }
}

async fn bind_udp(addr: &SocketAddr, port: u16) -> Result<UdpSocket> {
    let bind_addr = net::bind_addr_for(addr, port);
    UdpSocket::bind(&bind_addr)
        .await
        .map_err(|e| anyhow!("Failed to bind UDP socket {}: {}", bind_addr, e))
}

/// Return the pre-allocated socket if available, otherwise bind a fresh one.
async fn socket_or_bind(
    pre_allocated: Option<UdpSocket>,
    addr: &SocketAddr,
    port: u16,
) -> Result<UdpSocket> {
    match pre_allocated {
        Some(s) => Ok(s),
        None => bind_udp(addr, port).await,
    }
}

/// Spawn a task that reads RTP packets from `socket` and forwards them to `tx`
/// with the given routing key. Dropped frames are expected under UDP semantics.
fn spawn_udp_recv(
    socket: UdpSocket,
    key: u8,
    tx: Sender<InterleavedData>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((n, _)) => {
                            match tx.try_send((key, buf[..n].to_vec())) {
                                Ok(()) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });
}

/// Decrements the active-connection counter when dropped, even if the spawned
/// session task panics.
pub(crate) struct ConnectionGuard(Arc<AtomicUsize>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// Start a single-port RTSP server that multiplexes sessions by URL path.
///
/// The server runs until `cancel` is cancelled. The application logic for each
/// stream is supplied by `handler`.
pub async fn setup_rtsp_server_with_handler<H>(
    listen_addr: &str,
    mode: SessionMode,
    handler: H,
    config: ServerConfig,
    cancel: CancellationToken,
) -> Result<()>
where
    H: SessionHandler,
{
    info!(
        "Setting up RTSP server: addr={}, mode={:?}, max_connections={}",
        listen_addr, mode, config.max_connections
    );

    let listener = TcpListener::bind(listen_addr).await?;
    let local_addr = listener.local_addr()?;
    info!("RTSP server listening on {}", local_addr);

    let handler = Arc::new(handler);
    let sessions: Arc<RwLock<HashMap<String, ServerSession>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let config = Arc::new(config);
    let active_connections = Arc::new(AtomicUsize::new(0));
    let mut connection_count = 0u64;

    // Derive the cleanup interval from the configured session timeout:
    // 1/4 of the timeout, clamped to 1–60 s.  This avoids busy-write-
    // locking the sessions map every 5 s when the timeout is 600 s, and
    // ensures short timeouts (e.g. 15 s) are serviced promptly.
    let cleanup_interval_secs = (config.session_timeout / 4).clamp(1, 60);

    let cleanup_sessions = sessions.clone();
    let cleanup_cancel = cancel.child_token();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(cleanup_interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = cleanup_cancel.cancelled() => return,
            }
            let mut sessions = cleanup_sessions.write().await;
            let now = std::time::Instant::now();
            sessions.retain(|id, session| {
                if session.is_expired(now) {
                    tracing::info!("Removing expired RTSP session: {}", id);
                    false
                } else {
                    true
                }
            });
        }
    });

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("RTSP server on {} shutting down", local_addr);
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((socket, addr)) => {
                        let local_addr = match socket.local_addr() {
                            Ok(addr) => addr,
                            Err(e) => {
                                warn!("Failed to read local address for RTSP client {}: {}", addr, e);
                                drop(socket);
                                continue;
                            }
                        };
                        let active = active_connections.load(Ordering::Acquire);
                        if active >= config.max_connections {
                            warn!(
                                "RTSP server at max connections ({}/{}), rejecting {}",
                                active, config.max_connections, addr
                            );
                            let mut socket = socket;
                            let response = b"RTSP/1.0 503 Service Unavailable\r\nCSeq: 0\r\nContent-Length: 0\r\n\r\n";
                            if let Err(e) = socket.write_all(response).await {
                                debug!("Failed to write 503 response to {}: {}", addr, e);
                            }
                            let _ = socket.flush().await;
                            drop(socket);
                            continue;
                        }

                        active_connections.fetch_add(1, Ordering::Release);
                        connection_count += 1;
                        let conn_id = connection_count;
                        let guard = ConnectionGuard(active_connections.clone());
                        info!("RTSP client #{} connected from {}", conn_id, addr);

                        let session = RtspServerSession::new(
                            socket,
                            addr,
                            local_addr,
                            sessions.clone(),
                            (*config).clone(),
                            mode,
                            handler.clone(),
                            cancel.clone(),
                        );

                        tokio::spawn(async move {
                            match session.handle_session(guard).await {
                                Ok(SessionResult::Established(media_info)) => {
                                    info!(
                                        "Connection #{} session established: {:?}",
                                        conn_id, media_info
                                    );
                                }
                                Ok(SessionResult::Teardown) => {
                                    info!("Connection #{} session torn down", conn_id);
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
