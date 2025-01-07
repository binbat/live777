use anyhow::{anyhow, Ok, Result};
use cli::{codec_from_str, Codec};
use md5::{Digest, Md5};
use portpicker::pick_unused_port;
use rtsp_types::{
    headers,
    headers::{transport, HeaderValue, WWW_AUTHENTICATE},
    Message, Method, Request, Response, StatusCode, Url, Version,
};
use sdp_types::Session;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{self, Duration},
};
use tracing::{debug, error, info, trace, warn};

const USER_AGENT: &str = "whipinto";
const DEFAULT_RTSP_PORT: u16 = 554;

struct AuthParams {
    username: String,
    password: String,
}

struct RtspSession {
    stream: TcpStream,
    uri: String,
    cseq: u32,
    auth_params: AuthParams,
    session_id: Option<String>,
    rtp_client_port: Option<u16>,
    auth_header: Option<HeaderValue>,
}

impl RtspSession {
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
            &self.uri,
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
            _ => "UNKNOWN",
        };

        let response = self.generate_digest_response(realm, nonce, method_str);
        format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
            self.auth_params.username, realm, nonce, self.uri, response
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
                self.uri
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

    async fn keep_rtsp_alive(mut self) -> Result<()> {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let options_request = Request::builder(Method::Options, Version::V1_0)
                .header(headers::CSEQ, self.cseq.to_string())
                .header(headers::USER_AGENT, USER_AGENT)
                .empty();

            if self
                .send_request(&options_request.map_body(|_| vec![]))
                .await
                .is_err()
            {
                warn!("Failed to send OPTIONS request");
                break;
            }

            if self.read_response().await.is_err() {
                warn!("Failed to read OPTIONS response");
                break;
            }

            self.cseq += 1;
        }

        Ok(())
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
                self.uri
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
                self.uri
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

        if describe_response.status() == StatusCode::Unauthorized {
            if let Some(auth_header) = describe_response.header(&WWW_AUTHENTICATE).cloned() {
                describe_response = self
                    .handle_unauthorized(Method::Describe, &auth_header)
                    .await?;
            }
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
                self.uri
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

        info!(
            "Preparing SETUP request for URI: {}, RTP client port: {}-{}",
            self.uri,
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

        info!("Sending SETUP request...");
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
        info!(
            "Extracted server port from transport header: {}",
            server_port
        );

        info!(
            "SETUP request completed. Session ID: {}, Server Port: {}",
            session_id, server_port
        );

        Ok((session_id, server_port))
    }
}

