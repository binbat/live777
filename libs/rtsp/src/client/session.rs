use anyhow::{Result, anyhow};
use rtsp_types::{Message, Method, Request, Response, StatusCode, Url, Version, headers};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info, trace};

use super::RtspMode;
use super::auth::{AuthParams, generate_digest_response, parse_auth_header};
use crate::channels::InterleavedChannel;
use crate::transport_manager::{TransportConfig, UdpPortInfo};
use crate::{MediaInfo, TransportInfo};

const USER_AGENT: &str = "livetwo";

pub struct RtspSession<T> {
    stream: T,
    url: String,
    cseq: u32,
    auth_params: AuthParams,
    pub session_id: Option<String>,
    next_channel: u8,
}

impl<T> RtspSession<T>
where
    T: AsyncReadExt + AsyncWriteExt + Unpin,
{
    pub fn new(stream: T, url: String, auth_params: AuthParams) -> Self {
        Self {
            stream,
            url,
            cseq: 1,
            auth_params,
            session_id: None,
            next_channel: 0,
        }
    }

    pub async fn send_request(&mut self, request: &Request<Vec<u8>>) -> Result<()> {
        let mut buffer = Vec::new();
        request.write(&mut buffer)?;
        self.stream.write_all(&buffer).await?;
        trace!("Sent RTSP request: {:?} {}", request.method(), self.cseq);
        Ok(())
    }

    pub fn into_stream(self) -> T {
        self.stream
    }

    pub async fn read_response(&mut self) -> Result<Response<Vec<u8>>> {
        let mut buffer = vec![0; 4096];
        let n = self.stream.read(&mut buffer).await?;
        if n == 0 {
            return Err(anyhow!("Connection closed"));
        }
        let (message, _) = Message::parse(&buffer[..n])?;
        match message {
            Message::Response(response) => {
                trace!("Received RTSP response: {}", response.status());
                Ok(response)
            }
            _ => Err(anyhow!("Expected a response message")),
        }
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

        let response = generate_digest_response(
            &self.auth_params.username,
            &self.auth_params.password,
            &self.url,
            realm,
            nonce,
            method_str,
        );

        format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
            self.auth_params.username, realm, nonce, self.url, response
        )
    }

    pub async fn handle_unauthorized(
        &mut self,
        method: Method,
        auth_header: &headers::HeaderValue,
    ) -> Result<Response<Vec<u8>>> {
        let (realm, nonce) = parse_auth_header(auth_header)?;
        let auth_header_value = self.generate_authorization_header(&realm, &nonce, &method);

        let auth_request = Request::builder(method, Version::V1_0)
            .request_uri(self.url.parse::<Url>()?)
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

    pub async fn send_options_request(&mut self) -> Result<()> {
        let options_request = Request::builder(Method::Options, Version::V1_0)
            .request_uri(self.url.parse::<Url>()?)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        self.send_request(&options_request.map_body(|_| vec![]))
            .await?;
        self.read_response().await?;
        self.cseq += 1;
        info!("OPTIONS request successful");
        Ok(())
    }

    pub async fn send_describe_request(&mut self) -> Result<String> {
        let describe_request = Request::builder(Method::Describe, Version::V1_0)
            .request_uri(self.url.parse::<Url>()?)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::ACCEPT, "application/sdp")
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        self.send_request(&describe_request.map_body(|_| vec![]))
            .await?;
        let mut describe_response = self.read_response().await?;
        self.cseq += 1;

        if describe_response.status() == StatusCode::Unauthorized
            && let Some(auth_header) = describe_response
                .header(&headers::WWW_AUTHENTICATE)
                .cloned()
        {
            describe_response = self
                .handle_unauthorized(Method::Describe, &auth_header)
                .await?;
        }

        let sdp_content = String::from_utf8_lossy(describe_response.body()).to_string();
        if sdp_content.is_empty() {
            return Err(anyhow!("Received empty SDP content"));
        }

        info!("DESCRIBE request successful");
        Ok(sdp_content)
    }

    pub async fn send_announce_request(&mut self, sdp: String) -> Result<()> {
        let announce_request = Request::builder(Method::Announce, Version::V1_0)
            .request_uri(self.url.parse::<Url>()?)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::CONTENT_TYPE, "application/sdp")
            .header(headers::USER_AGENT, USER_AGENT)
            .build(sdp.into_bytes());

        self.send_request(&announce_request).await?;
        let announce_response = self.read_response().await?;
        self.cseq += 1;

        if announce_response.status() == StatusCode::Unauthorized {
            if let Some(auth_header) = announce_response
                .header(&headers::WWW_AUTHENTICATE)
                .cloned()
            {
                let announce_response = self
                    .handle_unauthorized(Method::Announce, &auth_header)
                    .await?;
                if announce_response.status() != StatusCode::Ok {
                    return Err(anyhow!("ANNOUNCE request failed after authentication"));
                }
            } else {
                return Err(anyhow!("ANNOUNCE request failed with 401 Unauthorized"));
            }
        } else if announce_response.status() != StatusCode::Ok {
            return Err(anyhow!("ANNOUNCE request failed"));
        }

        info!("ANNOUNCE request successful");
        Ok(())
    }

    pub async fn setup_udp(
        &mut self,
        control_url: &str,
        mode: &crate::client::RtspMode,
    ) -> Result<TransportConfig> {
        let rtp_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        let rtcp_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;

        let client_rtp_port = rtp_socket.local_addr()?.port();
        let client_rtcp_port = rtcp_socket.local_addr()?.port();

        info!(
            "Client local ports: RTP={}, RTCP={}",
            client_rtp_port, client_rtcp_port
        );

        drop(rtp_socket);
        drop(rtcp_socket);

        let transport_header = match mode {
            crate::client::RtspMode::Pull => {
                format!(
                    "RTP/AVP;unicast;client_port={}-{}",
                    client_rtp_port, client_rtcp_port
                )
            }
            crate::client::RtspMode::Push => {
                format!(
                    "RTP/AVP;unicast;client_port={}-{};mode=record",
                    client_rtp_port, client_rtcp_port
                )
            }
        };

        let mut setup_request = Request::builder(Method::Setup, Version::V1_0)
            .request_uri(control_url.parse::<Url>()?)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::TRANSPORT, transport_header)
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        if let Some(sid) = &self.session_id {
            setup_request = Request::builder(Method::Setup, Version::V1_0)
                .request_uri(control_url.parse::<Url>()?)
                .header(headers::CSEQ, self.cseq.to_string())
                .header(
                    headers::TRANSPORT,
                    match mode {
                        crate::client::RtspMode::Pull => {
                            format!(
                                "RTP/AVP;unicast;client_port={}-{}",
                                client_rtp_port, client_rtcp_port
                            )
                        }
                        crate::client::RtspMode::Push => {
                            format!(
                                "RTP/AVP;unicast;client_port={}-{};mode=record",
                                client_rtp_port, client_rtcp_port
                            )
                        }
                    },
                )
                .header(headers::SESSION, sid.as_str())
                .header(headers::USER_AGENT, USER_AGENT)
                .empty();
        }

        self.send_request(&setup_request.map_body(|_| vec![]))
            .await?;
        let response = self.read_response().await?;
        self.cseq += 1;

        if response.status() != StatusCode::Ok {
            return Err(anyhow!("SETUP failed: {}", response.status()));
        }

        if let Some(session_header) = response.header(&headers::SESSION) {
            let session_id = session_header
                .as_str()
                .split(';')
                .next()
                .unwrap_or("")
                .to_string();
            if self.session_id.is_none() {
                self.session_id = Some(session_id);
            }
        }

        let transport = response
            .header(&headers::TRANSPORT)
            .ok_or_else(|| anyhow!("No transport in SETUP response"))?;

        let (server_rtp_port, server_rtcp_port) = parse_server_ports(transport.as_str())?;

        let server_addr = self
            .url
            .parse::<Url>()?
            .host_str()
            .ok_or_else(|| anyhow!("No host in URL"))?
            .parse::<std::net::IpAddr>()?;

        let server_socket_addr = std::net::SocketAddr::new(server_addr, server_rtp_port);

        info!(
            "SETUP UDP successful, server ports: {}:{}",
            server_rtp_port, server_rtcp_port
        );

        Ok(TransportConfig::Udp(UdpPortInfo {
            client_rtp_port,
            client_rtcp_port,
            server_rtp_port,
            server_rtcp_port,
            client_addr: server_socket_addr,
        }))
    }

    pub async fn setup_tcp(
        &mut self,
        control_url: &str,
        mode: &crate::client::RtspMode,
    ) -> Result<TransportConfig> {
        let rtp_channel = self.next_channel;
        let rtcp_channel = self.next_channel + 1;
        self.next_channel += 2;

        let transport_header = match mode {
            crate::client::RtspMode::Pull => {
                format!(
                    "RTP/AVP/TCP;unicast;interleaved={}-{}",
                    rtp_channel, rtcp_channel
                )
            }
            crate::client::RtspMode::Push => {
                format!(
                    "RTP/AVP/TCP;unicast;interleaved={}-{};mode=record",
                    rtp_channel, rtcp_channel
                )
            }
        };

        let mut setup_request = Request::builder(Method::Setup, Version::V1_0)
            .request_uri(control_url.parse::<Url>()?)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::TRANSPORT, transport_header)
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        if let Some(sid) = &self.session_id {
            setup_request = Request::builder(Method::Setup, Version::V1_0)
                .request_uri(control_url.parse::<Url>()?)
                .header(headers::CSEQ, self.cseq.to_string())
                .header(
                    headers::TRANSPORT,
                    match mode {
                        crate::client::RtspMode::Pull => {
                            format!(
                                "RTP/AVP/TCP;unicast;interleaved={}-{}",
                                rtp_channel, rtcp_channel
                            )
                        }
                        crate::client::RtspMode::Push => {
                            format!(
                                "RTP/AVP/TCP;unicast;interleaved={}-{};mode=record",
                                rtp_channel, rtcp_channel
                            )
                        }
                    },
                )
                .header(headers::SESSION, sid.as_str())
                .header(headers::USER_AGENT, USER_AGENT)
                .empty();
        }

        self.send_request(&setup_request.map_body(|_| vec![]))
            .await?;
        let response = self.read_response().await?;
        self.cseq += 1;

        if response.status() != StatusCode::Ok {
            return Err(anyhow!("SETUP failed: {}", response.status()));
        }

        if let Some(session_header) = response.header(&headers::SESSION) {
            let session_id = session_header
                .as_str()
                .split(';')
                .next()
                .unwrap_or("")
                .to_string();
            if self.session_id.is_none() {
                self.session_id = Some(session_id);
            }
        }

        info!(
            "SETUP TCP successful, channels: {}-{}",
            rtp_channel, rtcp_channel
        );

        Ok(TransportConfig::Tcp {
            rtp_channel,
            rtcp_channel,
        })
    }

    pub async fn send_play_request(&mut self) -> Result<()> {
        let session_id = self
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow!("No session ID"))?;

        let play_request = Request::builder(Method::Play, Version::V1_0)
            .request_uri(self.url.parse::<Url>()?)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::SESSION, session_id.as_str())
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        self.send_request(&play_request.map_body(|_| vec![]))
            .await?;
        let response = self.read_response().await?;
        self.cseq += 1;

        if response.status() != StatusCode::Ok {
            return Err(anyhow!("PLAY failed: {}", response.status()));
        }

        info!("PLAY request successful");
        Ok(())
    }

    pub async fn send_record_request(&mut self) -> Result<()> {
        let session_id = self
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow!("No session ID"))?;

        let record_request = Request::builder(Method::Record, Version::V1_0)
            .request_uri(self.url.parse::<Url>()?)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::SESSION, session_id.as_str())
            .header(headers::USER_AGENT, USER_AGENT)
            .empty();

        self.send_request(&record_request.map_body(|_| vec![]))
            .await?;
        let response = self.read_response().await?;
        self.cseq += 1;

        if response.status() != StatusCode::Ok {
            return Err(anyhow!("RECORD failed: {}", response.status()));
        }

        info!("RECORD request successful");
        Ok(())
    }
}

