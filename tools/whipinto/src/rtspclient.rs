use anyhow::{anyhow, Result};
use cli::Codec;
use md5::{Digest, Md5};
use portpicker::pick_unused_port;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{sleep, Duration},
};
use tracing::info;
use url::Url;

const USER_AGENT: &str = "whipinto";
const DEFAULT_RTSP_PORT: u16 = 554;

struct RtspSession {
    stream: TcpStream,
    rtsp_url: String,
    username: String,
    password: String,
    realm: String,
    nonce: String,
    cseq: u32,
}

impl RtspSession {
    async fn new(rtsp_url: &str) -> Result<Self> {
        let url = Url::parse(rtsp_url)?;

        let username = url.username().to_string();
        let password = url.password().unwrap_or("").to_string();
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("Host not found"))?
            .to_string();
        let port = url.port().unwrap_or(DEFAULT_RTSP_PORT);

        let addr = format!("{}:{}", host, port);
        info!("Connecting to RTSP server at {}", addr);
        let stream = TcpStream::connect(addr).await?;

        Ok(RtspSession {
            stream,
            rtsp_url: rtsp_url.to_string(),
            username,
            password,
            realm: String::new(),
            nonce: String::new(),
            cseq: 1,
        })
    }

    async fn send_request(&mut self, request: &str) -> Result<()> {
        info!("Sending RTSP request: {}", request);
        self.stream.write_all(request.as_bytes()).await?;
        Ok(())
    }

    async fn read_response(&mut self) -> Result<String> {
        let mut buf = vec![0; 4096];
        let n = self.stream.read(&mut buf).await?;
        let response = String::from_utf8_lossy(&buf[..n]).to_string();
        info!("Received RTSP response: {}", response);
        Ok(response)
    }

    async fn handle_auth_response(
        &mut self,
        request_template: &str,
        uri: &str,
        method: &str,
        cseq: u32,
        realm: String,
        nonce: String,
    ) -> Result<String> {
        let auth_header = generate_authorization_header(
            &self.username,
            &self.password,
            &realm,
            &nonce,
            uri,
            method,
        );
        let auth_request = format!(
            "{}\r\nCSeq: {}\r\nAuthorization: {}\r\nUser-Agent: {}\r\n\r\n",
            request_template, cseq, auth_header, USER_AGENT
        );
        self.send_request(&auth_request).await?;
        self.read_response().await
    }

    async fn handle_authenticate(
        &mut self,
        request: &str,
        uri: &str,
        method: &str,
        response: String,
    ) -> Result<String> {
        if response.contains("401 Unauthorized") {
            let auth_line = response
                .lines()
                .find(|line| line.starts_with("WWW-Authenticate"))
                .ok_or_else(|| anyhow!("WWW-Authenticate header not found"))?;
            let (realm, nonce) = parse_auth(auth_line)?;
            self.realm.clone_from(&realm);
            self.nonce.clone_from(&nonce);
            let cseq = self.cseq;
            let response = self
                .handle_auth_response(request, uri, method, cseq, realm, nonce)
                .await?;
            self.cseq += 1;
            Ok(response)
        } else {
            Ok(response)
        }
    }

    async fn send_options(&mut self) -> Result<()> {
        let options_request = format!("OPTIONS {} RTSP/1.0", self.rtsp_url);
        self.send_request(&format!(
            "{}\r\nCSeq: {}\r\nUser-Agent: {}\r\n\r\n",
            &options_request, self.cseq, USER_AGENT
        ))
        .await?;
        let options_response = self.read_response().await?;
        self.cseq += 1;

        if options_response.contains("401 Unauthorized") {
            let response = {
                let options_request = options_request.clone();
                let rtsp_url = self.rtsp_url.clone();
                self.handle_authenticate(&options_request, &rtsp_url, "OPTIONS", options_response)
                    .await?
            };
            self.send_request(&response).await?;
        }

        Ok(())
    }

    async fn send_describe(&mut self) -> Result<String> {
        let describe_request = format!("DESCRIBE {} RTSP/1.0", self.rtsp_url);
        self.send_request(&format!(
            "{}\r\nCSeq: {}\r\nAccept: application/sdp\r\nUser-Agent: {}\r\n\r\n",
            &describe_request, self.cseq, USER_AGENT
        ))
        .await?;
        let describe_response = self.read_response().await?;
        self.cseq += 1;

        let response = if describe_response.contains("401 Unauthorized") {
            let describe_request = describe_request.clone();
            let rtsp_url = self.rtsp_url.clone();
            self.handle_authenticate(&describe_request, &rtsp_url, "DESCRIBE", describe_response)
                .await?
        } else {
            describe_response
        };

        Ok(response)
    }

    async fn send_setup(&mut self, rtp_port: u16, rtcp_port: u16) -> Result<String> {
        let setup_request = format!("SETUP {}/trackID=1 RTSP/1.0", self.rtsp_url);
        let setup_request = format!(
            "{}\r\nCSeq: {}\r\nTransport: RTP/AVP;unicast;client_port={}-{}\r\nAuthorization: Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"\r\nUser-Agent: {}\r\n\r\n",
            setup_request,
            self.cseq,
            rtp_port,
            rtcp_port,
            self.username,
            self.realm,
            self.nonce,
            self.rtsp_url,
            generate_digest_response(&self.username, &self.password, &self.realm, &self.nonce, &self.rtsp_url, "SETUP"),
            USER_AGENT
        );
        self.send_request(&setup_request).await?;
        let setup_response = self.read_response().await?;
        self.cseq += 1;

        let response = if setup_response.contains("401 Unauthorized") {
            let setup_request = setup_request.clone();
            let rtsp_url = self.rtsp_url.clone();
            self.handle_authenticate(&setup_request, &rtsp_url, "SETUP", setup_response)
                .await?
        } else {
            setup_response
        };

        Ok(response)
    }

    async fn send_play(&mut self, session_id: &str) -> Result<String> {
        let play_request = format!("PLAY {} RTSP/1.0", self.rtsp_url);
        let play_request = format!(
            "{}\r\nCSeq: {}\r\nSession: {}\r\nRange: npt=0.000-\r\nAuthorization: Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"\r\nUser-Agent: {}\r\n\r\n",
            play_request,
            self.cseq,
            session_id,
            self.username,
            self.realm,
            self.nonce,
            self.rtsp_url,
            generate_digest_response(&self.username, &self.password, &self.realm, &self.nonce, &self.rtsp_url, "PLAY"),
            USER_AGENT
        );
        self.send_request(&play_request).await?;
        let play_response = self.read_response().await?;
        self.cseq += 1;

        let response = if play_response.contains("401 Unauthorized") {
            let play_request = play_request.clone();
            let rtsp_url = self.rtsp_url.clone();
            self.handle_authenticate(&play_request, &rtsp_url, "PLAY", play_response)
                .await?
        } else {
            play_response
        };

        Ok(response)
    }

    async fn send_keepalive(mut self) -> Result<()> {
        loop {
            let options_request = format!("OPTIONS {} RTSP/1.0", self.rtsp_url);
            self.send_request(&format!(
                "{}\r\nCSeq: {}\r\nUser-Agent: {}\r\n\r\n",
                &options_request, self.cseq, USER_AGENT
            ))
            .await?;
            let options_response = self.read_response().await?;
            self.cseq += 1;

            if options_response.contains("401 Unauthorized") {
                let response = {
                    let options_request = options_request.clone();
                    let rtsp_url = self.rtsp_url.clone();
                    self.handle_authenticate(
                        &options_request,
                        &rtsp_url,
                        "OPTIONS",
                        options_response,
                    )
                    .await?
                };
                self.send_request(&response).await?;
            }

            // Sleep for 30 seconds before sending the next keepalive
            sleep(Duration::from_secs(30)).await;
        }
    }
}