pub async fn setup_rtsp_session(rtsp_url: &str) -> Result<rtsp::MediaInfo> {
    let mut url = Url::parse(rtsp_url)?;
    let host = url
        .host()
        .ok_or_else(|| anyhow!("Host not found"))?
        .to_string();
    let port = url.port().unwrap_or(DEFAULT_RTSP_PORT);

    let addr = format!("{}:{}", host, port);
    info!("Connecting to RTSP server at {}", addr);
    let stream = TcpStream::connect(addr).await?;

    let mut rtsp_session = RtspSession {
        stream,
        uri: url.as_str().to_string(),
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

    rtsp_session.send_options_request().await?;

    let sdp_content = rtsp_session.send_describe_request().await?;

    let sdp: Session = Session::parse(sdp_content.as_bytes())
        .map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;

    let video_track = sdp.medias.iter().find(|md| md.media == "video");
    let audio_track = sdp.medias.iter().find(|md| md.media == "audio");
    debug!("track video: {:?}, audio: {:?}", video_track, audio_track);

    if video_track.is_none() && audio_track.is_none() {
        return Err(anyhow!("No tracks found in SDP"));
    }

    let mut media_info = rtsp::MediaInfo::default();

    if let Some(video_track) = video_track {
        let (rtp_client, rtcp_client, rtp_server, codec) =
            setup_track(&mut rtsp_session, video_track, "0").await?;
        media_info.video_rtp_server = rtp_client;
        media_info.video_rtcp_client = rtcp_client;
        media_info.video_rtp_client = rtp_server;
        media_info.video_codec = codec;
    }

    if let Some(audio_track) = audio_track {
        let (rtp_client, rtcp_client, rtp_server, codec) =
            setup_track(&mut rtsp_session, audio_track, "1").await?;
        media_info.audio_rtp_server = rtp_client;
        media_info.audio_rtcp_client = rtcp_client;
        media_info.audio_rtp_client = rtp_server;
        media_info.audio_codec = codec;
    }

    let play_request = Request::builder(Method::Play, Version::V1_0)
        .request_uri(
            rtsp_session
                .uri
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
        .send_request(&play_request.map_body(|_| vec![]))
        .await?;
    let mut play_response = rtsp_session.read_response().await?;
    trace!("play_response: {:?}", play_response);

    if play_response.status() == StatusCode::Unauthorized {
        if let Some(auth_header) = play_response.header(&WWW_AUTHENTICATE).cloned() {
            play_response = rtsp_session
                .handle_unauthorized(Method::Play, &auth_header)
                .await?;
        }
    }

    if play_response.status() != StatusCode::Ok {
        return Err(anyhow!("PLAY request failed"));
    }

    tokio::spawn(rtsp_session.keep_rtsp_alive());

    Ok(media_info)
}

pub async fn setup_rtsp_push_session(
    rtsp_url: &str,
    sdp_content: String,
) -> Result<rtsp::MediaInfo> {
    let mut url = Url::parse(rtsp_url)?;
    let host = url.host_str().ok_or_else(|| anyhow!("Invalid RTSP URL"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("Invalid RTSP URL"))?;

    let addr = format!("{}:{}", host, port);

    let stream = TcpStream::connect(&addr).await?;
    info!("Connected to RTSP server: {}", addr);

    let mut rtsp_session = RtspSession {
        stream,
        uri: url.as_str().to_string(),
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

    rtsp_session.send_options_request().await?;
    debug!("OPTIONS request successful");

    debug!("SDP Content: {}", sdp_content);
    rtsp_session
        .send_announce_request(sdp_content.clone())
        .await?;
    debug!("ANNOUNCE request successful");

    let sdp: Session = Session::parse(sdp_content.as_bytes())
        .map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;
    info!("Parsed SDP successfully");
    debug!("Parsed SDP: {:?}", sdp);

    let video_track = sdp.medias.iter().find(|md| md.media == "video");
    let audio_track = sdp.medias.iter().find(|md| md.media == "audio");
    debug!(
        "Found video track: {:?}, audio track: {:?}",
        video_track, audio_track
    );

    if video_track.is_none() && audio_track.is_none() {
        error!("No tracks found in SDP");
        return Err(anyhow!("No tracks found in SDP"));
    }
    let mut media_info = rtsp::MediaInfo::default();

    if let Some(video_track) = video_track {
        info!("Setting up video track");
        let video_url = video_track
            .attributes
            .iter()
            .find_map(|attr| {
                if attr.attribute == "control" {
                    let value = attr.value.clone().unwrap_or_default();
                    if value.starts_with("rtsp://") {
                        Some(value)
                    } else {
                        Some(format!("{}/{}", rtsp_session.uri.clone(), value))
                    }
                } else {
                    None
                }
            })
            .unwrap_or_else(|| format!("{}/trackID=1", rtsp_session.uri));
        debug!("Video track URL: {}", video_url);

        media_info.video_rtp_server =
            Some(pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?);
        debug!(
            "Allocated RTP client port for video: {:?}",
            media_info.video_rtp_server
        );

        rtsp_session.rtp_client_port = media_info.video_rtp_server;
        rtsp_session.uri = video_url;

        let (session_id, v_server_port) = rtsp_session
            .send_setup_request(Some(transport::TransportMode::Record))
            .await?;
        info!(
            "Video track SETUP successful, Session ID: {}, Server Port: {}",
            session_id, v_server_port
        );

        rtsp_session.session_id = Some(session_id);
        media_info.video_rtp_client = Some(v_server_port);
    }

    if let Some(audio_track) = audio_track {
        rtsp_session.uri = url.as_str().to_string();
        info!("Audio track URL: {:?}", audio_track);
        let audio_url = audio_track
            .attributes
            .iter()
            .find_map(|attr| {
                if attr.attribute == "control" {
                    let value = attr.value.clone().unwrap_or_default();
                    if value.starts_with("rtsp://") {
                        Some(value)
                    } else {
                        Some(format!("{}/{}", rtsp_session.uri.clone(), value))
                    }
                } else {
                    None
                }
            })
            .unwrap_or_else(|| format!("{}/trackID=2", rtsp_session.uri));
        debug!("Audio track URL: {}", audio_url);

        media_info.audio_rtp_server =
            Some(pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?);
        debug!(
            "Allocated RTP client port for audio: {:?}",
            media_info.audio_rtp_server
        );

        rtsp_session.rtp_client_port = media_info.audio_rtp_server;
        rtsp_session.uri = audio_url;

        let (_session_id, a_server_port) = rtsp_session
            .send_setup_request(Some(transport::TransportMode::Record))
            .await?;
        info!(
            "Audio track SETUP successful, Server Port: {}",
            a_server_port
        );

        media_info.audio_rtp_client = Some(a_server_port);
    }

    info!("Sending RECORD request");
    rtsp_session.uri = url.as_str().to_string();
    let record_request = Request::builder(Method::Record, Version::V1_0)
        .request_uri(
            rtsp_session
                .uri
                .parse::<Url>()
                .map_err(|_| anyhow!("Invalid URI"))?,
        )
        .header(headers::CSEQ, rtsp_session.cseq.to_string())
        .header(headers::USER_AGENT, USER_AGENT)
        .header(
            headers::SESSION,
            rtsp_session
                .session_id
                .clone()
                .ok_or_else(|| anyhow!("Missing session ID"))?,
        )
        .empty();

    rtsp_session
        .send_request(&record_request.map_body(|_| vec![]))
        .await?;
    let response = rtsp_session.read_response().await?;
    rtsp_session.cseq += 1;

    if response.status() == StatusCode::Unauthorized {
        if let Some(auth_header) = response.header(&WWW_AUTHENTICATE).cloned() {
            info!("Handling unauthorized response for RECORD request");
            let response = rtsp_session
                .handle_unauthorized(Method::Record, &auth_header)
                .await?;
            if response.status() != StatusCode::Ok {
                error!("RECORD request failed after authentication");
                return Err(anyhow!("RECORD request failed after authentication"));
            }
        } else {
            error!("RECORD request failed with 401 Unauthorized and no WWW-Authenticate header");
            return Err(anyhow!(
                "RECORD request failed with 401 Unauthorized and no WWW-Authenticate header"
            ));
        }
    } else if response.status() != StatusCode::Ok {
        error!("RECORD request failed with status: {:?}", response.status());
        return Err(anyhow!(
            "RECORD request failed with status: {:?}",
            response.status()
        ));
    }

    info!("RTSP PUSH session setup complete, starting keep-alive task");
    tokio::spawn(rtsp_session.keep_rtsp_alive());

    Ok(media_info)
}

async fn setup_track(
    rtsp_session: &mut RtspSession,
    track: &sdp_types::Media,
    track_id: &str,
) -> Result<(Option<u16>, Option<u16>, Option<u16>, Option<Codec>)> {
    let track_url = track
        .attributes
        .iter()
        .find_map(|attr| {
            if attr.attribute == "control" {
                let value = attr.value.clone().unwrap_or_default();
                if value.starts_with("rtsp://") {
                    Some(value)
                } else {
                    Some(format!("{}/{}", rtsp_session.uri, value))
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| format!("{}/trackID={}", rtsp_session.uri, track_id));

    let rtp_client_port = pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?;
    rtsp_session.rtp_client_port = Some(rtp_client_port);
    rtsp_session.uri = track_url;

    let (session_id, rtp_server_port) = rtsp_session.send_setup_request(None).await?;
    rtsp_session.session_id = Some(session_id);

    let codec = track.attributes.iter().find_map(|attr| {
        if attr.attribute == "rtpmap" {
            let value = attr.value.as_ref()?.split_whitespace().nth(1)?;
            codec_from_str(value).ok()
        } else {
            None
        }
    });

    Ok((
        Some(rtp_client_port),
        Some(rtp_client_port + 1),
        Some(rtp_server_port),
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
    format!("{:x}", {
        let mut hasher = Md5::new();
        hasher.update(format!(
            "{}:{}:{}",
            format_args!("{:x}", {
                let hasher = Md5::new_with_prefix(format!("{}:{}:{}", username, realm, password));
                hasher.finalize()
            }),
            nonce,
            format_args!("{:x}", {
                let hasher = Md5::new_with_prefix(format!("{}:{}", method, uri));
                hasher.finalize()
            })
        ));
        hasher.finalize()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_digest_response() {
        let username = "username";
        let password = "password";
        let uri = "/resource";
        let realm = "Realm";
        let nonce = "1234567890";
        let method = "GET";

        let expected_response = "5a8a58beeb78f36ed2c0f0d474288f3d";

        let response = generate_digest_response(username, password, uri, realm, nonce, method);

        assert_eq!(response, expected_response);
    }
}
