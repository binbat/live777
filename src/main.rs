use anyhow::Result;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Router,
};
use hyper::header;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{channel, Sender};
use tower_http::services::{ServeDir, ServeFile};
use webrtc::{
    api::interceptor_registry::register_default_interceptors,
    api::media_engine::{MediaEngine, MIME_TYPE_VP8},
    api::APIBuilder,
    ice_transport::ice_connection_state::RTCIceConnectionState,
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::configuration::RTCConfiguration,
    peer_connection::peer_connection_state::RTCPeerConnectionState,
    peer_connection::sdp::session_description::RTCSessionDescription,
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::track_local_static_rtp::TrackLocalStaticRTP,
    track::track_local::{TrackLocal, TrackLocalWriter},
    Error,
};

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

    //let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);

    //let done_tx1 = done_tx.clone();
    // Set the handler for ICE connection state
    // This will notify you when the peer has connected/disconnected
    peer_connection.on_ice_connection_state_change(Box::new(
        move |connection_state: RTCIceConnectionState| {
            println!("Connection State has changed {connection_state}");
            //if connection_state == RTCIceConnectionState::Failed {
            //    let _ = done_tx1.try_send(());
            //}
            Box::pin(async {})
        },
    ));

    //let done_tx2 = done_tx.clone();
    // Set the handler for Peer connection state
    // This will notify you when the peer has connected/disconnected
    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        println!("Peer Connection State has changed: {s}");

        if s == RTCPeerConnectionState::Failed {
            // Wait until PeerConnection has had no network activity for 30 seconds or another failure. It may be reconnected using an ICE Restart.
            // Use webrtc.PeerConnectionStateDisconnected if you are interested in detecting faster timeout.
            // Note that the PeerConnection may come back from PeerConnectionStateDisconnected.
            println!("Peer Connection has gone to failed exiting: Done forwarding");
            //let _ = done_tx2.try_send(());
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
    peer_connection.set_local_description(answer).await.unwrap();

    // Block until ICE Gathering is complete, disabling trickle ICE
    // we do this because we only can exchange one signaling message
    // in a production application you should exchange ICE Candidates via OnICECandidate
    let _ = gather_complete.recv().await;

    let (tx, mut rx) = channel::<Vec<u8>>(32);

    //let done_tx3 = done_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(err) = video_track.write(&msg).await {
                if Error::ErrClosedPipe == err {
                    // The peerConnection has been closed.
                } else {
                    println!("video_track write err: {err}");
                }
                //let _ = done_tx3.try_send(());
                return;
            }
        }
    });

    //println!("Press ctrl-c to stop");
    //tokio::select! {
    //    _ = done_rx.recv() => {
    //        println!("received done signal!");
    //    }
    //    _ = tokio::signal::ctrl_c() => {
    //        println!();
    //    }
    //};

    // TODO:
    // peer_connection.close().await?;

    (peer_connection.local_description().await.unwrap(), tx)
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
        .route("/whep/endpoint", post(webrtc_handler))
        .nest_service("/", serve_dir.clone())
        .with_state(Arc::clone(&shared_state));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

//ToString -> RTCSessionDescription
async fn webrtc_handler(State(state): State<SharedState>, request: Request) -> impl IntoResponse {
    let body = request.into_body();
    let body_bytes = hyper::body::to_bytes(body).await.unwrap();
    let body_string = String::from_utf8_lossy(&body_bytes).to_string();
    //new RTCSessionDescription
    let whep_offer = RTCSessionDescription::offer(body_string).unwrap();
    let (answer, sender) = start_webrtc(whep_offer).await;
    state.write().unwrap().ch.lock().unwrap().push(sender);
    (
        StatusCode::CREATED,
        [(header::CONTENT_TYPE, "application/sdp")],
        //[(header::LOCATION, url+"whep/endpoint")],
        answer.sdp,
    )
}

type SharedState = Arc<RwLock<AppState>>;

#[derive(Default)]
struct AppState {
    ch: Mutex<Vec<Sender<Vec<u8>>>>,
} //监听rtp
async fn rtp_listener(state: SharedState) {
    let listener = UdpSocket::bind("127.0.0.1:5004").await.unwrap();
    println!("=== RTP listener started ===");

    let mut inbound_rtp_packet = vec![0u8; 1600]; // UDP MTU
    while let Ok((n, _)) = listener.recv_from(&mut inbound_rtp_packet).await {
        let data = inbound_rtp_packet[..n].to_vec();
        let arr = state.read().unwrap().ch.lock().unwrap().clone();
        for c in arr.iter() {
            c.send(data.clone()).await.unwrap();
        }
    }
}
