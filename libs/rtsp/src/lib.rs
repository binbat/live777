use anyhow::{anyhow, Error, Result};
use cli::{codec_from_str, Codec};
use portpicker::pick_unused_port;
use rtsp_types::ParseError;
use rtsp_types::{headers, headers::transport, Message, Method, Request, Response, StatusCode};
use sdp::{description::common::Attribute, SessionDescription};
use sdp_types::Session;
use std::io::Cursor;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc::UnboundedSender,
};
use tracing::{debug, error, warn};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

const SERVER_NAME: &str = "whipinto";

#[derive(Debug, Clone)]
pub struct Handler {
    sdp: Option<Vec<u8>>,
    media_info: MediaInfo,
    up_tx: UnboundedSender<MediaInfo>,
    dn_tx: UnboundedSender<()>,
}
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub video_rtp_client: Option<u16>,
    pub video_rtcp_client: Option<u16>,
    pub video_rtp_server: Option<u16>,
    pub audio_rtp_client: Option<u16>,
    pub audio_rtcp_client: Option<u16>,
    pub audio_rtp_server: Option<u16>,
    pub video_codec: Option<Codec>,
    pub audio_codec: Option<Codec>,
}

#[derive(Clone, Debug)]
pub struct CodecInfo {
    pub video_codec: Option<RTCRtpCodecParameters>,
    pub audio_codec: Option<RTCRtpCodecParameters>,
}

impl CodecInfo {
    pub fn new() -> Self {
        Self {
            video_codec: None,
            audio_codec: None,
        }
    }
}

impl Default for CodecInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler {
    pub fn new(up_tx: UnboundedSender<MediaInfo>, dn_tx: UnboundedSender<()>) -> Handler {
        Self {
            sdp: None,
            media_info: MediaInfo::default(),
            up_tx,
            dn_tx,
        }
    }

    pub fn set_sdp(&mut self, sdp: Vec<u8>) {
        self.sdp = Some(sdp);
    }

    fn todo(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        unimplemented!("{:?}", req.method());
    }

