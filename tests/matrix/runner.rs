//! Shared matrix runner: one copy of the liveion lifecycle, port allocation,
//! publish/subscribe wait loops and playback validation used by every
//! source × player matrix case.

use std::collections::HashSet;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{LazyLock, Mutex, Once};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::player::{PlayResult, Player};
use crate::profile::MediaProfile;
use crate::source::{Source, SourceHandle};

use crate::common::shutdown_signal;

#[cfg(feature = "rtsp")]
use crate::probe;

/// RTSP transport variant used by the round-trip matrix cases: both the
/// FFmpeg push and the ffprobe pull use it.
#[cfg(feature = "rtsp")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtspTransport {
    Udp,
    Tcp,
}

#[cfg(feature = "rtsp")]
impl RtspTransport {
    /// Arguments for the ffprobe pull.
    pub fn ffprobe_args(&self) -> &[&str] {
        match self {
            RtspTransport::Udp => &[],
            RtspTransport::Tcp => &["-rtsp_transport", "tcp"],
        }
    }

    /// Arguments for the FFmpeg RTSP push.
    pub fn ffmpeg_args(&self) -> &[&str] {
        self.ffprobe_args()
    }

    /// `?transport=` query suffix for livetwo's RTSP client URLs.
    pub fn query_param(&self) -> &'static str {
        match self {
            RtspTransport::Udp => "",
            RtspTransport::Tcp => "?transport=tcp",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RtspTransport::Udp => "udp",
            RtspTransport::Tcp => "tcp",
        }
    }
}