fn parse_server_ports(transport: &str) -> Result<(u16, u16)> {
    for part in transport.split(';') {
        let part = part.trim();
        if part.starts_with("server_port=") {
            let ports = part.strip_prefix("server_port=").unwrap();
            let mut parts = ports.split('-');
            let rtp = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow!("Invalid RTP port"))?;
            let rtcp = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow!("Invalid RTCP port"))?;
            return Ok((rtp, rtcp));
        }
    }
    Err(anyhow!("No server_port in transport"))
}

pub async fn setup_rtsp_session(
    rtsp_url: &str,
    sdp_content: Option<String>,
    target_host: &str,
    mode: RtspMode,
    use_tcp: bool,
) -> Result<(MediaInfo, Option<InterleavedChannel>)> {
    use tokio::sync::mpsc::unbounded_channel;

    let mut url = Url::parse(rtsp_url)?;
    info!("Connecting to RTSP server: {}", rtsp_url);

    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("No port specified"))?;
    let addr = format!("{}:{}", target_host, port);

    let stream = tokio::net::TcpStream::connect(&addr).await?;
    let auth_params = AuthParams::from_url(&url);

    url.set_username("").unwrap();
    url.set_password(None).unwrap();

    let auth = auth_params.unwrap_or_else(|| AuthParams::new(String::new(), String::new()));
    let mut session = RtspSession::new(stream, url.to_string(), auth);

    session.send_options_request().await?;

    let sdp: sdp_types::Session = match mode {
        RtspMode::Pull => {
            let sdp_content = session.send_describe_request().await?;
            debug!("Received SDP:\n{}", sdp_content);
            sdp_types::Session::parse(sdp_content.as_bytes())?
        }
        RtspMode::Push => {
            let sdp_content = sdp_content.ok_or_else(|| anyhow!("SDP required for push mode"))?;
            session.send_announce_request(sdp_content.clone()).await?;
            sdp_types::Session::parse(sdp_content.as_bytes())?
        }
    };

    let (video_codec, audio_codec) = crate::sdp::parse_codecs_from_sdp(&sdp)?;

    let mut media_info = MediaInfo {
        video_codec,
        audio_codec,
        video_transport: None,
        audio_transport: None,
    };

    let mut video_control: Option<String> = None;
    let mut audio_control: Option<String> = None;

    for media in &sdp.medias {
        let control = media
            .attributes
            .iter()
            .find(|a| a.attribute == "control")
            .and_then(|a| a.value.clone());
        if media.media == "video" {
            video_control = control;
        } else if media.media == "audio" {
            audio_control = control;
        }
    }

    if use_tcp {
        info!("Setting up TCP interleaved mode");

        if let Some(control) = video_control {
            let control_url = build_control_url(rtsp_url, &control);
            let config = session.setup_tcp(&control_url, &mode).await?;

            if let TransportConfig::Tcp {
                rtp_channel,
                rtcp_channel,
            } = config
            {
                media_info.video_transport = Some(TransportInfo::Tcp {
                    rtp_channel,
                    rtcp_channel,
                });
                info!(
                    "Video TCP channels: RTP={}, RTCP={}",
                    rtp_channel, rtcp_channel
                );
            }
        }

        if let Some(control) = audio_control {
            let control_url = build_control_url(rtsp_url, &control);
            let config = session.setup_tcp(&control_url, &mode).await?;

            if let TransportConfig::Tcp {
                rtp_channel,
                rtcp_channel,
            } = config
            {
                media_info.audio_transport = Some(TransportInfo::Tcp {
                    rtp_channel,
                    rtcp_channel,
                });
                info!(
                    "Audio TCP channels: RTP={}, RTCP={}",
                    rtp_channel, rtcp_channel
                );
            }
        }

        match mode {
            RtspMode::Pull => session.send_play_request().await?,
            RtspMode::Push => session.send_record_request().await?,
        }

        let (data_from_stream_tx, data_from_stream_rx) = unbounded_channel::<(u8, Vec<u8>)>();
        let (data_to_stream_tx, data_to_stream_rx) = unbounded_channel::<(u8, Vec<u8>)>();

        let stream = session.into_stream();
        let session_mode = mode.to_session_mode();

        tokio::spawn(async move {
            if let Err(e) = crate::tcp_stream::handle_tcp_stream(
                stream,
                session_mode,
                data_from_stream_tx,
                data_to_stream_rx,
            )
            .await
            {
                error!("TCP stream handler error: {}", e);
            }
        });

        Ok((media_info, Some((data_to_stream_tx, data_from_stream_rx))))
    } else {
        info!("Setting up UDP transport mode");

        if let Some(control) = video_control {
            let control_url = build_control_url(rtsp_url, &control);
            let config = session.setup_udp(&control_url, &mode).await?;

            if let TransportConfig::Udp(ref port_info) = config {
                media_info.video_transport = Some(match mode {
                    RtspMode::Pull => TransportInfo::Udp {
                        rtp_recv_port: Some(port_info.client_rtp_port),
                        rtp_send_port: None,
                        rtcp_recv_port: Some(port_info.client_rtcp_port),
                        rtcp_send_port: Some(port_info.server_rtcp_port),
                        server_addr: Some(port_info.client_addr),
                    },
                    RtspMode::Push => TransportInfo::Udp {
                        rtp_send_port: Some(port_info.server_rtp_port),
                        rtp_recv_port: None,
                        rtcp_send_port: Some(port_info.server_rtcp_port),
                        rtcp_recv_port: Some(port_info.client_rtcp_port),
                        server_addr: Some(port_info.client_addr),
                    },
                });
            }
        }

        if let Some(control) = audio_control {
            let control_url = build_control_url(rtsp_url, &control);
            let config = session.setup_udp(&control_url, &mode).await?;

            if let TransportConfig::Udp(ref port_info) = config {
                media_info.audio_transport = Some(match mode {
                    RtspMode::Pull => TransportInfo::Udp {
                        rtp_recv_port: Some(port_info.client_rtp_port),
                        rtp_send_port: None,
                        rtcp_recv_port: Some(port_info.client_rtcp_port),
                        rtcp_send_port: Some(port_info.server_rtcp_port),
                        server_addr: Some(port_info.client_addr),
                    },
                    RtspMode::Push => TransportInfo::Udp {
                        rtp_send_port: Some(port_info.server_rtp_port),
                        rtp_recv_port: None,
                        rtcp_send_port: Some(port_info.server_rtcp_port),
                        rtcp_recv_port: Some(port_info.client_rtcp_port),
                        server_addr: Some(port_info.client_addr),
                    },
                });
            }
        }

        match mode {
            RtspMode::Pull => session.send_play_request().await?,
            RtspMode::Push => session.send_record_request().await?,
        }

        info!("RTSP UDP session setup completed");

        let session_id = session
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("Missing session ID"))?;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

            loop {
                interval.tick().await;

                let options_request = Request::builder(Method::Options, Version::V1_0)
                    .request_uri(session.url.parse::<Url>().unwrap())
                    .header(headers::CSEQ, session.cseq.to_string())
                    .header(headers::USER_AGENT, USER_AGENT)
                    .header(headers::SESSION, session_id.as_str())
                    .empty();

                if session
                    .send_request(&options_request.map_body(|_| vec![]))
                    .await
                    .is_err()
                {
                    error!("Failed to send keep-alive OPTIONS request");
                    break;
                }

                if session.read_response().await.is_err() {
                    error!("Failed to read keep-alive OPTIONS response");
                    break;
                }

                session.cseq += 1;
                debug!("Keep-alive OPTIONS sent, session active");
            }

            info!("RTSP keep-alive task stopped");
        });

        Ok((media_info, None))
    }
}

fn build_control_url(base_url: &str, control: &str) -> String {
    if control.starts_with("rtsp://") {
        control.to_string()
    } else {
        format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            control.trim_start_matches('/')
        )
    }
}
