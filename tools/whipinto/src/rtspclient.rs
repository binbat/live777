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
use tracing::{debug, info, trace};

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
        let ha1 = format!("{:x}", {
            let mut hasher = Md5::new();
            hasher.update(format!(
                "{}:{}:{}",
                self.auth_params.username, realm, self.auth_params.password
            ));
            hasher.finalize()
        });

        let ha2 = format!("{:x}", {
            let mut hasher = Md5::new();
            hasher.update(format!("{}:{}", method, self.uri));
            hasher.finalize()
        });

        format!("{:x}", {
            let mut hasher = Md5::new();
            hasher.update(format!("{}:{}:{}", ha1, nonce, ha2));
            hasher.finalize()
        })
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
                eprintln!("Failed to send OPTIONS request");
                break;
            }

            if self.read_response().await.is_err() {
                eprintln!("Failed to read OPTIONS response");
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

    async fn send_setup_request(&mut self) -> Result<String> {
        let rtp_client_port = self
            .rtp_client_port
            .ok_or_else(|| anyhow!("RTP server port not set"))?;
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
                    params: transport::RtpTransportParameters {
                        unicast: true,
                        client_port: Some((rtp_client_port, Some(rtp_client_port + 1))),
                        ..Default::default()
                    },
                }),
            ]));

        if let Some(auth_header) = &self.auth_header {
            let (realm, nonce) = Self::parse_auth(auth_header)?;
            let auth_header_value =
                self.generate_authorization_header(&realm, &nonce, &Method::Setup);
            setup_request_builder =
                setup_request_builder.header(headers::AUTHORIZATION, auth_header_value)
        }

        if let Some(session_id) = &self.session_id {
            setup_request_builder =
                setup_request_builder.header(headers::SESSION, session_id.as_str());
        }

        let setup_request = setup_request_builder.empty();

        self.send_request(&setup_request.map_body(|_| vec![]))
            .await?;
        let setup_response = self.read_response().await?;
        self.cseq += 1;

        if setup_response.status() == StatusCode::Unauthorized {
            if let Some(auth_header) = setup_response.header(&WWW_AUTHENTICATE).cloned() {
                let setup_response = self
                    .handle_unauthorized(Method::Setup, &auth_header)
                    .await?;
                if setup_response.status() != StatusCode::Ok {
                    return Err(anyhow!("SETUP request failed after authentication"));
                }
            } else {
                return Err(anyhow!(
                    "SETUP request failed with 401 Unauthorized and no WWW-Authenticate header"
                ));
            }
        } else if setup_response.status() != StatusCode::Ok {
            return Err(anyhow!("SETUP request failed"));
        }

        let session_id = setup_response
            .header(&headers::SESSION)
            .ok_or_else(|| anyhow!("Session header not found"))?
            .as_str()
            .split(';')
            .next()
            .ok_or_else(|| anyhow!("Failed to parse session ID"))?
            .to_string();

        Ok(session_id)
    }
}

// pub async fn setup_rtsp_session(rtsp_url: &str) -> Result<rtsp::MediaInfo> {
//     let mut url = Url::parse(rtsp_url)?;
//     let host = url
//         .host()
//         .ok_or_else(|| anyhow!("Host not found"))?
//         .to_string();
//     let port = url.port().unwrap_or(DEFAULT_RTSP_PORT);

//     let addr = format!("{}:{}", host, port);
//     info!("Connecting to RTSP server at {}", addr);
//     let stream = TcpStream::connect(addr).await?;

//     let mut rtsp_session = RtspSession {
//         stream,
//         uri: url.as_str().to_string(),
//         cseq: 1,
//         auth_params: AuthParams {
//             username: url.username().to_string(),
//             password: url.password().unwrap_or("").to_string(),
//         },
//         session_id: None,
//         rtp_client_port: None,
//         auth_header: None,
//     };

//     url.set_username("").unwrap();
//     url.set_password(None).unwrap();

//     rtsp_session.send_options_request().await?;

//     let sdp_content = rtsp_session.send_describe_request().await?;

//     let sdp: Session = Session::parse(sdp_content.as_bytes())
//         .map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;

//     let video_track = sdp.medias.iter().find(|md| md.media == "video");
//     let audio_track = sdp.medias.iter().find(|md| md.media == "audio");
//     debug!("track video: {:?}, audio: {:?}", video_track, audio_track);

//     if video_track.is_none() && audio_track.is_none() {
//         return Err(anyhow!("No tracks found in SDP"));
//     }

//     let video_port = pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?;
//     let audio_port = pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?;

//     let video_uri = video_track
//         .and_then(|md| {
//             md.attributes.iter().find_map(|attr| {
//                 if attr.attribute == "control" {
//                     let value = attr.value.clone().unwrap_or_default();
//                     if value.starts_with("rtsp://") {
//                         Some(value)
//                     } else {
//                         Some(format!("{}/{}", rtsp_session.uri, value))
//                     }
//                 } else {
//                     None
//                 }
//             })
//         })
//         .unwrap_or_else(|| format!("{}/trackID=1", rtsp_session.uri));

//     let audio_uri = audio_track
//         .and_then(|md| {
//             md.attributes.iter().find_map(|attr| {
//                 if attr.attribute == "control" {
//                     let value = attr.value.clone().unwrap_or_default();
//                     if value.starts_with("rtsp://") {
//                         Some(value)
//                     } else {
//                         Some(format!("{}/{}", rtsp_session.uri, value))
//                     }
//                 } else {
//                     None
//                 }
//             })
//         })
//         .unwrap_or_else(|| format!("{}/trackID=2", rtsp_session.uri));

