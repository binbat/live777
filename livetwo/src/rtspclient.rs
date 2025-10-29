use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use cli::{Codec, codec_from_str};
use md5::{Digest, Md5};
use portpicker::pick_unused_port;
use rtsp_types::{
    Message, Method, Request, Response, StatusCode, Url, Version, headers,
    headers::{HeaderValue, WWW_AUTHENTICATE, transport},
};
use sdp_types::Session;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::TcpStream,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    time::{self, Duration},
};
use tracing::{debug, error, info, trace, warn};

const USER_AGENT: &str = "whipinto";
type RtspSender = UnboundedSender<(u8, Vec<u8>)>;
type RtspReceiver = UnboundedReceiver<(u8, Vec<u8>)>;

#[derive(Clone, Debug)]
pub enum RtspMode {
    Pull,
    Push,
}

impl RtspMode {
    fn transport_mode(&self) -> Option<transport::TransportMode> {
        match self {
            RtspMode::Pull => None,
            RtspMode::Push => Some(transport::TransportMode::Record),
        }
    }
}

pub struct RtspChannels {
    pub recv_tx: UnboundedSender<(u8, Vec<u8>)>,
    pub recv_rx: Option<UnboundedReceiver<(u8, Vec<u8>)>>,

    pub send_tx: UnboundedSender<(u8, Vec<u8>)>,
    pub send_rx: Option<UnboundedReceiver<(u8, Vec<u8>)>>,
}

impl RtspChannels {
    pub fn new() -> Self {
        let (recv_tx, recv_rx) = unbounded_channel::<(u8, Vec<u8>)>();
        let (send_tx, send_rx) = unbounded_channel::<(u8, Vec<u8>)>();

        Self {
            recv_tx,
            recv_rx: Some(recv_rx),
            send_tx,
            send_rx: Some(send_rx),
        }
    }
    pub fn get_channels(&mut self, mode: RtspMode) -> (RtspSender, RtspReceiver) {
        match mode {
            RtspMode::Pull => {
                let send_rx = self.send_rx.take().expect("send_rx already taken");
                (self.recv_tx.clone(), send_rx)
            }
            RtspMode::Push => {
                let recv_rx = self.recv_rx.take().expect("recv_rx already taken");
                (self.send_tx.clone(), recv_rx)
            }
        }
    }

    pub fn get_internal_rx(&mut self, mode: &RtspMode) -> RtspReceiver {
        match mode {
            RtspMode::Pull => self.recv_rx.take().expect("recv_rx already taken"),
            RtspMode::Push => self.send_rx.take().expect("send_rx already taken"),
        }
    }
}

#[derive(Clone)]
struct AuthParams {
    username: String,
    password: String,
}

struct RtspSession<T> {
    stream: T,
    url: String,
    cseq: u32,
    auth_params: AuthParams,
    session_id: Option<String>,
    rtp_client_port: Option<u16>,
    auth_header: Option<HeaderValue>,
}

