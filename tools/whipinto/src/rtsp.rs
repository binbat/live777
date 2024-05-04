use anyhow::{anyhow, Error, Result};
use rtsp_types::{headers, headers::transport, Message, Method, Request, Response, StatusCode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc::UnboundedSender,
};

const SERVER_NAME: &str = "whipinto";

fn handler_todo(req: &Request<Vec<u8>>) -> Response<rtsp_types::Empty> {
    Response::builder(req.version(), StatusCode::Ok)
        .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
        .header(headers::SERVER, "whipinto")
        .empty()
}

fn handler_setup(req: &Request<Vec<u8>>) -> Response<rtsp_types::Empty> {
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
        .empty()
}

fn handler_announce(
    req: &Request<Vec<u8>>,
    tx: UnboundedSender<String>,
) -> Response<rtsp_types::Empty> {
    // sdp-types = "0.1.6"
    // https://crates.io/crates/sdp-types
    // let sdp = sdp_types::Session::parse(req.body()).unwrap();
    // let rtpmap = sdp.medias.first().unwrap().attributes.first().unwrap().value.clone().unwrap_or("".to_string());

    // webrtc-sdp
    let sdp = sdp::description::session::SessionDescription::unmarshal(&mut std::io::Cursor::new(
        req.body(),
    ))
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
    tx.send(rtpmap).unwrap();

    Response::builder(req.version(), StatusCode::Ok)
        .header(headers::CSEQ, req.header(&headers::CSEQ).unwrap().as_str())
        .header(headers::SERVER, SERVER_NAME)
        .empty()
}

fn handler_options(req: &Request<Vec<u8>>) -> Response<rtsp_types::Empty> {
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
        .empty()
}

pub async fn process_socket(
    mut socket: TcpStream,
    tx: UnboundedSender<String>,
) -> Result<(), Error> {
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
                            Method::Options => handler_options(request),
                            Method::Announce => handler_announce(request, tx.clone()),
                            Method::Setup => handler_setup(request),
                            Method::Teardown => handler_todo(request),
                            _ => handler_todo(request),
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
