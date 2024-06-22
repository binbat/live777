use anyhow::{anyhow, Error, Result};
use portpicker::pick_unused_port;
use rtsp_types::ParseError;
use rtsp_types::{headers, headers::transport, Message, Method, Request, Response, StatusCode};
use sdp_types::Session;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc::UnboundedSender,
};

const SERVER_NAME: &str = "whipinto";

#[derive(Debug, Clone)]
pub struct Handler {
    sdp: Option<Vec<u8>>,
    rtp: Option<u16>,
    rtcp: Option<u16>,
    up_tx: UnboundedSender<String>,
    dn_tx: UnboundedSender<()>,
}

impl Handler {
    pub fn new(up_tx: UnboundedSender<String>, dn_tx: UnboundedSender<()>) -> Handler {
        Self {
            sdp: None,
            rtp: None,
            rtcp: None,
            up_tx,
            dn_tx,
        }
    }

    pub fn set_sdp(&mut self, sdp: Vec<u8>) {
        self.sdp = Some(sdp);
    }

    pub fn get_rtp(&self) -> u16 {
        self.rtp.unwrap()
    }

    pub fn get_rtcp(&self) -> u16 {
        self.rtcp.unwrap()
    }

    fn todo(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        unimplemented!("{:?}", req.method());
    }

    fn play(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, "whipinto")
            .build(self.sdp.clone().unwrap())
    }

    fn record(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, "whipinto")
            .build(self.sdp.clone().unwrap())
    }

    fn describe(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        if self.sdp.is_none() {
            println!("sdp is none");
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

        match tr {
            transport::Transport::Rtp(rtp_transport) => {
                println!("rtp_transport {:?}", rtp_transport);
                let (rtp, rtcp) = rtp_transport.params.client_port.unwrap();
                println!("rtp: {:?}, rtcp: {:?}", rtp, rtcp);
                self.rtp = Some(rtp);
                self.rtcp = rtcp;
                self.up_tx.send(rtp.to_string()).unwrap();
                self.up_tx.send(rtcp.unwrap().to_string()).unwrap();
            }
            transport::Transport::Other(other_transport) => {
                println!("other_transport {:?}", other_transport);
            }
        };
        let rtp_server_port = pick_unused_port().expect("Failed to find an unused port");
        let rtcp_server_port = rtp_server_port + 1;
        self.up_tx.send(rtp_server_port.to_string()).unwrap();

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, SERVER_NAME)
            .header(headers::SESSION, "1111-2222-3333-4444")
            .typed_header(&transport::Transports::from(vec![
                transport::Transport::Rtp(transport::RtpTransport {
                    profile: transport::RtpProfile::Avp,
                    lower_transport: None,
                    params: transport::RtpTransportParameters {
                        unicast: true,
                        //client_port: Some((18704, Some(18705))),
                        server_port: Some((rtp_server_port, Some(rtcp_server_port))),
                        ..Default::default()
                    },
                }),
            ]))
            .build(Vec::new())
    }

    fn announce(&mut self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        self.set_sdp(req.body().to_vec());
        let sdp = Session::parse(req.body()).unwrap();
        println!("parsed sdp: {:?}", sdp);
        // self.set_sdp(req.body().to_vec());
        // sdp-types = "0.1.6"
        // https://crates.io/crates/sdp-types
        // let sdp = sdp_types::Session::parse(req.body()).unwrap();
        // let rtpmap = sdp.medias.first().unwrap().attributes.first().unwrap().value.clone().unwrap_or("".to_string());

        // webrtc-sdp
        // let sdp = sdp::description::session::SessionDescription::unmarshal(
        //     &mut std::io::Cursor::new(req.body()),
        // )
        // .unwrap();
        // println!("{:?}", sdp);
        //let rtpmap = sdp
        //    .media_descriptions
        //    .first()
        //    .unwrap()
        //    .attributes
        //    .first()
        //    .unwrap()
        //    .value
        //    .clone()
        //    .unwrap_or("".to_string());

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
                        println!("parse error: {:?}", e);
                        return Err(anyhow!("parse error: {:?}", e));
                    }
                }
            }
            Err(e) => return Err(anyhow!(e)),
        }
    }
}