impl<T> RtspSession<T>
where
    T: AsyncReadExt + AsyncWriteExt + Unpin,
{
    async fn send_request(&mut self, request: &Request<Vec<u8>>) -> Result<()> {
        let mut buffer = Vec::new();
        request.write(&mut buffer)?;
        self.stream.write_all(&buffer).await?;
        Ok(())
    }

    async fn read_response(&mut self) -> Result<Response<Vec<u8>>> {
        let mut buffer = vec![0; 4096];
        let n = self.stream.read(&mut buffer).await?;
        let (message, _) = Message::parse(&buffer[..n])?;
        match message {
            Message::Response(response) => Ok(response),
            _ => Err(anyhow!("Expected a response message")),
        }
    }

    fn generate_digest_response(&self, realm: &str, nonce: &str, method: &str) -> String {
        generate_digest_response(
            &self.auth_params.username,
            &self.auth_params.password,
            &self.url,
            realm,
            nonce,
            method,
        )
    }

    fn generate_authorization_header(&self, realm: &str, nonce: &str, method: &Method) -> String {
        let method_str = match method {
            Method::Options => "OPTIONS",
            Method::Describe => "DESCRIBE",
            Method::Setup => "SETUP",
            Method::Play => "PLAY",
            Method::Record => "RECORD",
            Method::Teardown => "TEARDOWN",
            Method::Announce => "ANNOUNCE",
            _ => "UNKNOWN",
        };

        let response = self.generate_digest_response(realm, nonce, method_str);
        format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
            self.auth_params.username, realm, nonce, self.url, response
        )
    }

    fn parse_auth(header_value: &HeaderValue) -> Result<(String, String)> {
        let header_str = header_value.as_str();
        let realm_key = "realm=\"";
        let nonce_key = "nonce=\"";

        let parse_value = |key: &str| -> Result<String> {
            let start = header_str
                .find(key)
                .ok_or_else(|| anyhow!("{} not found", key))?
                + key.len();
            let end = header_str[start..]
                .find('"')
                .ok_or_else(|| anyhow!("end not found for {}", key))?
                + start;
            Ok(header_str[start..end].to_string())
        };

        Ok((parse_value(realm_key)?, parse_value(nonce_key)?))
    }

    async fn handle_unauthorized(
        &mut self,
        method: Method,
        auth_header: &HeaderValue,
    ) -> Result<Response<Vec<u8>>> {
        let (realm, nonce) = Self::parse_auth(auth_header)?;
        let auth_header_value = self.generate_authorization_header(&realm, &nonce, &method);
        let auth_request = Request::builder(method, Version::V1_0)
            .request_uri(
                self.url
                    .parse::<Url>()
                    .map_err(|_| anyhow!("Invalid URI"))?,
            )
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::USER_AGENT, USER_AGENT)
            .header(headers::AUTHORIZATION, auth_header_value)
            .empty();
        self.send_request(&auth_request.map_body(|_| vec![]))
            .await?;
        let response = self.read_response().await?;
        self.cseq += 1;
        Ok(response)
    }

    async fn send_options_request(&mut self) -> Result<()> {
        let options_request = Request::builder(Method::Options, Version::V1_0)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        self.send_request(&options_request.map_body(|_| vec![]))
            .await?;
        self.read_response().await?;
        self.cseq += 1;
        Ok(())
    }

    async fn send_announce_request(&mut self, sdp: String) -> Result<()> {
        let announce_request = Request::builder(Method::Announce, Version::V1_0)
            .request_uri(
                self.url
                    .parse::<Url>()
                    .map_err(|_| anyhow!("Invalid URI"))?,
            )
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::CONTENT_TYPE, "application/sdp")
            .header(headers::USER_AGENT, USER_AGENT)
            .build(sdp.into_bytes());

        self.send_request(&announce_request).await?;
        let announce_response = self.read_response().await?;
        self.cseq += 1;

        if announce_response.status() == StatusCode::Unauthorized {
            if let Some(auth_header) = announce_response.header(&WWW_AUTHENTICATE).cloned() {
                let announce_response = self
                    .handle_unauthorized(Method::Announce, &auth_header)
                    .await?;
                if announce_response.status() != StatusCode::Ok {
                    return Err(anyhow!("ANNOUNCE request failed after authentication"));
                }
            } else {
                return Err(anyhow!(
                    "ANNOUNCE request failed with 401 Unauthorized and no WWW-Authenticate header"
                ));
            }
        } else if announce_response.status() != StatusCode::Ok {
            return Err(anyhow!("ANNOUNCE request failed"));
        }

        Ok(())
    }

    async fn send_describe_request(&mut self) -> Result<String> {
        let describe_request = Request::builder(Method::Describe, Version::V1_0)
            .request_uri(
                self.url
                    .parse::<Url>()
                    .map_err(|_| anyhow!("Invalid URI"))?,
            )
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::ACCEPT, "application/sdp")
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        self.send_request(&describe_request.map_body(|_| vec![]))
            .await?;
        let mut describe_response = self.read_response().await?;
        self.cseq += 1;

        if describe_response.status() == StatusCode::Unauthorized
            && let Some(auth_header) = describe_response.header(&WWW_AUTHENTICATE).cloned()
        {
            describe_response = self
                .handle_unauthorized(Method::Describe, &auth_header)
                .await?;
        }

        let sdp_content = String::from_utf8_lossy(describe_response.body()).to_string();
        if sdp_content.is_empty() {
            return Err(anyhow!("Received empty SDP content"));
        }

        Ok(sdp_content)
    }

    async fn send_setup_request(
        &mut self,
        transport_mode: Option<transport::TransportMode>,
    ) -> Result<(String, u16)> {
        let rtp_client_port = self
            .rtp_client_port
            .ok_or_else(|| anyhow!("RTP server port not set"))?;
        debug!("Using RTP client port: {}", rtp_client_port);

        let mut transport_params = transport::RtpTransportParameters {
            unicast: true,
            client_port: Some((rtp_client_port, Some(rtp_client_port + 1))),
            ..Default::default()
        };

        if let Some(mode) = transport_mode {
            transport_params.mode.push(mode);
        }

        let mut setup_request_builder = Request::builder(Method::Setup, Version::V1_0)
            .request_uri(
                self.url
                    .parse::<Url>()
                    .map_err(|_| anyhow!("Invalid URI"))?,
            )
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::USER_AGENT, USER_AGENT)
            .typed_header(&transport::Transports::from(vec![
                transport::Transport::Rtp(transport::RtpTransport {
                    profile: transport::RtpProfile::Avp,
                    lower_transport: None,
                    params: transport_params,
                }),
            ]));

        debug!(
            "Preparing SETUP request for URI: {}, RTSP client port: {}-{}",
            self.url,
            rtp_client_port,
            rtp_client_port + 1
        );

        if let Some(auth_header) = &self.auth_header {
            debug!("Adding AUTHORIZATION header to SETUP request");
            let (realm, nonce) = Self::parse_auth(auth_header)?;
            let auth_header_value =
                self.generate_authorization_header(&realm, &nonce, &Method::Setup);
            setup_request_builder =
                setup_request_builder.header(headers::AUTHORIZATION, auth_header_value);
        } else {
            debug!("No AUTHORIZATION header required for SETUP request");
        }

        if let Some(session_id) = &self.session_id {
            debug!("Adding SESSION header with ID: {}", session_id);
            setup_request_builder =
                setup_request_builder.header(headers::SESSION, session_id.as_str());
        }

        let setup_request = setup_request_builder.empty();
        debug!("SETUP request constructed: {:?}", setup_request);

        self.send_request(&setup_request.map_body(|_| vec![]))
            .await?;
        debug!("SETUP request sent successfully");

        let setup_response = self.read_response().await?;
        self.cseq += 1;
        debug!("Received SETUP response: {:?}", setup_response);

        if setup_response.status() == StatusCode::Unauthorized {
            error!("SETUP request returned 401 Unauthorized");
            if let Some(auth_header) = setup_response.header(&WWW_AUTHENTICATE).cloned() {
                info!("Handling unauthorized response, retrying with authentication...");
                let setup_response = self
                    .handle_unauthorized(Method::Setup, &auth_header)
                    .await?;
                if setup_response.status() != StatusCode::Ok {
                    error!("SETUP request failed after authentication");
                    return Err(anyhow!("SETUP request failed after authentication"));
                }
            } else {
                error!("401 Unauthorized response but no WWW-Authenticate header found");
                return Err(anyhow!(
                    "SETUP request failed with 401 Unauthorized and no WWW-Authenticate header"
                ));
            }
        } else if setup_response.status() != StatusCode::Ok {
            error!(
                "SETUP request failed with status: {}",
                setup_response.status()
            );
            return Err(anyhow!("SETUP request failed"));
        }
        info!("SETUP request succeeded");

        let session_id = setup_response
            .header(&headers::SESSION)
            .ok_or_else(|| anyhow!("Session header not found"))?
            .as_str()
            .split(';')
            .next()
            .ok_or_else(|| anyhow!("Failed to parse session ID"))?
            .to_string();
        debug!("Extracted session ID: {}", session_id);

        let transport_header = setup_response
            .header(&headers::TRANSPORT)
            .ok_or_else(|| anyhow!("Transport header not found"))?
            .as_str();
        debug!("Received transport header: {}", transport_header);

        let server_port = transport_header
            .split(';')
            .find_map(|part| part.strip_prefix("server_port="))
            .and_then(|server_port_str| server_port_str.split('-').next())
            .ok_or_else(|| anyhow!("server_port not found in transport header"))?
            .parse::<u16>()
            .map_err(|_| anyhow!("Failed to parse server port"))?;
        debug!(
            "Extracted server port from transport header: {}",
            server_port
        );

        debug!(
            "SETUP request completed. Session ID: {}, Server Port: {}",
            session_id, server_port
        );

        Ok((session_id, server_port))
    }

    async fn send_tcp_setup_request(
        &mut self,
        rtp_channel: u8,
        rtcp_channel: u8,
        transport_mode: Option<transport::TransportMode>,
    ) -> Result<String> {
        debug!(
            "Setting up TCP transport with channels RTP: {}, RTCP: {}",
            rtp_channel, rtcp_channel
        );

        let mut transport_params = transport::RtpTransportParameters {
            unicast: true,
            interleaved: Some((rtp_channel, Some(rtcp_channel))),
            ..Default::default()
        };

        if let Some(mode) = transport_mode {
            transport_params.mode.push(mode);
        }

        let mut setup_request_builder = Request::builder(Method::Setup, Version::V1_0)
            .request_uri(
                self.url
                    .parse::<Url>()
                    .map_err(|_| anyhow!("Invalid URI"))?,
            )
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::USER_AGENT, USER_AGENT)
            .typed_header(&transport::Transports::from(vec![
                transport::Transport::Rtp(transport::RtpTransport {
                    profile: transport::RtpProfile::Avp,
                    lower_transport: Some(transport::RtpLowerTransport::Tcp),
                    params: transport_params,
                }),
            ]));

        if let Some(session_id) = &self.session_id {
            setup_request_builder =
                setup_request_builder.header(headers::SESSION, session_id.as_str());
        }

        if let Some(auth_header) = &self.auth_header {
            let (realm, nonce) = Self::parse_auth(auth_header)?;
            let auth_header_value =
                self.generate_authorization_header(&realm, &nonce, &Method::Setup);
            setup_request_builder =
                setup_request_builder.header(headers::AUTHORIZATION, auth_header_value);
        }

        let setup_request = setup_request_builder.empty();
        debug!("TCP SETUP request constructed: {:?}", setup_request);

        self.send_request(&setup_request.map_body(|_| vec![]))
            .await?;
        let setup_response = self.read_response().await?;
        self.cseq += 1;
        debug!("Received TCP SETUP response: {:?}", setup_response);

        if setup_response.status() == StatusCode::Unauthorized {
            error!("TCP SETUP request returned 401 Unauthorized");
            if let Some(auth_header) = setup_response.header(&WWW_AUTHENTICATE).cloned() {
                info!("Handling unauthorized response, retrying with authentication...");
                let setup_response = self
                    .handle_unauthorized(Method::Setup, &auth_header)
                    .await?;
                if setup_response.status() != StatusCode::Ok {
                    error!("TCP SETUP request failed after authentication");
                    return Err(anyhow!("TCP SETUP request failed after authentication"));
                }
            } else {
                error!("401 Unauthorized response but no WWW-Authenticate header found");
                return Err(anyhow!(
                    "TCP SETUP request failed with 401 Unauthorized and no WWW-Authenticate header"
                ));
            }
        } else if setup_response.status() != StatusCode::Ok {
            error!(
                "TCP SETUP request failed with status: {}",
                setup_response.status()
            );
            return Err(anyhow!("TCP SETUP request failed"));
        }

        let session_id = setup_response
            .header(&headers::SESSION)
            .ok_or_else(|| anyhow!("Session header not found"))?
            .as_str()
            .split(';')
            .next()
            .ok_or_else(|| anyhow!("Failed to parse session ID"))?
            .to_string();

        debug!(
            "TCP SETUP completed successfully. Session ID: {}",
            session_id
        );
        Ok(session_id)
    }
}
pub struct RtspTcpHandler {
    mode: RtspMode,
    channels: RtspChannels,
}