    fn play(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        self.up_tx
            .send(MediaInfo {
                video_rtp_client: self.media_info.video_rtp_client,
                video_rtcp_client: self.media_info.video_rtcp_client,
                video_rtp_server: self.media_info.video_rtp_server,
                audio_rtp_client: self.media_info.audio_rtp_client,
                audio_rtcp_client: self.media_info.audio_rtcp_client,
                audio_rtp_server: self.media_info.audio_rtp_server,
                video_codec: self.media_info.video_codec,
                audio_codec: self.media_info.audio_codec,
            })
            .unwrap();

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, "whipinto")
            .build(self.sdp.clone().unwrap())
    }

    fn record(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        self.up_tx
            .send(MediaInfo {
                video_rtp_client: self.media_info.video_rtp_client,
                video_rtcp_client: self.media_info.video_rtcp_client,
                video_rtp_server: self.media_info.video_rtp_server,
                audio_rtp_client: self.media_info.audio_rtp_client,
                audio_rtcp_client: self.media_info.audio_rtcp_client,
                audio_rtp_server: self.media_info.audio_rtp_server,
                video_codec: self.media_info.video_codec,
                audio_codec: self.media_info.audio_codec,
            })
            .unwrap();

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, "whipinto")
            .build(self.sdp.clone().unwrap())
    }

    fn describe(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        if self.sdp.is_none() {
            error!("SDP data is none");
        }

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, "whipinto")
            .build(self.sdp.clone().unwrap())
    }

    fn setup(&mut self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        let trs = req
            .typed_header::<transport::Transports>()
            .unwrap()
            .unwrap();
        let tr = trs.first().unwrap();

        if let transport::Transport::Rtp(rtp_transport) = tr {
            let (rtp, rtcp) = rtp_transport.params.client_port.unwrap();
            let uri = req.request_uri().unwrap().as_str();

            let url_id = uri
                .split("streamid=")
                .nth(1)
                .and_then(|id_str| id_str.split('&').next())
                .map(|id| id.to_string());

            if let Some(sdp_data) = &self.sdp {
                let sdp = sdp_types::Session::parse(sdp_data).unwrap();

                for media in sdp.medias.iter() {
                    let media_control = media
                        .attributes
                        .iter()
                        .find(|attr| attr.attribute == "control")
                        .and_then(|attr| attr.value.as_deref())
                        .and_then(|control| {
                            if control.contains("streamid=") {
                                control.split("streamid=").nth(1).map(|id| id.to_string())
                            } else {
                                None
                            }
                        });

                    if media.media == "audio" && media_control.as_deref() == url_id.as_deref() {
                        let audio_server_port =
                            pick_unused_port().expect("Failed to find an unused audio port");
                        let audio_rtcp_server_port = audio_server_port + 1;

                        self.media_info.audio_rtp_client = Some(rtp);
                        self.media_info.audio_rtcp_client = rtcp;
                        self.media_info.audio_rtp_server = Some(audio_server_port);
                        self.media_info.audio_codec = media
                            .attributes
                            .iter()
                            .find(|attr| attr.attribute == "rtpmap")
                            .and_then(|attr| attr.value.as_ref())
                            .and_then(|value| {
                                value
                                    .split_whitespace()
                                    .nth(1)
                                    .unwrap_or("")
                                    .split('/')
                                    .next()
                                    .map(|codec_str| codec_from_str(codec_str).ok())
                            })
                            .unwrap_or(None);

                        return Response::builder(req.version(), StatusCode::Ok)
                            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
                            .header(headers::SERVER, SERVER_NAME)
                            .header(headers::SESSION, "1111-2222-3333-4444")
                            .typed_header(&transport::Transports::from(vec![
                                transport::Transport::Rtp(transport::RtpTransport {
                                    profile: transport::RtpProfile::Avp,
                                    lower_transport: None,
                                    params: transport::RtpTransportParameters {
                                        unicast: true,
                                        server_port: Some((
                                            audio_server_port,
                                            Some(audio_rtcp_server_port),
                                        )),
                                        ..Default::default()
                                    },
                                }),
                            ]))
                            .build(Vec::new());
                    } else if media.media == "video"
                        && media_control.as_deref() == url_id.as_deref()
                    {
                        let video_server_port =
                            pick_unused_port().expect("Failed to find an unused video port");
                        let video_rtcp_server_port = video_server_port + 1;

                        self.media_info.video_rtp_client = Some(rtp);
                        self.media_info.video_rtcp_client = rtcp;
                        self.media_info.video_rtp_server = Some(video_server_port);
                        self.media_info.video_codec = media
                            .attributes
                            .iter()
                            .find(|attr| attr.attribute == "rtpmap")
                            .and_then(|attr| attr.value.as_ref())
                            .and_then(|value| {
                                value
                                    .split_whitespace()
                                    .nth(1)
                                    .unwrap_or("")
                                    .split('/')
                                    .next()
                                    .map(|codec_str| codec_from_str(codec_str).ok())
                            })
                            .unwrap_or(None);

                        return Response::builder(req.version(), StatusCode::Ok)
                            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
                            .header(headers::SERVER, SERVER_NAME)
                            .header(headers::SESSION, "1111-2222-3333-4444")
                            .typed_header(&transport::Transports::from(vec![
                                transport::Transport::Rtp(transport::RtpTransport {
                                    profile: transport::RtpProfile::Avp,
                                    lower_transport: None,
                                    params: transport::RtpTransportParameters {
                                        unicast: true,
                                        server_port: Some((
                                            video_server_port,
                                            Some(video_rtcp_server_port),
                                        )),
                                        ..Default::default()
                                    },
                                }),
                            ]))
                            .build(Vec::new());
                    }
                }
            } else {
                warn!("SDP data is not available");
            }
        }

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, SERVER_NAME)
            .build(Vec::new())
    }

    fn announce(&mut self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        self.set_sdp(req.body().to_vec());
        let sdp = Session::parse(req.body()).unwrap();
        debug!("parsed sdp: {:?}", sdp);

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, SERVER_NAME)
            .build(Vec::new())
    }

    fn teardown(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        self.dn_tx.send(()).unwrap();

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, "whipinto")
            .build(Vec::new())
    }

    fn options(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, SERVER_NAME)
            .typed_header(
                &headers::public::Public::builder()
                    .method(Method::Describe)
                    .method(Method::Announce)
                    .method(Method::Setup)
                    .method(Method::Record)
                    .method(Method::Teardown)
                    .build(),
            )
            .build(Vec::new())
    }
}

