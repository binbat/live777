use anyhow::{Result, anyhow};
use rtsp_types::{Request, Response, StatusCode, Version, headers};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::{ServerConfig, ServerSession};

pub struct Handler {
    addr: SocketAddr,
    cseq: u32,
    session_id: Option<String>,
    sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
    config: ServerConfig,
    sdp_content: Option<Vec<u8>>,
    next_channel: u8,
}

impl Handler {
    pub fn new(
        addr: SocketAddr,
        sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
        config: ServerConfig,
    ) -> Self {
        Self {
            addr,
            cseq: 0,
            session_id: None,
            sessions,
            config,
            sdp_content: None,
            next_channel: 0,
        }
    }

    pub fn set_sdp(&mut self, sdp: Vec<u8>) {
        self.sdp_content = Some(sdp);
    }

    pub fn cseq(&self) -> u32 {
        self.cseq
    }

    pub fn sdp_content(&self) -> Option<&Vec<u8>> {
        self.sdp_content.as_ref()
    }

    pub fn client_addr(&self) -> SocketAddr {
        self.addr
    }

    pub async fn handle_options(
        &mut self,
        _request: &Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>> {
        debug!("Handling OPTIONS request from {}", self.addr);
        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(
                headers::PUBLIC,
                "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, ANNOUNCE, RECORD",
            )
            .empty();
        Ok(response.map_body(|_| vec![]))
    }

    pub async fn handle_announce(
        &mut self,
        request: &Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>> {
        debug!("Handling ANNOUNCE request from {}", self.addr);
        let sdp_content = request.body().to_vec();
        if let Ok(sdp_str) = std::str::from_utf8(&sdp_content) {
            debug!("Received SDP:\n{}", sdp_str);
        }
        self.sdp_content = Some(sdp_content);

        let session_id = self.get_or_create_session().await;
        debug!("Created session: {} for {}", session_id, self.addr);

        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::SESSION, session_id.as_str())
            .empty();
        Ok(response.map_body(|_| vec![]))
    }

    pub async fn handle_setup_tcp(
        &mut self,
        _transport_str: &str,
    ) -> Result<(Response<Vec<u8>>, u8, u8)> {
        debug!("Handling SETUP TCP request from {}", self.addr);

        let rtp_channel = self.next_channel;
        let rtcp_channel = self.next_channel + 1;
        self.next_channel += 2;

        let session_id = self.get_or_create_session().await;
        let response_transport = format!(
            "RTP/AVP/TCP;unicast;interleaved={}-{}",
            rtp_channel, rtcp_channel
        );
        let session_header = format!("{};timeout={}", session_id, self.config.session_timeout);

        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::SESSION, session_header.as_str())
            .header(headers::TRANSPORT, response_transport)
            .empty();

        Ok((response.map_body(|_| vec![]), rtp_channel, rtcp_channel))
    }

    pub async fn handle_setup_udp(
        &mut self,
        transport_str: &str,
    ) -> Result<(Response<Vec<u8>>, u16, u16, u16, u16)> {
        debug!("Handling SETUP UDP request from {}", self.addr);

        let (client_rtp_port, client_rtcp_port) = self.parse_client_ports(transport_str)?;
        debug!(
            "Client ports: RTP={}, RTCP={}",
            client_rtp_port, client_rtcp_port
        );

        let temp_rtp = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        let temp_rtcp = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        let server_rtp_port = temp_rtp.local_addr()?.port();
        let server_rtcp_port = temp_rtcp.local_addr()?.port();
        drop(temp_rtp);
        drop(temp_rtcp);

        debug!(
            "Allocated server ports: RTP={}, RTCP={}",
            server_rtp_port, server_rtcp_port
        );

        let session_id = self.get_or_create_session().await;
        let response_transport = format!(
            "RTP/AVP;unicast;client_port={}-{};server_port={}-{}",
            client_rtp_port, client_rtcp_port, server_rtp_port, server_rtcp_port
        );
        let session_header = format!("{};timeout={}", session_id, self.config.session_timeout);

        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::SESSION, session_header.as_str())
            .header(headers::TRANSPORT, response_transport)
            .empty();

        Ok((
            response.map_body(|_| vec![]),
            client_rtp_port,
            client_rtcp_port,
            server_rtp_port,
            server_rtcp_port,
        ))
    }

    fn parse_client_ports(&self, transport_str: &str) -> Result<(u16, u16)> {
        let client_ports: Vec<&str> = transport_str
            .split("client_port=")
            .nth(1)
            .and_then(|s| s.split(';').next())
            .ok_or_else(|| anyhow!("Missing client_port"))?
            .split('-')
            .collect();

        if client_ports.len() != 2 {
            return Err(anyhow!("Invalid port format"));
        }

        Ok((client_ports[0].parse()?, client_ports[1].parse()?))
    }

    async fn get_or_create_session(&mut self) -> String {
        if let Some(id) = &self.session_id {
            id.clone()
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            self.session_id = Some(id.clone());
            let session = ServerSession::new(id.clone(), self.addr, self.config.session_timeout);
            self.sessions.write().await.insert(id.clone(), session);
            id
        }
    }

    pub async fn handle_describe(
        &mut self,
        _request: &Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>> {
        debug!("Handling DESCRIBE request from {}", self.addr);

        let sdp_content = self
            .sdp_content
            .as_ref()
            .ok_or_else(|| anyhow!("No SDP content available"))?;

        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::CONTENT_TYPE, "application/sdp")
            .header(headers::CONTENT_LENGTH, sdp_content.len().to_string())
            .build(sdp_content.clone());

        Ok(response)
    }

    pub async fn handle_play(&mut self, _request: &Request<Vec<u8>>) -> Result<Response<Vec<u8>>> {
        debug!("Handling PLAY request from {}", self.addr);

        let session_id = self
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow!("No session ID"))?;

        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::SESSION, session_id.as_str())
            .header(headers::RANGE, "npt=0.000-")
            .empty();

        Ok(response.map_body(|_| vec![]))
    }

    pub async fn handle_record(
        &mut self,
        _request: &Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>> {
        debug!("Handling RECORD request from {}", self.addr);
        let session_id = self
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow!("No session ID"))?;
        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .header(headers::SESSION, session_id.as_str())
            .empty();
        Ok(response.map_body(|_| vec![]))
    }

    pub async fn handle_teardown(
        &mut self,
        _request: &Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>> {
        info!("Handling TEARDOWN request from {}", self.addr);
        if let Some(session_id) = &self.session_id {
            self.sessions.write().await.remove(session_id);
            info!("Removed session: {}", session_id);
        }
        let response = Response::builder(Version::V1_0, StatusCode::Ok)
            .header(headers::CSEQ, self.cseq.to_string())
            .empty();
        Ok(response.map_body(|_| vec![]))
    }

    pub fn update_cseq(&mut self, request: &Request<Vec<u8>>) {
        if let Some(cseq_header) = request.header(&headers::CSEQ) {
            self.cseq = cseq_header.as_str().parse().unwrap_or(0);
        }
    }
}