impl RtspTcpHandler {
    pub fn new(mode: RtspMode) -> Self {
        Self {
            mode,
            channels: RtspChannels::new(),
        }
    }
    pub async fn start(&mut self, stream: TcpStream) -> Result<()> {
        let (reader, writer) = tokio::io::split(stream);

        let rx = self.channels.get_internal_rx(&self.mode);
        let reader_task = self.handle_read(reader);

        let writer_task = self.handle_write(writer, rx);

        let (reader_result, writer_result) = tokio::join!(reader_task, writer_task);

        if let Err(e) = reader_result {
            error!("RTSP Reader task failed: {}", e);
        } else {
            debug!("RTSP Reader task completed");
        }

        if let Err(e) = writer_result {
            error!("RTSP Writer task failed: {}", e);
        } else {
            debug!("RTSP Writer task completed");
        }

        Ok(())
    }

    async fn handle_read<R>(&self, reader: R) -> Result<()>
    where
        R: AsyncReadExt + Unpin,
    {
        let mut reader = BufReader::new(reader);
        let mut buffer = vec![0u8; 8192];
        let mut accumulated_buf = Vec::new();
        let recv_tx = self.channels.recv_tx.clone();

        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    warn!("RTSP Connection closed by remote peer");
                    return Ok(());
                }
                Ok(n) => {
                    accumulated_buf.extend_from_slice(&buffer[..n]);

                    while accumulated_buf.len() >= 4 && accumulated_buf[0] == b'$' {
                        let channel = accumulated_buf[1];
                        let length =
                            ((accumulated_buf[2] as usize) << 8) | (accumulated_buf[3] as usize);

                        if accumulated_buf.len() < 4 + length {
                            break;
                        }

                        let data = accumulated_buf[4..4 + length].to_vec();
                        trace!(
                            "RTSP Received interleaved data on channel {}, {} bytes",
                            channel,
                            data.len()
                        );

                        if let Err(e) = recv_tx.send((channel, data)) {
                            error!("RTSP Failed to forward interleaved data: {}", e);
                            return Ok(());
                        }

                        accumulated_buf.drain(..4 + length);
                    }

                    if !accumulated_buf.is_empty() && accumulated_buf[0] != b'$' {
                        match Message::<Vec<u8>>::parse(&accumulated_buf) {
                            Ok((message, consumed)) => {
                                debug!("Received RTSP message: {:?}", message);
                                accumulated_buf.drain(..consumed);
                                match message {
                                    Message::Response(response) => {
                                        debug!("Processing response: {:?}", response.status());
                                    }
                                    Message::Request(request) => {
                                        debug!(
                                            "Received unexpected request: {:?}",
                                            request.method()
                                        );
                                    }
                                    Message::Data(_) => {
                                        debug!("Received data message, ignoring");
                                    }
                                }
                            }
                            Err(rtsp_types::ParseError::Incomplete(_)) => {}
                            Err(e) => {
                                error!("Failed to parse RTSP message: {:?}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Error reading from socket: {}", e);
                    return Err(anyhow!("Socket read error: {}", e));
                }
            }
        }
    }