/// Liveion RTSP server round-trip: push a source via RTSP ANNOUNCE/RECORD and
/// validate it by pulling from liveion's own RTSP pull side with ffprobe —
/// no WHIP/WHEP involved. Covers the former tests/rtsp.rs topology.
#[cfg(feature = "rtsp")]
pub async fn run_rtsp_roundtrip<S: Source>(source: S, transport: RtspTransport, bind_ip: IpAddr) {
    init_liveion_test_environment();

    let profile = source.profile();
    let rtsp_port = reserve_and_release_tcp_port(bind_ip);

    let mut cfg = liveion::config::Config::default();
    cfg.rtsp.listen = SocketAddr::new(bind_ip, rtsp_port).to_string();

    let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0))
        .await
        .unwrap();
    let api_addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    // liveion's RTSP server binds inside a spawned task — wait until the
    // port is accepting connections before starting ffmpeg.
    let rtsp_addr = SocketAddr::new(bind_ip, rtsp_port);
    for i in 0..50 {
        match tokio::net::TcpStream::connect(rtsp_addr).await {
            Ok(_) => break,
            Err(_) if i == 49 => {
                panic!("RTSP server did not start on {rtsp_addr} after 5 s");
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }

    let rtsp_host = match bind_ip {
        IpAddr::V6(_) => format!("[{bind_ip}]"),
        _ => bind_ip.to_string(),
    };
    let rtsp_url = format!("rtsp://{rtsp_host}:{rtsp_port}/-");

    let source_handle = source
        .start_rtsp_with_transport(&rtsp_url, transport)
        .expect("Failed to start RTSP source");

    let start = tokio::time::Instant::now();

    wait_stream_publish_ready(&rtsp_addr, &api_addr, "-", None).await;

    // Wait a moment for media to flow through to the pull side.
    tokio::time::sleep(Duration::from_secs(1)).await;

    // ffprobe pulls from liveion's RTSP server pull side.
    let mut probe_args: Vec<&str> = transport.ffprobe_args().to_vec();
    probe_args.extend(["-i", rtsp_url.as_str()]);
    let probe_result = probe::run(&probe_args)
        .await
        .expect("ffprobe pull from liveion RTSP server failed");

    let duration_ms = start.elapsed().as_millis() as u64;
    let playback = probe::into_play_result(probe_result, &profile, true, duration_ms);

    tracing::info!(
        source = source.name(),
        transport = transport.as_str(),
        ?playback,
        "RTSP round-trip result"
    );

    assert_playback_ok("rtsp-roundtrip", &profile, &playback);

    source_handle.stop().await;
}

/// RTSP conversion cycle (former tests/rtsp2.rs topology):
///
/// ```text
/// ffmpeg --RTSP--> liveion(cycle-a)
///   --rtsp pull--> whipinto --WHIP--> liveion(cycle-b)
///     --WHEP--> whepfrom --RTSP--> liveion(cycle-c) <-- ffprobe
/// ```
///
/// The transport variant applies to both livetwo client hops and the final
/// ffprobe pull.
#[cfg(feature = "rtsp")]
pub async fn run_rtsp_cycle<S: Source>(source: S, transport: RtspTransport, bind_ip: IpAddr) {
    init_liveion_test_environment();

    let profile = source.profile();
    let rtsp_port = reserve_and_release_tcp_port(bind_ip);

    let mut cfg = liveion::config::Config::default();
    cfg.rtsp.listen = SocketAddr::new(bind_ip, rtsp_port).to_string();

    let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0))
        .await
        .unwrap();
    let api_addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    // liveion's RTSP server binds inside a spawned task — wait until the
    // port is accepting connections before starting ffmpeg.
    let rtsp_addr = SocketAddr::new(bind_ip, rtsp_port);
    for i in 0..50 {
        match tokio::net::TcpStream::connect(rtsp_addr).await {
            Ok(_) => break,
            Err(_) if i == 49 => {
                panic!("RTSP server did not start on {rtsp_addr} after 5 s");
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }

    let rtsp_host = match bind_ip {
        IpAddr::V6(_) => format!("[{bind_ip}]"),
        _ => bind_ip.to_string(),
    };
    let rtsp_url_a = format!("rtsp://{rtsp_host}:{rtsp_port}/cycle-a");

    let start = tokio::time::Instant::now();

    // Stream A: ffmpeg → liveion RTSP push.
    let source_handle = source
        .start_rtsp_with_transport(&rtsp_url_a, transport)
        .expect("Failed to start RTSP source");
    wait_stream_publish_ready(&rtsp_addr, &api_addr, "cycle-a", None).await;

    let ct = CancellationToken::new();
    let transport_param = transport.query_param();

    // Stream A → B: livetwo RTSP client pull + WHIP publish.
    let mut handle_whip = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        format!("{rtsp_url_a}{transport_param}"),
        format!("http://{api_addr}{}", api::path::whip("cycle-b")),
        None,
        None,
    ));
    wait_stream_publish_ready(&rtsp_addr, &api_addr, "cycle-b", Some(&mut handle_whip)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Stream B → C: livetwo WHEP subscribe + RTSP client push.
    let rtsp_url_c = format!("rtsp://{rtsp_host}:{rtsp_port}/cycle-c");
    let mut handle_whep = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        format!("{rtsp_url_c}{transport_param}"),
        format!("http://{api_addr}{}", api::path::whep("cycle-b")),
        None,
        None,
        None,
        None,
    ));
    wait_stream_publish_ready(&rtsp_addr, &api_addr, "cycle-c", Some(&mut handle_whep)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify: ffprobe pulls stream C from liveion's RTSP pull side.
    let mut probe_args: Vec<&str> = transport.ffprobe_args().to_vec();
    probe_args.extend(["-i", rtsp_url_c.as_str()]);
    let probe_result = probe::run(&probe_args)
        .await
        .expect("ffprobe pull from cycle-c failed");

    let duration_ms = start.elapsed().as_millis() as u64;
    let playback = probe::into_play_result(probe_result, &profile, true, duration_ms);

    tracing::info!(
        source = source.name(),
        transport = transport.as_str(),
        ?playback,
        "RTSP cycle result"
    );

    assert_playback_ok("rtsp-cycle", &profile, &playback);

    ct.cancel();
    let result_whip = handle_whip.await.unwrap();
    let result_whep = handle_whep.await.unwrap();
    assert!(result_whip.is_ok(), "whip task failed: {result_whip:?}");
    assert!(result_whep.is_ok(), "whep task failed: {result_whep:?}");

    source_handle.stop().await;
}

