use std::borrow::Borrow;
use std::net::SocketAddr;
use anyhow::Result;
use std::sync::{Arc, Mutex, RwLock};
use axum::{extract::Json, handler::post, response::IntoResponse, AddExtensionLayer, Router};
use axum::extract::State;
use tokio::net::{UdpSocket, ToSocketAddrs};
use tokio::sync::mpsc::{channel, Sender};
use tower_http::services::{ServeDir, ServeFile};
use webrtc::{
    api::interceptor_registry::register_default_interceptors,
    api::media_engine::{MediaEngine, MIME_TYPE_VP8},
    api::APIBuilder,
    ice_transport::ice_connection_state::RTCIceConnectionState,
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    media::rtp::{rtp_receiver::RTPReceiver, rtp_transceiver::RTPTransceiverDirection},
    peer_connection::configuration::RTCConfiguration,
    peer_connection::peer_connection_state::RTCPeerConnectionState,
    peer_connection::sdp::session_description::RTCSessionDescription,
    rtp_transceiver::RTPTransceiverInit,
    track::track_local::track_local_static_rtp::TrackLocalStaticRTP,
    track::track_local::{TrackLocal, TrackLocalWriter},
    Error,
};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use rocket::http::hyper::Server;
use rocket::{Data, Response, State};
use rocket::http::Status;
use rocket::Data;
use rocket::Response;
use rocket::State;
use serde::{Deserialize, Serialize};
use axum::handler::{post, Handler};
use axum::http::Response;
use webrtc::turn::proto::data::Data;
use serde::Deserialize;
use webrtc_rs::sdp::RTCSessionDescription;
use webrtc_rs::start_webrtc;




async fn start_webrtc(offer: RTCSessionDescription) -> (RTCSessionDescription, Sender<Vec<u8>>) {
    // Create a MediaEngine object to configure the supported codec
    let mut m = MediaEngine::default();

    m.register_default_codecs().unwrap();

    // Create a InterceptorRegistry. This is the user configurable RTP/RTCP Pipeline.
    // This provides NACKs, RTCP Reports and other features. If you use `webrtc.NewPeerConnection`
    // this is enabled by default. If you are manually managing You MUST create a InterceptorRegistry
    // for each PeerConnection.
    let mut registry = Registry::new();

    // Use the default set of Interceptors
    registry = register_default_interceptors(registry, &mut m).unwrap();

    // Create the API object with the MediaEngine
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    // Prepare the configuration
    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };

    // Create a new RTCPeerConnection
    let peer_connection = Arc::new(api.new_peer_connection(config).await.unwrap());

    // Create Track that we send video back to browser on
    let video_track = Arc::new(TrackLocalStaticRTP::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_VP8.to_owned(),
            ..Default::default()
        },
        "video".to_owned(),
        "webrtc-rs".to_owned(),
    ));

    // Add this newly created track to the PeerConnection
    let rtp_sender = peer_connection
        .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
        .await
        .unwrap();

    // Read incoming RTCP packets
    // Before these packets are returned they are processed by interceptors. For things
    // like NACK this needs to be called.
    tokio::spawn(async move {
        let mut rtcp_buf = vec![0u8; 1500];
        while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
        Result::<()>::Ok(())
    });

    // Set the handler for ICE connection state
    // This will notify you when the peer has connected/disconnected
    peer_connection.on_ice_connection_state_change(Box::new(
        move |connection_state: RTCIceConnectionState| {
            println!("Connection State has changed: {:?}", connection_state);
            Box::pin(async {})
        },
    ));

    // Set the handler for Peer connection state
    // This will notify you when the peer has connected/disconnected
    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        println!("Peer Connection State has changed: {:?}", s);

        if s == RTCPeerConnectionState::Failed {
            println!("Peer Connection has gone to failed exiting: Done forwarding");
        }

        Box::pin(async {})
    }));

    // Set the remote SessionDescription
    peer_connection.set_remote_description(offer).await.unwrap();

    // Create an answer
    let answer = peer_connection.create_answer(None).await.unwrap();

    // Create channel that is blocked until ICE Gathering is complete
    let mut gather_complete = peer_connection.gathering_complete_promise().await;

    // Sets the LocalDescription, and starts our UDP listeners
    peer_connection.set_local_description(answer.clone()).await.unwrap();

    // Block until ICE Gathering is complete, disabling trickle ICE
    // we do this because we only can exchange one signaling message
    // in a production application you should exchange ICE Candidates via OnICECandidate
    let _ = gather_complete.recv().await;

    let (tx, mut rx) = channel::<Vec<u8>>(32);

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(err) = video_track.write(&msg).await {
                if Error::ErrClosedPipe == err {
                    // The peerConnection has been closed.
                } else {
                    println!("video_track write err: {:?}", err);
                }
                return;
            }
        }
    });

    (answer, tx)
}

#[tokio::main]
async fn main() {
    let shared_state = SharedState::default();
    let state = Arc::clone(&shared_state);

    tokio::spawn(async move {
        rtp_listener(state).await;
    });

    let serve_dir = ServeDir::new("assets").not_found_service(ServeFile::new("assets/index.html"));

    // build our application with a route
    let app = Router::new()
        .route("/webrtc", post(webrtc_handler))
        .route("/whip", post(webrtc_handler))
        .nest_service("/", serve_dir.clone())
        .layer(AddExtensionLayer::new(shared_state));

    // run our app with hyper, listening globally on port 3000
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    axum::Server::bind(&addr.into())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn webrtc_handler(
    State(state): State<SharedState>,
    Data(offer): Data,
) -> Response<'static> {
    let offer_str = String::from_utf8_lossy(&offer).to_string();

    let offer = RTCSessionDescription::new(offer_str.clone(), "offer".to_string());
    let (mut answer, sender) = start_webrtc(offer).await;

    if let Some(media) = answer.sdp.media.get_mut(0) {
        media.attribute.push("recvonly".to_owned());
    }
    state.write().unwrap().ch.lock().unwrap().push(sender);

    let sdp_answer = answer.to_string();

    Response::build()
        .status(Status::Created)
        .header(rocket::http::ContentType::new("application", "sdp")) // 修改Content-Type
        .sized_body((sdp_answer.len(), std::io::Cursor::new(sdp_answer)))
        .finalize()
}

type SharedState = Arc<RwLock<AppState>>;

#[derive(Default)]
struct AppState {
    ch: Mutex<Vec<Sender<Vec<u8>>>>,
}

async fn rtp_listener(state: SharedState) {
    let socket_addr = "0.0.0.0:5000".parse().unwrap();
    let mut listener = UdpSocket::bind(socket_addr).await.unwrap();

    let mut inbound_rtp_packet = vec![0u8; 1600]; // UDP MTU
    while let Ok((n, _)) = listener.recv_from(&mut inbound_rtp_packet).await {
        let data = inbound_rtp_packet[..n].to_vec();
        let arr = state.read().unwrap().ch.lock().unwrap().clone();
        for c in arr.iter() {
            c.send(data.clone()).await.unwrap();
        }
    }
}
