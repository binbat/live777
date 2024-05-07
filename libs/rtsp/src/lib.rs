use anyhow::{anyhow, Error, Result};
use rtsp_types::{headers, headers::transport, Message, Method, Request, Response, StatusCode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc::UnboundedSender,
};

const SERVER_NAME: &str = "whipinto";

pub struct Handler {
    sdp: Option<Vec<u8>>,
    rtp: Option<u16>,
    tx: UnboundedSender<String>,
}

impl Handler {
    pub fn new(tx: UnboundedSender<String>) -> Handler {
        Self {
            sdp: None,
            rtp: None,
            tx: tx,
        }
    }

    pub fn set_sdp(&mut self, sdp: Vec<u8>) {
        self.sdp = Some(sdp);
    }

    pub fn get_rtp(&self) -> u16 {
        self.rtp.unwrap()
    }

    fn todo(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, "whipinto")
            .build(Vec::new())
    }

    fn describe(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        println!("describe");
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
                self.tx.send(rtp.to_string()).unwrap();
            }
            transport::Transport::Other(other_transport) => {
                println!("other_transport {:?}", other_transport);
            }
        };

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
                        server_port: Some((8000, Some(8001))),
                        ..Default::default()
                    },
                }),
            ]))
            .build(Vec::new())
    }

    fn announce(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        // sdp-types = "0.1.6"
        // https://crates.io/crates/sdp-types
        // let sdp = sdp_types::Session::parse(req.body()).unwrap();
        // let rtpmap = sdp.medias.first().unwrap().attributes.first().unwrap().value.clone().unwrap_or("".to_string());

        // webrtc-sdp
        let sdp = sdp::description::session::SessionDescription::unmarshal(
            &mut std::io::Cursor::new(req.body()),
        )
        .unwrap();
        let rtpmap = sdp
            .media_descriptions
            .first()
            .unwrap()
            .attributes
            .first()
            .unwrap()
            .value
            .clone()
            .unwrap_or("".to_string());

        println!("{:?}", sdp);
        self.tx.send(rtpmap).unwrap();

        Response::builder(req.version(), StatusCode::Ok)
            .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
            .header(headers::SERVER, SERVER_NAME)
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
    let mut buf = vec![0; 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => return Err(anyhow!("Client already closed")),
            Ok(n) => {
                let (message, _consumed): (Message<Vec<u8>>, _) = match Message::parse(&buf[..n]) {
                    Ok(m) => m,
                    Err(e) => {
                        println!("{:?}", e);
                        continue;
                    }
                };

                match message {
                    Message::Request(ref request) => {
                        let response = match request.method() {
                            Method::Options => handler.options(request),
                            Method::Describe => handler.describe(request),
                            Method::Announce => handler.announce(request),
                            Method::Setup => handler.setup(request),
                            Method::Teardown => handler.todo(request),
                            _ => handler.todo(request),
                        };

                        let mut buffer: Vec<u8> = Vec::new();
                        response.write(&mut buffer)?;
                        writer.write_all(&buffer).await?;
                    }
                    _ => unreachable!(),
                }
            }
            Err(e) => return Err(anyhow!(e)),
        }
    }
}
