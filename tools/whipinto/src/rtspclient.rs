use anyhow::{anyhow, Result};
use cli::Codec;
use md5::{Digest, Md5};
use rtsp_types::{
    headers::{HeaderName, HeaderValue, WWW_AUTHENTICATE},
    Message, Method, Request, Response, Version,
};
use sdp_types::Session;
use tokio::net::TcpListener;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{self, Duration},
};
use tracing::info;
use url::Url;

const USER_AGENT: &str = "whipinto";
const DEFAULT_RTSP_PORT: u16 = 554;

pub async fn send_request(stream: &mut TcpStream, request: &Request<Vec<u8>>) -> Result<()> {
    let mut buffer = Vec::new();
    request.write(&mut buffer)?;
    stream.write_all(&buffer).await?;
    Ok(())
}

pub async fn read_response(stream: &mut TcpStream) -> Result<Response<Vec<u8>>> {
    let mut buffer = vec![0; 4096];
    let n = stream.read(&mut buffer).await?;
    let (message, _) = Message::parse(&buffer[..n])?;
    if let Message::Response(response) = message {
        Ok(response)
    } else {
        Err(anyhow!("Expected a response message"))
    }
}

fn generate_digest_response(
    username: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    uri: &str,
    method: &str,
) -> String {
    let ha1 = format!("{:x}", {
        let mut hasher = Md5::new();
        hasher.update(format!("{}:{}:{}", username, realm, password));
        hasher.finalize()
    });

    let ha2 = format!("{:x}", {
        let mut hasher = Md5::new();
        hasher.update(format!("{}:{}", method, uri));
        hasher.finalize()
    });

    format!("{:x}", {
        let mut hasher = Md5::new();
        hasher.update(format!("{}:{}:{}", ha1, nonce, ha2));
        hasher.finalize()
    })
}

fn generate_authorization_header(
    username: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    uri: &str,
    method: &str,
) -> String {
    let response = generate_digest_response(username, password, realm, nonce, uri, method);
    format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
        username, realm, nonce, uri, response
    )
}

fn parse_auth(header_value: &HeaderValue) -> Result<(String, String)> {
    let header_str = header_value.as_str();
    let realm_key = "realm=\"";
    let nonce_key = "nonce=\"";
    let realm_start = header_str
        .find(realm_key)
        .ok_or_else(|| anyhow!("realm not found"))?
        + realm_key.len();
    let realm_end = header_str[realm_start..]
        .find('"')
        .ok_or_else(|| anyhow!("realm end not found"))?
        + realm_start;
    let realm = header_str[realm_start..realm_end].to_string();

    let nonce_start = header_str
        .find(nonce_key)
        .ok_or_else(|| anyhow!("nonce not found"))?
        + nonce_key.len();
    let nonce_end = header_str[nonce_start..]
        .find('"')
        .ok_or_else(|| anyhow!("nonce end not found"))?
        + nonce_start;
    let nonce = header_str[nonce_start..nonce_end].to_string();

    Ok((realm, nonce))
}

async fn keep_rtsp_alive(mut stream: TcpStream, mut cseq: u32) -> Result<()> {
    let mut interval = time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        let options_request = Request::builder(Method::Options, Version::V1_0)
            .header(
                HeaderName::from_static_str("CSeq").unwrap(),
                cseq.to_string(),
            )
            .header(
                HeaderName::from_static_str("User-Agent").unwrap(),
                USER_AGENT,
            )
            .empty();

        if let Err(e) = send_request(&mut stream, &options_request.map_body(|_| vec![])).await {
            eprintln!("Failed to send OPTIONS request: {:?}", e);
            break;
        }

        if let Err(e) = read_response(&mut stream).await {
            eprintln!("Failed to read OPTIONS response: {:?}", e);
            break;
        }

        cseq += 1;
    }

    Ok(())
}

fn codec_from_str(s: &str) -> Result<Codec> {
    match s {
        "VP8" => Ok(Codec::Vp8),
        "VP9" => Ok(Codec::Vp9),
        "H264" => Ok(Codec::H264),
        "AV1" => Ok(Codec::AV1),
        "OPUS" => Ok(Codec::Opus),
        "G722" => Ok(Codec::G722),
        "PCMU" => Ok(Codec::PCMU),
        "PCMA" => Ok(Codec::PCMA),
        _ => Err(anyhow!("Unknown codec: {}", s)),
    }
}

