use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::constants::net;
use crate::{Request, Response, StatusCode, Version, headers};

use super::{ServerConfig, ServerSession};

pub struct Handler {
    addr: SocketAddr,
    cseq: u32,
    session_id: Option<String>,
    sessions: Arc<RwLock<HashMap<String, ServerSession>>>,
    config: ServerConfig,
    sdp_content: Option<Vec<u8>>,
    /// Cached parsed SDP, populated on `set_sdp` so that callers
    /// (`resolve_setup_media_kind`, `parse_codecs`) don't re-parse.
    parsed_sdp: Option<sdp_types::Session>,
    next_channel: u8,
    /// UDP sockets allocated during the most recent SETUP. Kept open so the
    /// advertised server ports cannot be stolen before the data transfer starts.
    udp_rtp_socket: Option<UdpSocket>,
    udp_rtcp_socket: Option<UdpSocket>,
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
            parsed_sdp: None,
            next_channel: 0,
            udp_rtp_socket: None,
            udp_rtcp_socket: None,
        }
    }

    pub fn set_sdp(&mut self, sdp: Vec<u8>) {
        match sdp_types::Session::parse(&sdp) {
            Ok(parsed) => {
                self.parsed_sdp = Some(parsed);
                self.sdp_content = Some(sdp);
            }
            Err(e) => {
                tracing::warn!("Failed to parse SDP in set_sdp: {}", e);
                self.sdp_content = Some(sdp);
                self.parsed_sdp = None;
            }
        }
    }

    pub fn cseq(&self) -> u32 {
        self.cseq
    }

    pub fn sdp_content(&self) -> Option<&Vec<u8>> {
        self.sdp_content.as_ref()
    }

    pub fn parsed_sdp(&self) -> Option<&sdp_types::Session> {
        self.parsed_sdp.as_ref()
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
        info!(
            "RTSP ANNOUNCE SDP media summary: {}",
            summarize_sdp_media(&sdp_content)
        );
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

        let bind_addr = net::bind_any_for(&self.addr);

        let rtp_socket = UdpSocket::bind(&bind_addr).await?;
        let rtcp_socket = UdpSocket::bind(&bind_addr).await?;
        let server_rtp_port = rtp_socket.local_addr()?.port();
        let server_rtcp_port = rtcp_socket.local_addr()?.port();
        self.udp_rtp_socket = Some(rtp_socket);
        self.udp_rtcp_socket = Some(rtcp_socket);

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

    /// Take the UDP sockets allocated by the last SETUP and return them to the
    /// caller. Returns `None` if SETUP has not yet been called for this session.
    pub fn take_udp_sockets(&mut self) -> Option<(UdpSocket, UdpSocket)> {
        match (self.udp_rtp_socket.take(), self.udp_rtcp_socket.take()) {
            (Some(rtp), Some(rtcp)) => Some((rtp, rtcp)),
            _ => None,
        }
    }

    fn parse_client_ports(&self, transport_str: &str) -> Result<(u16, u16)> {
        let value = transport_str
            .split(';')
            .map(str::trim)
            .find(|param| {
                param
                    .split('=')
                    .next()
                    .map(str::trim)
                    .is_some_and(|name| name.eq_ignore_ascii_case("client_port"))
            })
            .and_then(|param| param.split('=').nth(1))
            .ok_or_else(|| anyhow!("Missing client_port in Transport header"))?
            .trim();

        let mut ports = value.split('-').map(str::trim);
        let rtp_port = ports
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("Missing RTP port in client_port"))?
            .parse::<u16>()
            .map_err(|e| anyhow!("Invalid RTP port in client_port: {}", e))?;
        let rtcp_port = ports
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("Missing RTCP port in client_port"))?
            .parse::<u16>()
            .map_err(|e| anyhow!("Invalid RTCP port in client_port: {}", e))?;

        if ports.next().is_some() {
            return Err(anyhow!("Too many ports in client_port"));
        }

        Ok((rtp_port, rtcp_port))
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

    pub async fn update_activity(&self) {
        if let Some(session_id) = &self.session_id {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.update_activity();
            }
        }
    }
}

fn summarize_sdp_media(sdp: &[u8]) -> String {
    let Ok(session) = sdp_types::Session::parse(sdp) else {
        return "<failed to parse SDP>".to_string();
    };

    session
        .medias
        .iter()
        .map(|media| {
            let formats = media.fmt.clone();
            let attrs = media
                .attributes
                .iter()
                .filter_map(|attr| match attr.attribute.as_str() {
                    "rtpmap" | "fmtp" | "control" => Some(format!(
                        "a={}:{}",
                        attr.attribute,
                        attr.value.as_deref().unwrap_or("")
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "m={} {} {} {} [{}]",
                media.media, media.port, media.proto, formats, attrs
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn dummy_handler() -> Handler {
        Handler::new(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0),
            Arc::new(RwLock::new(HashMap::new())),
            ServerConfig::default(),
        )
    }

    #[test]
    fn parse_client_ports_standard_format() {
        let h = dummy_handler();
        assert_eq!(
            h.parse_client_ports("RTP/AVP;unicast;client_port=5004-5005;server_port=6004-6005")
                .unwrap(),
            (5004, 5005)
        );
    }

    #[test]
    fn parse_client_ports_with_whitespace() {
        let h = dummy_handler();
        assert_eq!(
            h.parse_client_ports("RTP/AVP;unicast;client_port = 5004 - 5005")
                .unwrap(),
            (5004, 5005)
        );
    }

    #[test]
    fn parse_client_ports_missing_rtcp() {
        let h = dummy_handler();
        assert!(
            h.parse_client_ports("RTP/AVP;unicast;client_port=5004")
                .is_err()
        );
    }

    #[test]
    fn parse_client_ports_extra_ports() {
        let h = dummy_handler();
        assert!(
            h.parse_client_ports("RTP/AVP;unicast;client_port=5004-5005-5006")
                .is_err()
        );
    }
}
