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
use crate::constants::{media_type, net};
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

    /// Called for every incoming RTCP packet (UDP RTCP port or TCP interleaved
    /// odd channel). The default implementation ignores it.
    async fn on_rtcp(&self, _path: String, _data: Vec<u8>) -> Result<()> {
        Ok(())
    }
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
    read_buffer: Vec<u8>,
    video_udp_sockets: Option<(UdpSocket, UdpSocket)>,
    audio_udp_sockets: Option<(UdpSocket, UdpSocket)>,
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
            read_buffer: Vec::with_capacity(8192),
            video_udp_sockets: None,
            audio_udp_sockets: None,
        }
    }

    pub async fn handle_session(mut self) -> Result<MediaInfo> {
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
                    if session_mode == SessionMode::Push {
                        return Err(anyhow!("DESCRIBE is not supported on a push session"));
                    }
                    if session_mode == SessionMode::Mixed {
                        session_mode = SessionMode::Pull;
                    }

                    let p = request_path(&request)?.unwrap_or_default();
                    let sdp = self.app_handler.on_describe(p.clone()).await?;
                    self.handler.set_sdp(sdp);
                    path = Some(p);
                    let response = self.handler.handle_describe(&request).await?;
                    self.send_response(&response).await?;
                }
                Method::Announce => {
                    if session_mode == SessionMode::Pull {
                        return Err(anyhow!("ANNOUNCE is not supported on a pull session"));
                    }
                    if session_mode == SessionMode::Mixed {
                        session_mode = SessionMode::Push;
                    }

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

                    let is_video = {
                        let sdp_bytes = self
                            .handler
                            .sdp_content()
                            .ok_or_else(|| anyhow!("No SDP"))?;
                        let sdp = sdp_types::Session::parse(sdp_bytes)?;
                        resolve_setup_media_kind(&uri, &sdp, video_channels, video_ports)
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
                    let response = match session_mode {
                        SessionMode::Pull => self.handler.handle_play(&request).await?,
                        SessionMode::Push => self.handler.handle_record(&request).await?,
                        SessionMode::Mixed => unreachable!(),
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
                        self.start_tcp_data_transfer(p, media_info.clone(), session_mode)
                            .await?;
                    } else {
                        self.start_udp_data_transfer(p, media_info.clone(), session_mode)
                            .await?;
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

    async fn start_tcp_data_transfer(
        self,
        path: String,
        media_info: MediaInfo,
        session_mode: SessionMode,
    ) -> Result<()> {
        let (data_from_stream_tx, mut data_from_stream_rx) = unbounded_channel::<InterleavedData>();
        let (data_to_stream_tx, data_to_stream_rx) = unbounded_channel::<InterleavedData>();

        let (endpoint, server_side) = match session_mode {
            SessionMode::Push => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Push(rx), ServerSide::Push(tx))
            }
            SessionMode::Pull => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Pull(tx), ServerSide::Pull(rx))
            }
            SessionMode::Mixed => unreachable!("session mode must be resolved"),
        };

        let app_handler = self.app_handler.clone();
        self.app_handler
            .on_session(path.clone(), session_mode, media_info, endpoint)
            .await?;

        let stream = self.stream;
        tokio::spawn(async move {
            if let Err(e) =
                handle_tcp_stream(stream, session_mode, data_from_stream_tx, data_to_stream_rx)
                    .await
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
                // Handle incoming RTCP/TEARDOWN frames so the read half stays alive.
                tokio::spawn(async move {
                    while let Some((channel, data)) = data_from_stream_rx.recv().await {
                        if channel % 2 != 0
                            && app_handler.on_rtcp(path.clone(), data).await.is_err()
                        {
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
    ) -> Result<()> {
        let (endpoint, server_side) = match session_mode {
            SessionMode::Push => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Push(rx), ServerSide::Push(tx))
            }
            SessionMode::Pull => {
                let (tx, rx) = unbounded_channel::<InterleavedData>();
                (SessionEndpoint::Pull(tx), ServerSide::Pull(rx))
            }
            SessionMode::Mixed => unreachable!("session mode must be resolved"),
        };

        let app_handler = self.app_handler.clone();
        self.app_handler
            .on_session(path.clone(), session_mode, media_info.clone(), endpoint)
            .await?;

        let client_addr = self.addr;
        let video_sockets = self.video_udp_sockets.take();
        let audio_sockets = self.audio_udp_sockets.take();
        tokio::spawn(async move {
            if let Err(e) = run_udp_transfer(
                session_mode,
                client_addr,
                media_info,
                server_side,
                app_handler,
                path,
                video_sockets,
                audio_sockets,
            )
            .await
            {
                error!("UDP transfer error: {}", e);
            }
        });

        // Keep the RTSP control connection alive for UDP sessions so that
        // clients (e.g. ffmpeg) do not see an unexpected EOF before TEARDOWN.
        // Minimal RTSP message handling: respond to OPTIONS/GET_PARAMETER and
        // honour TEARDOWN so clients can close cleanly.
        let stream = self.stream;
        tokio::spawn(async move {
            let (mut read_half, mut write_half) = tokio::io::split(stream);
            let mut buffer = Vec::with_capacity(4096);

            loop {
                match read_rtsp_message(&mut read_half, &mut buffer).await {
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

                        if request.method() == Method::Teardown {
                            break;
                        }
                    }
                    Ok(_) => break,
                    Err(_) => break,
                }
            }
            let _ = write_half.shutdown().await;
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

/// Read a complete RTSP message from `reader`, accumulating into `buffer`.
/// Consumed bytes are drained from `buffer` before the message is returned.
async fn read_rtsp_message<R>(reader: &mut R, buffer: &mut Vec<u8>) -> Result<Message<Vec<u8>>>
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

        let n = reader.read(&mut temp_buf).await?;
        if n == 0 {
            return Err(anyhow!("Connection closed"));
        }
        buffer.extend_from_slice(&temp_buf[..n]);
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

/// Resolve whether a SETUP request targets the video media.
///
/// Resolution order:
/// 1. Match against each media's `a=control` attribute.
/// 2. Explicit `video` / `audio` keywords in the URI.
/// 3. `streamid=N` / `trackID=N` refers to the N-th media in the SDP.
/// 4. Fallback based on the number of media and SETUP order.
fn resolve_setup_media_kind(
    uri: &str,
    sdp: &sdp_types::Session,
    video_channels: Option<(u8, u8)>,
    video_ports: Option<(u16, u16, u16, u16)>,
) -> bool {
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
                    uri_lower.ends_with(&control_lower)
                        || uri_lower.contains(&format!("/{}", control_lower))
                })
        });
        if matched {
            return is_video;
        }
    }

    // 2. Explicit video/audio keywords in the final path segment only, so a
    // stream ID like "myvideo" does not confuse audio-only streams.
    if let Some(segment) = last_path_segment(&uri_lower) {
        if segment == media_type::VIDEO {
            return true;
        }
        if segment == media_type::AUDIO {
            return false;
        }
    }

    // 3. streamid=N / trackID=N
    if let Some(index) = parse_track_index(uri)
        && let Some(media) = sdp.medias.get(index)
    {
        return media.media == media_type::VIDEO;
    }

    // 4. Fallback.
    let has_video = sdp.medias.iter().any(|m| m.media == media_type::VIDEO);
    let has_audio = sdp.medias.iter().any(|m| m.media == media_type::AUDIO);
    match (has_video, has_audio) {
        (true, false) => true,
        (false, true) => false,
        (true, true) => video_channels.is_none() && video_ports.is_none(),
        (false, false) => true,
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
async fn run_udp_transfer(
    mode: SessionMode,
    client_addr: SocketAddr,
    media_info: MediaInfo,
    server_side: ServerSide,
    app_handler: Arc<dyn SessionHandler>,
    path: String,
    video_sockets: Option<(UdpSocket, UdpSocket)>,
    audio_sockets: Option<(UdpSocket, UdpSocket)>,
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
                let socket = match video_rtp {
                    Some(rtp) => rtp,
                    None => bind_udp(&client_addr, port).await?,
                };
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
                let socket = match audio_rtp {
                    Some(rtp) => rtp,
                    None => bind_udp(&client_addr, port).await?,
                };
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

            drain_rtcp_ports(video_rtcp, audio_rtcp, app_handler, path).await?;
        }
        SessionMode::Pull => {
            let ServerSide::Pull(mut rx) = server_side else {
                return Err(anyhow!("Unexpected server side for pull"));
            };

            let send_socket = UdpSocket::bind(net::bind_any_for(&client_addr)).await?;

            let mut channel_map: HashMap<u8, u16> = HashMap::new();
            if let Some(TransportInfo::Udp {
                rtp_send_port: Some(port),
                rtcp_send_port: Some(rtcp_port),
                ..
            }) = media_info.video_transport
            {
                channel_map.insert(0, port);
                channel_map.insert(1, rtcp_port);
            }
            if let Some(TransportInfo::Udp {
                rtp_send_port: Some(port),
                rtcp_send_port: Some(rtcp_port),
                ..
            }) = media_info.audio_transport
            {
                channel_map.insert(2, port);
                channel_map.insert(3, rtcp_port);
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

            drain_rtcp_ports(video_rtcp, audio_rtcp, app_handler, path).await?;
        }
        SessionMode::Mixed => unreachable!("session mode must be resolved"),
    }

    Ok(())
}

async fn drain_rtcp_ports(
    video_rtcp: Option<UdpSocket>,
    audio_rtcp: Option<UdpSocket>,
    app_handler: Arc<dyn SessionHandler>,
    path: String,
) -> Result<()> {
    for socket in [video_rtcp, audio_rtcp].into_iter().flatten() {
        let app_handler = app_handler.clone();
        let path = path.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            while let Ok((n, _)) = socket.recv_from(&mut buf).await {
                let data = buf[..n].to_vec();
                if app_handler.on_rtcp(path.clone(), data).await.is_err() {
                    break;
                }
            }
        });
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
    handler: H,
    cancel: CancellationToken,
) -> Result<()>
where
    H: SessionHandler,
{
    info!(
        "Setting up RTSP server: addr={}, mode={:?}",
        listen_addr, mode
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
                            match session.handle_session().await {
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