/// Wait until a stream's publish session is Connected and liveion has
/// learned its codecs.
#[cfg(feature = "rtsp")]
async fn wait_stream_publish_ready(
    rtsp_addr: &SocketAddr,
    api_addr: &SocketAddr,
    stream_id: &str,
    mut handle: Option<&mut tokio::task::JoinHandle<anyhow::Result<()>>>,
) {
    let mut last_state = None;
    let mut last_codecs = Vec::new();
    for attempt in 0..300 {
        if let Some(h) = handle.as_mut()
            && h.is_finished()
        {
            let result = h.await.unwrap();
            panic!(
                "task exited before publish connected on {stream_id}: result={result:?}, rtsp={rtsp_addr}, last_state={last_state:?}, last_codecs={last_codecs:?}"
            );
        }

        let res = reqwest::get(format!("http://{api_addr}{}", api::path::streams("")))
            .await
            .unwrap();
        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();
        if let Some(r) = body.into_iter().find(|i| i.id == stream_id)
            && !r.publish.sessions.is_empty()
        {
            last_state = Some(r.publish.sessions[0].state);
            last_codecs = r.codecs.clone();
            if r.publish.sessions[0].state == api::response::RTCPeerConnectionState::Connected
                && !r.codecs.is_empty()
            {
                return;
            }
        }

        if attempt == 299 {
            panic!(
                "Stream '{stream_id}' did not reach Connected with codecs; last_state={last_state:?}, last_codecs={last_codecs:?}"
            );
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

static TRACING_INIT: Once = Once::new();

pub fn init_liveion_test_environment() {
    TRACING_INIT.call_once(|| {
        // These tests run both WebRTC peers locally. Pin ICE candidates to
        // loopback so CI runners cannot choose an unroutable host interface.
        unsafe {
            std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
        }

        let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| {
            "matrix=info,live777=info,liveion=info,livetwo=info,libwish=info".to_string()
        });
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

static ALLOCATED_UDP_PORTS: LazyLock<Mutex<HashSet<u16>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Allocate `count` consecutive UDP ports and reserve them in this test
/// process so concurrent cases cannot reuse them. Each RTP flow also uses the
/// next consecutive port for RTCP, so tracks are allocated in pairs.
pub fn alloc_udp_ports(ip: IpAddr, count: u16) -> u16 {
    let mut allocated = ALLOCATED_UDP_PORTS.lock().unwrap();

    for _ in 0..1000 {
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).unwrap();
        let base_port = socket.local_addr().unwrap().port();
        drop(socket);
        if base_port > u16::MAX - count {
            continue;
        }

        let ports = base_port..base_port + count;
        if ports.clone().any(|port| allocated.contains(&port)) {
            continue;
        }
        if ports
            .clone()
            .all(|port| UdpSocket::bind(SocketAddr::new(ip, port)).is_ok())
        {
            allocated.extend(ports);
            return base_port;
        }
    }

    panic!("failed to allocate {count} available UDP ports for {ip}");
}

/// Reserve a TCP port on `ip` for the RTSP server, read the port number,
/// and immediately release it. The port must be released **before** starting
/// liveion so the RTSP server can bind to it. Unlike the WHIP UDP path,
/// RTSP needs no pre-allocated address in a data file.
#[cfg(feature = "rtsp")]
pub fn reserve_and_release_tcp_port(ip: IpAddr) -> u16 {
    let listener =
        std::net::TcpListener::bind(SocketAddr::new(ip, 0)).expect("Failed to reserve TCP port");
    listener.local_addr().unwrap().port()
}

/// Run one matrix case: publish `source` through liveion, then play it back
/// with `player` and validate the result against the source's media profile.
pub async fn run_whep_test_with_host<S, P>(source: S, player: P, bind_ip: IpAddr, whep_host: &str)
where
    S: Source,
    P: Player,
{
    let profile = source.profile();
    let (_api_addr, port, source_handle, whip_ct, whip_handle) =
        start_published_stream(&source, bind_ip).await;

    // Give the source a moment to produce keyframes before subscribing.
    source.wait_for_ready().await;

    // Run the WHEP player and verify playback.
    let whep_url = format!("http://{whep_host}:{port}{}", api::path::whep("-"));
    let playback = player
        .play(&whep_url, &profile)
        .await
        .expect("WHEP player failed");

    tracing::info!(
        source = source.name(),
        player = player.name(),
        ?playback,
        "WHEP playback result"
    );

    assert_playback_ok(player.name(), &profile, &playback);

    source_handle.stop().await;
    whip_ct.cancel();
    let result_whip = whip_handle.await.unwrap();
    assert!(result_whip.is_ok());
}

fn assert_playback_ok(player_name: &str, profile: &MediaProfile, playback: &PlayResult) {
    /// Players report codec names in different conventions: ffprobe uses
    /// lowercase (`h264`, `hevc`), the rsmpeg probe uses RTP names (`H264`,
    /// `H265`). Compare case-insensitively and treat h265/hevc as aliases.
    fn codec_matches(reported: &str, expected: &str) -> bool {
        let reported = reported.to_lowercase();
        let expected = expected.to_lowercase();
        reported == expected
            || (reported == "h265" && expected == "hevc")
            || (reported == "hevc" && expected == "h265")
    }

    assert!(
        playback.success,
        "{player_name} playback did not succeed: {:?}",
        playback.error
    );
    assert!(playback.connected, "{player_name} did not connect");
    assert!(
        playback.duration_ms > 0,
        "{player_name} reported zero duration"
    );

    match profile.video {
        Some(spec) => {
            assert!(
                playback.video_tracks >= 1,
                "{player_name} reported no video track for {profile}"
            );
            // All players report real dimensions (ffprobe, decoder or
            // browser-rendered frames).
            assert_eq!(
                playback.video_width, spec.width,
                "{player_name} video width mismatch for {profile}"
            );
            assert_eq!(
                playback.video_height, spec.height,
                "{player_name} video height mismatch for {profile}"
            );
            if !playback.codecs.is_empty() {
                assert!(
                    playback
                        .codecs
                        .iter()
                        .any(|c| codec_matches(c, spec.codec.ffprobe_name())),
                    "{player_name} expected video codec {} for {profile}, got {:?}",
                    spec.codec.ffprobe_name(),
                    playback.codecs
                );
            }
        }
        None => assert_eq!(
            playback.video_tracks, 0,
            "{player_name} reported an unexpected video track for {profile}"
        ),
    }

    match profile.audio {
        Some(audio) => {
            assert!(
                playback.audio_tracks >= 1,
                "{player_name} reported no audio track for {profile}"
            );
            if playback.audio_channels > 0 {
                assert_eq!(
                    playback.audio_channels,
                    audio.channels() as u32,
                    "{player_name} audio channel mismatch for {profile}"
                );
            }
            if !playback.codecs.is_empty() {
                assert!(
                    playback
                        .codecs
                        .iter()
                        .any(|c| codec_matches(c, audio.ffprobe_name())),
                    "{player_name} expected audio codec {} for {profile}, got {:?}",
                    audio.ffprobe_name(),
                    playback.codecs
                );
            }
        }
        None => assert_eq!(
            playback.audio_tracks, 0,
            "{player_name} reported an unexpected audio track for {profile}"
        ),
    }
}

/// Start liveion, create a stream, publish a source via WHIP (or RTSP), and
/// wait for the publish session to reach Connected.
///
/// Returns `(api_addr, http_port, source_handle, whip_cancellation_token, whip_join_handle)`.
pub async fn start_published_stream<S>(
    source: &S,
    bind_ip: IpAddr,
) -> (
    SocketAddr,
    u16,
    Box<dyn SourceHandle>,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
)
where
    S: Source,
{
    init_liveion_test_environment();

    let mut cfg = liveion::config::Config::default();
    cfg.http.cors = true;

    // RTSP sources need the RTSP listen port configured before liveion starts.
    // Reserve-and-release: the port is freed before liveion binds so the
    // RTSP server can claim it. This is a TOCTOU race but acceptable here.
    #[cfg(feature = "rtsp")]
    let rtsp_port: Option<u16> = if source.is_rtsp() {
        let port = reserve_and_release_tcp_port(bind_ip);
        cfg.rtsp.listen = SocketAddr::new(bind_ip, port).to_string();
        Some(port)
    } else {
        None
    };

    let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let api_addr = SocketAddr::new(bind_ip, port);

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::Client::new()
        .post(format!("http://{api_addr}{}", api::path::streams("-")))
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    // --- RTSP path: ffmpeg pushes directly to liveion's RTSP server ---
    #[cfg(feature = "rtsp")]
    if let Some(rtsp_port) = rtsp_port {
        let rtsp_host = match bind_ip {
            IpAddr::V6(_) => format!("[{bind_ip}]"),
            _ => bind_ip.to_string(),
        };
        let rtsp_url = format!("rtsp://{rtsp_host}:{rtsp_port}/-");

        // liveion's RTSP server binds inside a spawned task — wait until the
        // port is accepting connections before starting ffmpeg.
        let rtsp_addr = SocketAddr::new(bind_ip, rtsp_port);
        for i in 0..50 {
            match tokio::net::TcpStream::connect(rtsp_addr).await {
                Ok(_) => break,
                Err(_) if i == 49 => {
                    panic!("RTSP server did not start on {rtsp_addr} after 5 s");
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }

        let source_handle = source
            .start_rtsp(&rtsp_url)
            .expect("Failed to start RTSP FFmpeg source");

        wait_for_publish_connected(&api_addr, None).await;

        // No WHIP handle — return a no-op join handle so callers can
        // keep the same shape.
        let ct = CancellationToken::new();
        let handle_whip = tokio::spawn(async move { Ok(()) });

        return (api_addr, port, source_handle, ct, handle_whip);
    }

    let whip_url = format!("http://{api_addr}{}", api::path::whip("-"));

    if source.publishes_directly() {
        return start_direct_published_stream(source, api_addr, port, whip_url).await;
    }

    let profile = source.profile();
    let whip_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let video_addr = profile
        .video
        .map(|_| SocketAddr::new(whip_ip, alloc_udp_ports(whip_ip, 2)));
    let audio_addr = profile
        .audio
        .map(|_| SocketAddr::new(whip_ip, alloc_udp_ports(whip_ip, 2)));

    // Write the SDP file that liveion will use to receive the source stream.
    let sdp = source.sdp_with_audio(video_addr, audio_addr);
    let _whip_sdp = tempfile::NamedTempFile::new().unwrap();
    let sdp_path = _whip_sdp.path().to_str().unwrap().to_string();
    {
        let mut file = std::fs::File::create(&sdp_path).unwrap();
        file.write_all(sdp.as_bytes()).unwrap();
    }

    let ct = CancellationToken::new();
    let whip_ct = ct.clone();
    let mut handle_whip = tokio::spawn(async move {
        // Keep the temp SDP file alive for the lifetime of the WHIP task so the
        // runner cannot read a deleted path.
        let _whip_sdp = _whip_sdp;
        livetwo::whip::into(whip_ct, sdp_path, whip_url, None, None).await
    });

    wait_for_publish_connected(&api_addr, Some(&mut handle_whip)).await;

    // Start the media source only after the WHIP/RTP listener is bound so that
    // sources which open a connected UDP socket don't hit ICMP errors before
    // the receiver is ready.
    let source_handle = source
        .start_with_audio(video_addr, audio_addr)
        .expect("Failed to start media source");

    (api_addr, port, source_handle, ct, handle_whip)
}

async fn start_direct_published_stream<S>(
    source: &S,
    api_addr: SocketAddr,
    port: u16,
    whip_url: String,
) -> (
    SocketAddr,
    u16,
    Box<dyn SourceHandle>,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
)
where
    S: Source,
{
    let source_handle = source
        .start_direct(&whip_url)
        .expect("Failed to start direct WHIP source");

    wait_for_publish_connected(&api_addr, None).await;

    // The publisher is already running inside the source handle; return a
    // no-op WHIP handle so callers can keep the same shape.
    let ct = CancellationToken::new();
    let handle_whip = tokio::spawn(async move { Ok(()) });

    (api_addr, port, source_handle, ct, handle_whip)
}

async fn wait_for_publish_connected(
    api_addr: &SocketAddr,
    mut handle_whip: Option<&mut tokio::task::JoinHandle<anyhow::Result<()>>>,
) {
    let mut publish_connected = false;
    for _ in 0..300 {
        let res = reqwest::get(format!("http://{api_addr}{}", api::path::streams("")))
            .await
            .unwrap();
        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();
        if let Some(r) = body.into_iter().find(|i| i.id == "-")
            && !r.publish.sessions.is_empty()
        {
            let s = r.publish.sessions[0].clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                publish_connected = true;
                break;
            }
        }

        if let Some(handle) = handle_whip.as_mut()
            && handle.is_finished()
        {
            let result_whip = handle.await.unwrap();
            panic!("WHIP task exited before publish connected: {result_whip:?}");
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(publish_connected, "Publish session did not reach Connected");
}