//     trace!("video uri: {:?}", video_uri);
//     trace!("audio uri: {:?}", audio_uri);

//     rtsp_session.uri.clone_from(&video_uri);
//     rtsp_session.rtp_client_port = Some(video_port);

//     let session_id = rtsp_session.send_setup_request().await?;
//     trace!("session id: {:?}", session_id);

//     rtsp_session.session_id = Some(session_id);

//     rtsp_session.uri.clone_from(&audio_uri);
//     rtsp_session.rtp_client_port = Some(audio_port);
//     rtsp_session.send_setup_request().await?;

//     let play_request = Request::builder(Method::Play, Version::V1_0)
//         .request_uri(
//             rtsp_session
//                 .uri
//                 .parse::<Url>()
//                 .map_err(|_| anyhow!("Invalid URI"))?,
//         )
//         .header(headers::CSEQ, rtsp_session.cseq.to_string())
//         .header(headers::USER_AGENT, USER_AGENT)
//         .header(
//             headers::SESSION,
//             rtsp_session.session_id.as_ref().unwrap().as_str(),
//         )
//         .empty();

//     rtsp_session
//         .send_request(&play_request.map_body(|_| vec![]))
//         .await?;
//     let mut play_response = rtsp_session.read_response().await?;
//     trace!("play_response: {:?}", play_response);

//     if play_response.status() == StatusCode::Unauthorized {
//         if let Some(auth_header) = play_response.header(&WWW_AUTHENTICATE).cloned() {
//             play_response = rtsp_session
//                 .handle_unauthorized(Method::Play, &auth_header)
//                 .await?;
//         }
//     }

//     if play_response.status() != StatusCode::Ok {
//         return Err(anyhow!("PLAY request failed"));
//     }

//     tokio::spawn(rtsp_session.keep_rtsp_alive());

//     let video_codec = video_track
//         .and_then(|md| {
//             md.attributes.iter().find_map(|attr| {
//                 if attr.attribute == "rtpmap" {
//                     let parts: Vec<&str> = attr.value.as_ref()?.split_whitespace().collect();
//                     if parts.len() > 1 {
//                         Some(parts[1].split('/').next().unwrap_or("").to_string())
//                     } else {
//                         None
//                     }
//                 } else {
//                     None
//                 }
//             })
//         })
//         .and_then(|codec_str| codec_from_str(&codec_str).ok());

//     let audio_codec = audio_track
//         .and_then(|md| {
//             md.attributes.iter().find_map(|attr| {
//                 if attr.attribute == "rtpmap" {
//                     let parts: Vec<&str> = attr.value.as_ref()?.split_whitespace().collect();
//                     if parts.len() > 1 {
//                         Some(parts[1].split('/').next().unwrap_or("").to_string())
//                     } else {
//                         None
//                     }
//                 } else {
//                     None
//                 }
//             })
//         })
//         .and_then(|codec_str| codec_from_str(&codec_str).ok());

//     let media_info = rtsp::MediaInfo {
//         video_rtp_client: Some(video_port),
//         video_rtcp_client: Some(video_port + 1),
//         video_rtp_server: None,
//         audio_rtp_client: Some(audio_port),
//         audio_rtp_server: None,
//         video_codec,
//         audio_codec,
//     };

//     Ok(media_info)
// }
pub async fn setup_rtsp_session(rtsp_url: &str) -> Result<(u16, u16, Codec, Codec)> {
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

    let rtp_port = pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?;
    let rtp_audio_port = pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?;

    let video_uri = video_track
        .and_then(|md| {
            md.attributes.iter().find_map(|attr| {
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
        })
        .unwrap_or_else(|| format!("{}/trackID=1", rtsp_session.uri));

    let audio_uri = audio_track
        .and_then(|md| {
            md.attributes.iter().find_map(|attr| {
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
        })
        .unwrap_or_else(|| format!("{}/trackID=2", rtsp_session.uri));

    trace!("video uri: {:?}", video_uri);
    trace!("audio uri: {:?}", audio_uri);

    rtsp_session.uri.clone_from(&video_uri);
    rtsp_session.rtp_client_port = Some(rtp_port);

    let session_id = rtsp_session.send_setup_request().await?;
    trace!("session id: {:?}", session_id);

    rtsp_session.session_id = Some(session_id);

    rtsp_session.uri.clone_from(&audio_uri);
    rtsp_session.rtp_client_port = Some(rtp_audio_port);
    rtsp_session.send_setup_request().await?;

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

    let video_codec = video_track
        .and_then(|md| {
            md.attributes.iter().find_map(|attr| {
                if attr.attribute == "rtpmap" {
                    let parts: Vec<&str> = attr.value.as_ref()?.split_whitespace().collect();
                    if parts.len() > 1 {
                        Some(parts[1].split('/').next().unwrap_or("").to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| "unknown".to_string());

    let audio_codec = audio_track
        .and_then(|md| {
            md.attributes.iter().find_map(|attr| {
                if attr.attribute == "rtpmap" {
                    let parts: Vec<&str> = attr.value.as_ref()?.split_whitespace().collect();
                    if parts.len() > 1 {
                        Some(parts[1].split('/').next().unwrap_or("").to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| "unknown".to_string());

    let video_codec = codec_from_str(&video_codec)?;
    let audio_codec = codec_from_str(&audio_codec)?;

    Ok((rtp_port, rtp_audio_port, video_codec, audio_codec))
}