pub async fn setup_rtsp_session(rtsp_url: &str) -> Result<(u16, Codec)> {
    let url = Url::parse(rtsp_url)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("Host not found"))?
        .to_string();
    let port = url.port().unwrap_or(DEFAULT_RTSP_PORT);

    let addr = format!("{}:{}", host, port);
    info!("Connecting to RTSP server at {}", addr);
    let mut stream = TcpStream::connect(addr).await?;

    let mut cseq = 1;
    let username = url.username().to_string();
    let password = url.password().unwrap_or("").to_string();
    let uri = rtsp_url.to_string();

    // Send OPTIONS request
    let options_request = Request::builder(Method::Options, Version::V1_0)
        .header(
            HeaderName::from_static_str("CSeq").unwrap(),
            cseq.to_string(),
        )
        .header(
            HeaderName::from_static_str("User-Agent").unwrap(),
            USER_AGENT,
        )
        .empty();

    send_request(&mut stream, &options_request.map_body(|_| vec![])).await?;
    let options_response = read_response(&mut stream).await?;
    cseq += 1;

    // Handle authentication if required
    if options_response.status() == rtsp_types::StatusCode::Unauthorized {
        let auth_header = options_response
            .header(&WWW_AUTHENTICATE)
            .ok_or_else(|| anyhow!("WWW-Authenticate header not found"))?;
        let (realm, nonce) = parse_auth(auth_header)?;

        // Send authenticated OPTIONS request
        let auth_header_value =
            generate_authorization_header(&username, &password, &realm, &nonce, &uri, "OPTIONS");
        let options_request = Request::builder(Method::Options, Version::V1_0)
            .header(
                HeaderName::from_static_str("CSeq").unwrap(),
                cseq.to_string(),
            )
            .header(
                HeaderName::from_static_str("User-Agent").unwrap(),
                USER_AGENT,
            )
            .header(
                HeaderName::from_static_str("Authorization").unwrap(),
                auth_header_value,
            )
            .empty();

        send_request(&mut stream, &options_request.map_body(|_| vec![])).await?;
        let options_response = read_response(&mut stream).await?;
        cseq += 1;

        if options_response.status() != rtsp_types::StatusCode::Ok {
            return Err(anyhow!("OPTIONS request failed"));
        }
    }

    // Send DESCRIBE request
    let describe_request = Request::builder(Method::Describe, Version::V1_0)
        .request_uri(
            uri.clone()
                .parse::<Url>()
                .map_err(|_| anyhow!("Invalid URI"))?,
        )
        .header(
            HeaderName::from_static_str("CSeq").unwrap(),
            cseq.to_string(),
        )
        .header(
            HeaderName::from_static_str("Accept").unwrap(),
            "application/sdp",
        )
        .header(
            HeaderName::from_static_str("User-Agent").unwrap(),
            USER_AGENT,
        )
        .empty();

    send_request(&mut stream, &describe_request.map_body(|_| vec![])).await?;
    let mut describe_response = read_response(&mut stream).await?;
    cseq += 1;

    if describe_response.status() == rtsp_types::StatusCode::Unauthorized {
        let auth_header = describe_response
            .header(&WWW_AUTHENTICATE)
            .ok_or_else(|| anyhow!("WWW-Authenticate header not found"))?;
        let (realm, nonce) = parse_auth(auth_header)?;

        // Send authenticated DESCRIBE request
        let auth_header_value =
            generate_authorization_header(&username, &password, &realm, &nonce, &uri, "DESCRIBE");
        let describe_request = Request::builder(Method::Describe, Version::V1_0)
            .request_uri(
                uri.clone()
                    .parse::<Url>()
                    .map_err(|_| anyhow!("Invalid URI"))?,
            )
            .header(
                HeaderName::from_static_str("CSeq").unwrap(),
                cseq.to_string(),
            )
            .header(
                HeaderName::from_static_str("Accept").unwrap(),
                "application/sdp",
            )
            .header(
                HeaderName::from_static_str("User-Agent").unwrap(),
                USER_AGENT,
            )
            .header(
                HeaderName::from_static_str("Authorization").unwrap(),
                auth_header_value,
            )
            .empty();

        send_request(&mut stream, &describe_request.map_body(|_| vec![])).await?;
        describe_response = read_response(&mut stream).await?;
        cseq += 1;

        if describe_response.status() != rtsp_types::StatusCode::Ok {
            return Err(anyhow!("DESCRIBE request failed"));
        }
    }
    let sdp_content = String::from_utf8_lossy(describe_response.body()).to_string();

    if sdp_content.is_empty() {
        return Err(anyhow!("Received empty SDP content"));
    }

    let sdp: Session = Session::parse(sdp_content.as_bytes())
        .map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;

    let video_track = sdp.medias.iter().find(|md| md.media == "video");
    let audio_track = sdp.medias.iter().find(|md| md.media == "audio");
    println!("wwww:{:?}", video_track);

    if video_track.is_none() && audio_track.is_none() {
        return Err(anyhow!("No tracks found in SDP"));
    }

    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let rtp_port = listener.local_addr()?.port();
    let rtcp_port = rtp_port + 1;

    async fn send_setup_request(
        stream: &mut TcpStream,
        uri: &str,
        cseq: &mut u32,
        rtp_port: u16,
        rtcp_port: u16,
        session_id: Option<&str>,
    ) -> Result<String> {
        let mut setup_request_builder = Request::builder(Method::Setup, Version::V1_0)
            .request_uri(uri.parse::<Url>().map_err(|_| anyhow!("Invalid URI"))?)
            .header(
                HeaderName::from_static_str("CSeq").unwrap(),
                cseq.to_string(),
            )
            .header(
                HeaderName::from_static_str("User-Agent").unwrap(),
                USER_AGENT,
            )
            .header(
                HeaderName::from_static_str("Transport").unwrap(),
                format!("RTP/AVP;unicast;client_port={}-{}", rtp_port, rtcp_port),
            );

        if let Some(session) = session_id {
            setup_request_builder = setup_request_builder
                .header(HeaderName::from_static_str("Session").unwrap(), session);
        }

        let setup_request = setup_request_builder.empty();

        send_request(stream, &setup_request.map_body(|_| vec![])).await?;
        let setup_response = read_response(stream).await?;
        *cseq += 1;

        if setup_response.status() != rtsp_types::StatusCode::Ok {
            return Err(anyhow!("SETUP request failed"));
        }

        let session_id = setup_response
            .header(&HeaderName::from_static_str("Session").unwrap())
            .ok_or_else(|| anyhow!("Session header not found"))?
            .as_str()
            .split(';')
            .next()
            .ok_or_else(|| anyhow!("Failed to parse session ID"))?
            .to_string();

        Ok(session_id)
    }

    let video_uri = video_track
        .and_then(|md| {
            md.attributes.iter().find_map(|attr| {
                if attr.attribute == "control" {
                    Some(attr.value.clone().unwrap_or_default())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| format!("{}/trackID=1", uri));

    //audio
    // let audio_url = video_track
    //     .and_then(|md| {
    //         md.attributes.iter().find_map(|attr| {
    //             if attr.attribute == "control" {
    //                 Some(attr.value.clone().unwrap_or_default())
    //             } else {
    //                 None
    //             }
    //         })
    //     })
    //     .unwrap_or_else(|| format!("{}/trackID=1", uri));

    let session_id = send_setup_request(
        &mut stream,
        &video_uri,
        &mut cseq,
        rtp_port,
        rtcp_port,
        None,
    )
    .await?;

    //audio
    // send_setup_request(&mut stream, &audio_uri, &mut cseq, rtp_port, rtcp_port, Some(&session_id)).await?;

    // Send PLAY request
    let play_request = Request::builder(Method::Play, Version::V1_0)
        .request_uri(uri.parse::<Url>().map_err(|_| anyhow!("Invalid URI"))?)
        .header(
            HeaderName::from_static_str("CSeq").unwrap(),
            cseq.to_string(),
        )
        .header(
            HeaderName::from_static_str("User-Agent").unwrap(),
            USER_AGENT,
        )
        .header(
            HeaderName::from_static_str("Session").unwrap(),
            &*session_id,
        )
        .empty();

    send_request(&mut stream, &play_request.map_body(|_| vec![])).await?;
    let play_response = read_response(&mut stream).await?;

    if play_response.status() != rtsp_types::StatusCode::Ok {
        return Err(anyhow!("PLAY request failed"));
    }

    tokio::spawn(async move {
        if let Err(e) = keep_rtsp_alive(stream, cseq).await {
            eprintln!("Failed to keep RTSP alive: {:?}", e);
        }
    });

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

    //audio_codec
    // let audio_codec = video_track
    // .and_then(|md| {
    //     md.attributes.iter().find_map(|attr| {
    //         if attr.attribute == "rtpmap" {
    //             let parts: Vec<&str> = attr.value.as_ref()?.split_whitespace().collect();
    //             if parts.len() > 1 {
    //                 Some(parts[1].split('/').next().unwrap_or("").to_string())
    //             } else {
    //                 None
    //             }
    //         } else {
    //             None
    //         }
    //     })
    // })
    // .unwrap_or_else(|| "unknown".to_string());

    let video_codec = codec_from_str(&video_codec)?;

    Ok((rtp_port, video_codec))
}
