use anyhow::{anyhow, Error, Result};
use cli::{codec_from_str, Codec};
use portpicker::pick_unused_port;
use rtsp_types::ParseError;
use rtsp_types::{headers, headers::transport, Message, Method, Request, Response, StatusCode};
use sdp::{description::common::Attribute, SessionDescription};
use sdp_types::Session;
use std::io::Cursor;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
};

use tracing::{debug, error, warn};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

const SERVER_NAME: &str = "whipinto";
type InterleavedSender = Arc<tokio::sync::Mutex<UnboundedSender<(u8, Vec<u8>)>>>;
type InterleavedReceiver = Arc<tokio::sync::Mutex<UnboundedReceiver<(u8, Vec<u8>)>>>;

#[derive(Debug, Clone)]
pub struct Handler {
    sdp: Option<Vec<u8>>,
    media_info: MediaInfo,
    up_tx: UnboundedSender<MediaInfo>,
    dn_tx: UnboundedSender<()>,
    pub interleaved_tx: Option<InterleavedSender>,
    pub interleaved_rx: Option<InterleavedReceiver>,
}

#[derive(Debug, Clone)]
pub enum TransportInfo {
    Tcp {
        rtp_channel: u8,
        rtcp_channel: u8,
    },
    Udp {
        rtp_send_port: Option<u16>,
        rtp_recv_port: Option<u16>,
        rtcp_send_port: Option<u16>,
        rtcp_recv_port: Option<u16>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub video_transport: Option<TransportInfo>,
    pub audio_transport: Option<TransportInfo>,
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
            interleaved_tx: None,
            interleaved_rx: None,
        }
    }

    pub fn set_interleaved_sender(&mut self, tx: UnboundedSender<(u8, Vec<u8>)>) {
        self.interleaved_tx = Some(Arc::new(tokio::sync::Mutex::new(tx)));
    }

    pub fn set_interleaved_receiver(&mut self, rx: UnboundedReceiver<(u8, Vec<u8>)>) {
        self.interleaved_rx = Some(Arc::new(tokio::sync::Mutex::new(rx)));
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
                video_transport: self.media_info.video_transport.clone(),
                audio_transport: self.media_info.audio_transport.clone(),
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
                video_transport: self.media_info.video_transport.clone(),
                audio_transport: self.media_info.audio_transport.clone(),
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
            if rtp_transport.lower_transport == Some(transport::RtpLowerTransport::Tcp) {
                let interleaved = rtp_transport.params.interleaved.unwrap_or((0, Some(1)));
                let url = req.request_uri().unwrap().as_str();

                let url_id = url
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
                            debug!("Setting up audio stream with TCP transport, interleaved channels: {:?}", interleaved);

                            self.media_info.audio_transport = Some(TransportInfo::Tcp {
                                rtp_channel: interleaved.0,
                                rtcp_channel: interleaved.1.unwrap_or(1),
                            });

                            self.media_info.audio_codec = extract_codec(media);

                            return Response::builder(req.version(), StatusCode::Ok)
                                .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
                                .header(headers::SERVER, SERVER_NAME)
                                .header(headers::SESSION, "1111-2222-3333-4444")
                                .typed_header(&transport::Transports::from(vec![
                                    transport::Transport::Rtp(transport::RtpTransport {
                                        profile: transport::RtpProfile::Avp,
                                        lower_transport: Some(transport::RtpLowerTransport::Tcp),
                                        params: transport::RtpTransportParameters {
                                            unicast: true,
                                            interleaved: Some(interleaved),
                                            ..Default::default()
                                        },
                                    }),
                                ]))
                                .build(Vec::new());
                        } else if media.media == "video"
                            && media_control.as_deref() == url_id.as_deref()
                        {
                            debug!("Setting up video stream with TCP transport, interleaved channels: {:?}", interleaved);

                            self.media_info.video_transport = Some(TransportInfo::Tcp {
                                rtp_channel: interleaved.0,
                                rtcp_channel: interleaved.1.unwrap_or(1),
                            });

                            self.media_info.video_codec = extract_codec(media);

                            return Response::builder(req.version(), StatusCode::Ok)
                                .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
                                .header(headers::SERVER, SERVER_NAME)
                                .header(headers::SESSION, "1111-2222-3333-4444")
                                .typed_header(&transport::Transports::from(vec![
                                    transport::Transport::Rtp(transport::RtpTransport {
                                        profile: transport::RtpProfile::Avp,
                                        lower_transport: Some(transport::RtpLowerTransport::Tcp),
                                        params: transport::RtpTransportParameters {
                                            unicast: true,
                                            interleaved: Some(interleaved),
                                            ..Default::default()
                                        },
                                    }),
                                ]))
                                .build(Vec::new());
                        }
                    }
                } else {
                    warn!("SDP data is not available for TCP setup");
                }
            } else {
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

                            self.media_info.audio_transport = Some(TransportInfo::Udp {
                                rtp_send_port: Some(rtp),
                                rtp_recv_port: Some(audio_server_port),
                                rtcp_send_port: rtcp,
                                rtcp_recv_port: Some(audio_rtcp_server_port),
                            });

                            self.media_info.audio_codec = extract_codec(media);

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

                            self.media_info.video_transport = Some(TransportInfo::Udp {
                                rtp_send_port: Some(rtp),
                                rtp_recv_port: Some(video_server_port),
                                rtcp_send_port: rtcp,
                                rtcp_recv_port: Some(video_rtcp_server_port),
                            });

                            self.media_info.video_codec = extract_codec(media);

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

pub async fn send_interleaved_frame(
    writer: &mut (impl AsyncWriteExt + Unpin),
    channel: u8,
    data: &[u8],
) -> Result<()> {
    let mut frame = vec![
        b'$',
        channel,
        ((data.len() >> 8) & 0xFF) as u8,
        (data.len() & 0xFF) as u8,
    ];
    frame.extend_from_slice(data);
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn process_socket(socket: TcpStream, handler: &mut Handler) -> Result<(), Error> {
    let (reader, writer) = tokio::io::split(socket);

    let writer = Arc::new(tokio::sync::Mutex::new(writer));

    let interleaved_rx = match handler.interleaved_rx.take() {
        Some(rx) => rx,
        None => {
            let (_, rx) = unbounded_channel::<(u8, Vec<u8>)>();
            Arc::new(tokio::sync::Mutex::new(rx))
        }
    };
    let writer_clone = writer.clone();
    let _writer_task = tokio::spawn(async move {
        loop {
            let mut rx_guard = interleaved_rx.lock().await;
            match rx_guard.recv().await {
                Some((channel, data)) => {
                    debug!(
                        "Sending interleaved frame on channel {}, {} bytes",
                        channel,
                        data.len()
                    );
                    let mut writer_guard = writer_clone.lock().await;
                    if let Err(e) = send_interleaved_frame(&mut *writer_guard, channel, &data).await
                    {
                        error!("Failed to send interleaved frame: {}", e);
                        break;
                    }
                }
                None => {
                    warn!("Interleaved receiver closed");
                    break;
                }
            }
        }
        warn!("Writer task stopped");
    });
    let mut reader = BufReader::new(reader);
    let mut accumulated_buf = Vec::new();

    loop {
        let mut buf = vec![0; 1024];
        match reader.read(&mut buf).await {
            Ok(0) => return Err(anyhow!("Client already closed")),
            Ok(n) => {
                accumulated_buf.extend_from_slice(&buf[..n]);

                while accumulated_buf.len() >= 4 && accumulated_buf[0] == b'$' {
                    let channel = accumulated_buf[1];
                    let len = ((accumulated_buf[2] as usize) << 8) | (accumulated_buf[3] as usize);
                    if accumulated_buf.len() < 4 + len {
                        break;
                    }
                    let rtp_data = accumulated_buf[4..4 + len].to_vec();
                    if let Some(tx) = &handler.interleaved_tx {
                        if let Ok(tx_guard) = tx.try_lock() {
                            if let Err(e) = tx_guard.send((channel, rtp_data.clone())) {
                                error!("Failed to forward interleaved data: {}", e);
                            }
                        } else {
                            match tx.lock().await.send((channel, rtp_data)) {
                                Ok(_) => {}
                                Err(e) => error!("Failed to forward interleaved data: {}", e),
                            }
                        }
                    }

                    accumulated_buf.drain(..4 + len);
                }

                match Message::parse(&accumulated_buf) {
                    Ok((message, consumed)) => {
                        debug!("Received RTSP message: {:?}", message);
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
                        //writer.write_all(&buffer).await?;
                        let mut writer_guard = writer.lock().await;
                        writer_guard.write_all(&buffer).await?;
                        writer_guard.flush().await?;
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
                        && attr
                            .value
                            .as_ref()
                            .is_some_and(|v| v.starts_with(&video_codec.payload_type.to_string()))
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
                        && attr
                            .value
                            .as_ref()
                            .is_some_and(|v| v.starts_with(&audio_codec.payload_type.to_string()))
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

pub fn extract_codec(media: &sdp_types::Media) -> Option<Codec> {
    media
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
        .unwrap_or(None)
}