    async fn handle_write<W>(
        &self,
        writer: W,
        mut rx: UnboundedReceiver<(u8, Vec<u8>)>,
    ) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
    {
        let mut writer = BufWriter::new(writer);

        while let Some((channel, data)) = rx.recv().await {
            trace!("Sending data on channel {}, {} bytes", channel, data.len());

            let mut frame = vec![
                b'$',
                channel,
                ((data.len() >> 8) & 0xFF) as u8,
                (data.len() & 0xFF) as u8,
            ];
            frame.extend_from_slice(&data);

            if let Err(e) = writer.write_all(&frame).await {
                error!("Failed to send data: {}", e);
                break;
            }

            if let Err(e) = writer.flush().await {
                error!("Failed to flush data: {}", e);
                break;
            }
        }

        Ok(())
    }

    pub fn get_channels(&mut self) -> (RtspSender, RtspReceiver) {
        self.channels.get_channels(self.mode.clone())
    }
}

fn find_control_attribute(track: &sdp_types::Media, base_url: &str, track_id: &str) -> String {
    track
        .attributes
        .iter()
        .find_map(|attr| {
            if attr.attribute == "control" {
                let value = attr.value.clone().unwrap_or_default();
                if value.starts_with("rtsp://") {
                    Some(value)
                } else {
                    Some(format!("{base_url}/{value}"))
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| format!("{base_url}/trackID={track_id}"))
}

fn extract_codec_from_track(track: &sdp_types::Media) -> Option<Codec> {
    track.attributes.iter().find_map(|attr| {
        if attr.attribute == "rtpmap" {
            let value = attr.value.as_ref()?;
            let codec_name = value
                .split_whitespace()
                .nth(1)?
                .split('/')
                .next()?
                .to_string();
            codec_from_str(&codec_name).ok()
        } else {
            None
        }
    })
}

pub async fn setup_rtsp_session(
    rtsp_url: &str,
    sdp_content: Option<String>,
    target_host: &str,
    mode: RtspMode,
) -> Result<(
    rtsp::MediaInfo,
    Option<UnboundedSender<(u8, Vec<u8>)>>,
    Option<UnboundedReceiver<(u8, Vec<u8>)>>,
)> {
    let mut url = Url::parse(rtsp_url).map_err(|e| anyhow!("Invalid RTSP URL: {}", e))?;
    info!("Parsed RTSP URL: {}", rtsp_url);

    let use_tcp = url
        .query_pairs()
        .any(|(k, v)| (k == "transport" || k == "trans") && v == "tcp")
        || rtsp_url.contains("rtp/tcp");
    info!(
        "Using transport mode: {}",
        if use_tcp { "TCP" } else { "UDP" }
    );

    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("Invalid RTSP URL: no port specified"))?;
    let addr = format!("{target_host}:{port}");
    let base_url = url.as_str().to_string();

    let stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| anyhow!("Failed to connect to RTSP server: {}", e))?;
    info!("Connected to RTSP server: {}", addr);

    let mut rtsp_session = RtspSession {
        stream,
        url: base_url.clone(),
        cseq: 1,
        auth_params: AuthParams {
            username: url.username().to_string(),
            password: url.password().unwrap_or("").to_string(),
        },
        session_id: None,
        rtp_client_port: None,
        auth_header: None,
    };

    url.set_username("").unwrap();
    url.set_password(None).unwrap();

    rtsp_session
        .send_options_request()
        .await
        .map_err(|e| anyhow!("OPTIONS request failed: {}", e))?;
    info!("OPTIONS request successful");

    let sdp: Session = match mode {
        RtspMode::Pull => {
            let sdp_content = rtsp_session
                .send_describe_request()
                .await
                .map_err(|e| anyhow!("DESCRIBE request failed: {}", e))?;
            info!("DESCRIBE request successful");
            Session::parse(sdp_content.as_bytes())
                .map_err(|e| anyhow!("Failed to parse SDP: {}", e))?
        }
        RtspMode::Push => {
            let sdp_content =
                sdp_content.ok_or_else(|| anyhow!("SDP content required for push mode"))?;
            debug!("SDP content: {}", sdp_content);
            rtsp_session
                .send_announce_request(sdp_content.clone())
                .await
                .map_err(|e| anyhow!("ANNOUNCE request failed: {}", e))?;
            info!("ANNOUNCE request successful");
            Session::parse(sdp_content.as_bytes())
                .map_err(|e| anyhow!("Failed to parse SDP: {}", e))?
        }
    };
    debug!("Parsed SDP: {:?}", sdp);

    let video_track = sdp.medias.iter().find(|md| md.media == "video");
    let audio_track = sdp.medias.iter().find(|md| md.media == "audio");
    debug!(
        "Found video track: {}, audio track: {}",
        video_track.is_some(),
        audio_track.is_some()
    );

    if video_track.is_none() && audio_track.is_none() {
        error!("No tracks found in SDP");
        return Err(anyhow!("No tracks found in SDP"));
    }

    let mut media_info = rtsp::MediaInfo::default();
    let transport_mode = mode.transport_mode();

    if let Some(video_track) = video_track {
        let video_url = find_control_attribute(video_track, &base_url, "0");
        debug!("Video track URL: {}", video_url);
        rtsp_session.url = video_url;

        if use_tcp {
            let video_rtp_channel = 0;
            let video_rtcp_channel = 1;
            let session_id = rtsp_session
                .send_tcp_setup_request(
                    video_rtp_channel,
                    video_rtcp_channel,
                    transport_mode.clone(),
                )
                .await
                .map_err(|e| anyhow!("Video SETUP request failed: {}", e))?;
            rtsp_session.session_id = Some(session_id);
            let codec = extract_codec_from_track(video_track);
            media_info.video_transport = Some(rtsp::TransportInfo::Tcp {
                rtp_channel: video_rtp_channel,
                rtcp_channel: video_rtcp_channel,
            });
            media_info.video_codec = codec;

            if let Some(codec) = codec {
                match codec {
                    cli::Codec::H264 => {
                        if let Some(params) = extract_h264_params(video_track) {
                            let sps_len = params.sps.len();
                            let pps_len = params.pps.len();

                            media_info.video_params = Some(rtsp::VideoCodecParams::H264 {
                                sps: params.sps,
                                pps: params.pps,
                            });

                            trace!(
                                "Extracted H.264 params from RTSP - SPS: {} bytes, PPS: {} bytes",
                                sps_len, pps_len
                            );
                        }
                    }
                    cli::Codec::H265 => {
                        if let Some(params) = extract_h265_params(video_track) {
                            let vps_len = params.vps.len();
                            let sps_len = params.sps.len();
                            let pps_len = params.pps.len();

                            media_info.video_params = Some(rtsp::VideoCodecParams::H265 {
                                vps: params.vps,
                                sps: params.sps,
                                pps: params.pps,
                            });

                            trace!(
                                "Extracted H.265 params from RTSP - VPS: {} bytes, SPS: {} bytes, PPS: {} bytes",
                                vps_len, sps_len, pps_len
                            );
                        }
                    }
                    _ => {}
                }
            }
        } else {
            let (rtp_client, rtcp_client, rtp_server, rtcp_server, codec) =
                setup_track(&mut rtsp_session, video_track, "0", &base_url).await?;
            media_info.video_transport = Some(rtsp::TransportInfo::Udp {
                rtp_send_port: rtp_server,
                rtp_recv_port: rtp_client,
                rtcp_send_port: rtcp_server,
                rtcp_recv_port: rtcp_client,
            });
            media_info.video_codec = codec;
            if let Some(codec) = codec {
                match codec {
                    cli::Codec::H264 => {
                        if let Some(params) = extract_h264_params(video_track) {
                            let sps_len = params.sps.len();
                            let pps_len = params.pps.len();

                            media_info.video_params = Some(rtsp::VideoCodecParams::H264 {
                                sps: params.sps,
                                pps: params.pps,
                            });

                            trace!(
                                "Extracted H.264 params from RTSP - SPS: {} bytes, PPS: {} bytes",
                                sps_len, pps_len
                            );
                        }
                    }
                    cli::Codec::H265 => {
                        if let Some(params) = extract_h265_params(video_track) {
                            let vps_len = params.vps.len();
                            let sps_len = params.sps.len();
                            let pps_len = params.pps.len();

                            media_info.video_params = Some(rtsp::VideoCodecParams::H265 {
                                vps: params.vps,
                                sps: params.sps,
                                pps: params.pps,
                            });

                            trace!(
                                "Extracted H.265 params from RTSP - VPS: {} bytes, SPS: {} bytes, PPS: {} bytes",
                                vps_len, sps_len, pps_len
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(audio_track) = audio_track {
        let audio_url = find_control_attribute(audio_track, &base_url, "1");
        debug!("Audio track URL: {}", audio_url);
        rtsp_session.url = audio_url;

        if use_tcp {
            let audio_rtp_channel = 2;
            let audio_rtcp_channel = 3;
            let session_id = rtsp_session.session_id.clone().unwrap_or_else(String::new);
            if session_id.is_empty() {
                let session_id = rtsp_session
                    .send_tcp_setup_request(audio_rtp_channel, audio_rtcp_channel, transport_mode)
                    .await
                    .map_err(|e| anyhow!("Audio SETUP request failed: {}", e))?;
                rtsp_session.session_id = Some(session_id);
            } else {
                rtsp_session
                    .send_tcp_setup_request(audio_rtp_channel, audio_rtcp_channel, transport_mode)
                    .await
                    .map_err(|e| anyhow!("Audio SETUP request failed: {}", e))?;
            }
            let codec = extract_codec_from_track(audio_track);
            media_info.audio_transport = Some(rtsp::TransportInfo::Tcp {
                rtp_channel: audio_rtp_channel,
                rtcp_channel: audio_rtcp_channel,
            });
            media_info.audio_codec = codec;
        } else {
            let (rtp_client, rtcp_client, rtp_server, rtcp_server, codec) =
                setup_track(&mut rtsp_session, audio_track, "1", &base_url).await?;
            media_info.audio_transport = Some(rtsp::TransportInfo::Udp {
                rtp_send_port: rtp_server,
                rtp_recv_port: rtp_client,
                rtcp_send_port: rtcp_server,
                rtcp_recv_port: rtcp_client,
            });
            media_info.audio_codec = codec;
        }
    }

    rtsp_session.url = base_url;
    let method = match mode {
        RtspMode::Pull => Method::Play,
        RtspMode::Push => Method::Record,
    };
    let request = Request::builder(method.clone(), Version::V1_0)
        .request_uri(
            rtsp_session
                .url
                .parse::<Url>()
                .map_err(|_| anyhow!("Invalid URI"))?,
        )
        .header(headers::CSEQ, rtsp_session.cseq.to_string())
        .header(headers::USER_AGENT, USER_AGENT)
        .header(
            headers::SESSION,
            rtsp_session.session_id.as_ref().unwrap().as_str(),
        )
        .empty();

    rtsp_session
        .send_request(&request.map_body(|_| vec![]))
        .await
        .map_err(|e| anyhow!("request failed: {}", e))?;

    let mut response = rtsp_session
        .read_response()
        .await
        .map_err(|e| anyhow!("Failed to read response: {}", e))?;
    rtsp_session.cseq += 1;

    if response.status() == StatusCode::Unauthorized {
        if let Some(auth_header) = response.header(&headers::WWW_AUTHENTICATE).cloned() {
            debug!("Handling unauthorized response");
            response = rtsp_session
                .handle_unauthorized(method, &auth_header)
                .await
                .map_err(|e| anyhow!("Authentication failed: {}", e))?;
            if response.status() != StatusCode::Ok {
                error!(
                    "request failed after authentication: {:?}",
                    response.status()
                );
                return Err(anyhow!(
                    "request failed after authentication: {:?}",
                    response.status()
                ));
            }
        } else {
            error!("request failed with 401 Unauthorized and no WWW-Authenticate header");
            return Err(anyhow!(
                "request failed with 401 Unauthorized and no WWW-Authenticate header"
            ));
        }
    } else if response.status() != StatusCode::Ok {
        error!("request failed with status: {:?}", response.status());
        return Err(anyhow!(
            "request failed with status: {:?}",
            response.status()
        ));
    }

    let session_id = rtsp_session
        .session_id
        .clone()
        .ok_or_else(|| anyhow!("Missing session ID after SETUP"))?;
    let mut rtsp_session_clone = rtsp_session;

    if use_tcp {
        info!("TCP transport mode enabled, setting up interleaved data handling");
        let mut tcp_handler = RtspTcpHandler::new(mode.clone());
        let (media_info, sender, receiver) = match mode {
            RtspMode::Pull => {
                let (rtsp_to_whip_tx, rtsp_to_whip_rx) = unbounded_channel::<(u8, Vec<u8>)>();
                let (whip_to_rtsp_tx, whip_to_rtsp_rx) = unbounded_channel::<(u8, Vec<u8>)>();

                tcp_handler.channels.recv_tx = rtsp_to_whip_tx.clone();
                tcp_handler.channels.send_rx = Some(whip_to_rtsp_rx);

                (media_info, Some(whip_to_rtsp_tx), Some(rtsp_to_whip_rx))
            }
            RtspMode::Push => {
                let (whep_to_rtsp_tx, rtsp_to_whep_rx) = tcp_handler.get_channels();
                (media_info, Some(whep_to_rtsp_tx), Some(rtsp_to_whep_rx))
            }
        };

        let stream = rtsp_session_clone.stream;
        tokio::spawn(async move {
            if let Err(e) = tcp_handler.start(stream).await {
                error!("TCP handler error: {}", e);
            }
        });

        Ok((media_info, sender, receiver))
    } else {
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let options_request = Request::builder(Method::Options, Version::V1_0)
                    .header(headers::CSEQ, rtsp_session_clone.cseq.to_string())
                    .header(headers::USER_AGENT, USER_AGENT)
                    .header(headers::SESSION, session_id.as_str())
                    .empty();

                if rtsp_session_clone
                    .send_request(&options_request.map_body(|_| vec![]))
                    .await
                    .is_err()
                {
                    warn!("Failed to send keep-alive OPTIONS request");
                    break;
                }

                if rtsp_session_clone.read_response().await.is_err() {
                    warn!("Failed to read keep-alive OPTIONS response");
                    break;
                }
                rtsp_session_clone.cseq += 1;
            }
        });

        Ok((media_info, None, None))
    }
}
async fn setup_track<T>(
    rtsp_session: &mut RtspSession<T>,
    track: &sdp_types::Media,
    track_id: &str,
    base_url: &str,
) -> Result<(
    Option<u16>,
    Option<u16>,
    Option<u16>,
    Option<u16>,
    Option<Codec>,
)>
where
    T: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let track_url = find_control_attribute(track, base_url, track_id);

    let rtp_client_port = pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?;
    rtsp_session.rtp_client_port = Some(rtp_client_port);
    rtsp_session.url = track_url;

    let (session_id, rtp_server_port) = rtsp_session.send_setup_request(None).await?;
    rtsp_session.session_id = Some(session_id);

    let codec = extract_codec_from_track(track);

    Ok((
        Some(rtp_client_port),
        Some(rtp_client_port + 1),
        Some(rtp_server_port),
        Some(rtp_server_port + 1),
        codec,
    ))
}

fn generate_digest_response(
    username: &str,
    password: &str,
    uri: &str,
    realm: &str,
    nonce: &str,
    method: &str,
) -> String {
    let mut hasher = Md5::new();
    hasher.update(format!("{username}:{realm}:{password}"));
    let ha1 = format!("{:x}", hasher.finalize());

    let mut hasher = Md5::new();
    hasher.update(format!("{method}:{uri}"));
    let ha2 = format!("{:x}", hasher.finalize());

    let mut hasher = Md5::new();
    hasher.update(format!("{ha1}:{nonce}:{ha2}"));
    format!("{:x}", hasher.finalize())
}

fn extract_h264_params(track: &sdp_types::Media) -> Option<rtsp::H264Params> {
    track
        .attributes
        .iter()
        .find(|attr| attr.attribute == "fmtp")
        .and_then(|attr| {
            let value = attr.value.as_ref()?;

            let params_start = value.find("sprop-parameter-sets=")?;
            let params_str = &value[params_start + 21..];
            let params_end = params_str.find(';').unwrap_or(params_str.len());
            let params = &params_str[..params_end].trim();

            let mut parts = params.split(',');
            let sps_b64 = parts.next()?.trim();
            let pps_b64 = parts.next()?.trim();

            let sps = general_purpose::STANDARD.decode(sps_b64).ok()?;
            let pps = general_purpose::STANDARD.decode(pps_b64).ok()?;

            trace!(
                "Extracted H.264 params from track - SPS: {} bytes, PPS: {} bytes",
                sps.len(),
                pps.len()
            );

            Some(rtsp::H264Params { sps, pps })
        })
}

fn extract_h265_params(track: &sdp_types::Media) -> Option<rtsp::H265Params> {
    track
        .attributes
        .iter()
        .find(|attr| attr.attribute == "fmtp")
        .and_then(|attr| {
            let value = attr.value.as_ref()?;

            let extract_param = |param_name: &str| -> Option<Vec<u8>> {
                let param_start = value.find(&format!("{}=", param_name))?;
                let param_str = &value[param_start + param_name.len() + 1..];
                let param_end = param_str.find(';').unwrap_or(param_str.len());
                let param_b64 = param_str[..param_end].trim();
                general_purpose::STANDARD.decode(param_b64).ok()
            };

            let vps = extract_param("sprop-vps")?;
            let sps = extract_param("sprop-sps")?;
            let pps = extract_param("sprop-pps")?;

            trace!(
                "Extracted H.265 params from track - VPS: {} bytes, SPS: {} bytes, PPS: {} bytes",
                vps.len(),
                sps.len(),
                pps.len()
            );

            Some(rtsp::H265Params { vps, sps, pps })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn test_send_describe_request() {
        let (client, server) = duplex(4096);

        let sdp_content = "v=0\r\no=- 12345 12345 IN IP4 127.0.0.1\r\ns=Test\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n";
        let content_length = sdp_content.len();

        tokio::spawn(async move {
            let mut server = server;
            let mut buffer = vec![0; 4096];
            let n = server.read(&mut buffer).await.unwrap();
            let _ = String::from_utf8_lossy(&buffer[..n]);

            let response = format!(
                "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Type: application/sdp\r\nContent-Length: {content_length}\r\n\r\n{sdp_content}"
            );

            server.write_all(response.as_bytes()).await.unwrap();
            server.flush().await.unwrap();
        });

        let mut rtsp_session = RtspSession {
            stream: client,
            url: "rtsp://example.com".to_string(),
            cseq: 1,
            auth_params: AuthParams {
                username: "".to_string(),
                password: "".to_string(),
            },
            session_id: None,
            rtp_client_port: None,
            auth_header: None,
        };

        let sdp_content = rtsp_session.send_describe_request().await.unwrap();
        assert!(sdp_content.contains("v=0"));
        assert!(sdp_content.contains("m=video"));
        assert_eq!(rtsp_session.cseq, 2);
    }

    #[tokio::test]
    async fn test_send_describe_request_unauthorized() {
        let (client, server) = duplex(4096);

        let sdp_content = "v=0\r\no=- 12345 12345 IN IP4 127.0.0.1\r\ns=Test\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n";
        let content_length = sdp_content.len();

        tokio::spawn(async move {
            let mut server = server;
            let mut buffer = vec![0; 4096];
            let n = server.read(&mut buffer).await.unwrap();
            let _ = String::from_utf8_lossy(&buffer[..n]);

            let unauthorized_response = "RTSP/1.0 401 Unauthorized\r\nCSeq: 1\r\nWWW-Authenticate: Digest realm=\"testrealm\", nonce=\"testnonce\"\r\nContent-Length: 0\r\n\r\n";
            server
                .write_all(unauthorized_response.as_bytes())
                .await
                .unwrap();
            server.flush().await.unwrap();

            let n = server.read(&mut buffer).await.unwrap();
            let _ = String::from_utf8_lossy(&buffer[..n]);

            let ok_response = format!(
                "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Type: application/sdp\r\nContent-Length: {content_length}\r\n\r\n{sdp_content}"
            );
            server.write_all(ok_response.as_bytes()).await.unwrap();
            server.flush().await.unwrap();
        });

        let mut rtsp_session = RtspSession {
            stream: client,
            url: "rtsp://example.com".to_string(),
            cseq: 1,
            auth_params: AuthParams {
                username: "user".to_string(),
                password: "pass".to_string(),
            },
            session_id: None,
            rtp_client_port: None,
            auth_header: None,
        };

        let sdp_content = rtsp_session.send_describe_request().await.unwrap();
        assert!(sdp_content.contains("v=0"));
        assert!(sdp_content.contains("m=video"));
        assert!(rtsp_session.cseq > 1);
    }

    #[tokio::test]
    async fn test_send_options_request() {
        let (client, server) = duplex(4096);

        tokio::spawn(async move {
            let mut server = server;
            let mut buffer = vec![0; 4096];
            let n = server.read(&mut buffer).await.unwrap();
            let _ = String::from_utf8_lossy(&buffer[..n]);

            let response = "RTSP/1.0 200 OK\r\nCSeq: 1\r\nPublic: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN\r\n\r\n";
            server.write_all(response.as_bytes()).await.unwrap();
            server.flush().await.unwrap();
        });

        let mut rtsp_session = RtspSession {
            stream: client,
            url: "rtsp://example.com".to_string(),
            cseq: 1,
            auth_params: AuthParams {
                username: "".to_string(),
                password: "".to_string(),
            },
            session_id: None,
            rtp_client_port: None,
            auth_header: None,
        };

        rtsp_session.send_options_request().await.unwrap();
        assert_eq!(rtsp_session.cseq, 2);
    }
}