pub async fn process_socket(mut socket: TcpStream, handler: &mut Handler) -> Result<(), Error> {
    let (mut reader, mut writer) = socket.split();
    let mut accumulated_buf = Vec::new();

    loop {
        let mut buf = vec![0; 1024];
        match reader.read(&mut buf).await {
            Ok(0) => return Err(anyhow!("Client already closed")),
            Ok(n) => {
                accumulated_buf.extend_from_slice(&buf[..n]);

                match Message::parse(&accumulated_buf) {
                    Ok((message, consumed)) => {
                        accumulated_buf.drain(..consumed);
                        let response = match message {
                            Message::Request(ref request) => match request.method() {
                                // push, pull
                                Method::Options => handler.options(request),
                                // push
                                Method::Announce => handler.announce(request),
                                // pull
                                Method::Describe => handler.describe(request),
                                // push, pull
                                Method::Setup => handler.setup(request),
                                // push
                                Method::Record => handler.record(request),
                                // pull
                                Method::Play => handler.play(request),
                                // push, pull
                                Method::Teardown => handler.teardown(request),
                                _ => handler.todo(request),
                            },
                            _ => continue,
                        };

                        let mut buffer = Vec::new();
                        response.write(&mut buffer)?;
                        writer.write_all(&buffer).await?;
                    }
                    Err(ParseError::Incomplete(_)) => {
                        continue;
                    }
                    Err(e) => {
                        return Err(anyhow!("parse error: {:?}", e));
                    }
                }
            }
            Err(e) => return Err(anyhow!(e)),
        }
    }
}

pub fn filter_sdp(
    webrtc_sdp: &str,
    video_codec: Option<&RTCRtpCodecParameters>,
    audio_codec: Option<&RTCRtpCodecParameters>,
) -> Result<String, String> {
    let mut reader = Cursor::new(webrtc_sdp.as_bytes());
    let mut session = match SessionDescription::unmarshal(&mut reader) {
        Ok(sdp) => sdp,
        Err(e) => return Err(format!("Failed to parse SDP: {:?}", e)),
    };

    session.media_descriptions.retain_mut(|media| {
        if media.media_name.media == "video" {
            if video_codec.is_none() {
                return false;
            } else if let Some(video_codec) = video_codec {
                media
                    .media_name
                    .formats
                    .retain(|fmt| fmt == &video_codec.payload_type.to_string());
                media.attributes.retain(|attr| {
                    attr.key == "rtpmap"
                        && attr.value.as_ref().is_some_and(|v| {
                            v.starts_with(&video_codec.payload_type.to_string())
                        })
                });
                media.media_name.protos = vec!["RTP".to_string(), "AVP".to_string()];
                media.attributes.push(Attribute {
                    key: "control".to_string(),
                    value: Some("streamid=0".to_string()),
                });
            }
        } else if media.media_name.media == "audio" {
            if audio_codec.is_none() {
                return false;
            } else if let Some(audio_codec) = audio_codec {
                media
                    .media_name
                    .formats
                    .retain(|fmt| fmt == &audio_codec.payload_type.to_string());
                media.attributes.retain(|attr| {
                    attr.key == "rtpmap"
                        && attr.value.as_ref().is_some_and(|v| {
                            v.starts_with(&audio_codec.payload_type.to_string())
                        })
                });
                media.media_name.protos = vec!["RTP".to_string(), "AVP".to_string()];
                media.attributes.push(Attribute {
                    key: "control".to_string(),
                    value: Some("streamid=1".to_string()),
                });
            }
        }
    
        true
    });
    
    session.attributes.retain(|attr| {
        !attr.key.starts_with("group")
            && !attr.key.starts_with("fingerprint")
            && !attr.key.starts_with("end-of-candidates")
            && !attr.key.starts_with("setup")
            && !attr.key.starts_with("mid")
            && !attr.key.starts_with("ice-ufrag")
            && !attr.key.starts_with("ice-pwd")
            && !attr.key.starts_with("extmap")
    });

    for media in &mut session.media_descriptions {
        media.attributes.retain(|attr| {
            !attr.key.starts_with("rtcp")
                && !attr.key.starts_with("ssrc")
                && !attr.key.starts_with("candidate")
                && !attr.key.starts_with("fmtp")
                && !attr.key.starts_with("setup")
                && !attr.key.starts_with("mid")
                && !attr.key.starts_with("ice-ufrag")
                && !attr.key.starts_with("ice-pwd")
                && !attr.key.starts_with("extmap")
                && !attr.key.starts_with("end-of-candidates")
        });
    }

    Ok(session.marshal())
}