fn parse_auth(header: &str) -> Result<(String, String)> {
    let realm_key = "realm=\"";
    let nonce_key = "nonce=\"";
    let realm_start = header
        .find(realm_key)
        .ok_or_else(|| anyhow!("realm not found"))?
        + realm_key.len();
    let realm_end = header[realm_start..]
        .find('"')
        .ok_or_else(|| anyhow!("realm end not found"))?
        + realm_start;
    let realm = header[realm_start..realm_end].to_string();

    let nonce_start = header
        .find(nonce_key)
        .ok_or_else(|| anyhow!("nonce not found"))?
        + nonce_key.len();
    let nonce_end = header[nonce_start..]
        .find('"')
        .ok_or_else(|| anyhow!("nonce end not found"))?
        + nonce_start;
    let nonce = header[nonce_start..nonce_end].to_string();

    Ok((realm, nonce))
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

fn parse_sdp(sdp: &str) -> Result<(Option<String>, Option<String>, Codec)> {
    let mut video_track = None;
    let mut audio_track = None;
    let mut video_codec = None;

    for line in sdp.lines() {
        if line.starts_with("m=video") {
            for line in sdp.lines() {
                if line.starts_with("a=control:") {
                    video_track = Some(line.replace("a=control:", "").trim().to_string());
                } else if line.starts_with("a=rtpmap:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() > 1 {
                        if parts[1].contains("H264") {
                            video_codec = Some(Codec::H264);
                        } else if parts[1].contains("AV1") {
                            video_codec = Some(Codec::AV1);
                        } else if parts[1].contains("VP8") {
                            video_codec = Some(Codec::Vp8);
                        } else if parts[1].contains("VP9") {
                            video_codec = Some(Codec::Vp9);
                        } else if parts[1].contains("Opus") {
                            video_codec = Some(Codec::Opus);
                        } else if parts[1].contains("G722") {
                            video_codec = Some(Codec::G722);
                        }
                    }
                }
                if video_track.is_some() && video_codec.is_some() {
                    break;
                }
            }
        }
        if line.starts_with("m=audio") {
            for line in sdp.lines() {
                if line.starts_with("a=control:") {
                    audio_track = Some(line.replace("a=control:", "").trim().to_string());
                    break;
                }
            }
        }
    }

    // Ok((video_track, audio_track, video_codec))
    match video_codec {
        Some(codec) => Ok((video_track, audio_track, codec)),
        None => Err(anyhow!("No valid video codec found in SDP")),
    }
}

pub async fn setup_rtsp_session(rtsp_url: &str) -> Result<(u16, Codec)> {
    let mut session = RtspSession::new(rtsp_url).await?;

    // Send OPTIONS
    session.send_options().await?;

    // Send DESCRIBE
    let describe_response = session.send_describe().await?;

    let sdp = describe_response
        .lines()
        .skip_while(|line| !line.is_empty())
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n");

    info!("SDP content: \n{}", sdp);

    let (video_track, audio_track, video_codec) = parse_sdp(&sdp)?;

    if video_track.is_none() && audio_track.is_none() {
        return Err(anyhow!("No tracks found in SDP"));
    }

    let rtp_port = pick_unused_port().ok_or_else(|| anyhow!("No available port found"))?;
    let rtcp_port = rtp_port + 1;

    // Send SETUP
    let setup_response = session.send_setup(rtp_port, rtcp_port).await?;
    let session_id = setup_response
        .lines()
        .find(|line| line.starts_with("Session"))
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("Session ID not found"))?;

    // Send PLAY
    session.send_play(session_id).await?;

    tokio::spawn(async move {
        session.send_keepalive().await.unwrap();
    });

    Ok((rtp_port, video_codec))
}
