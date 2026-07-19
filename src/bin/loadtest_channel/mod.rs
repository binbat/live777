//! DataChannel <-> UDP forwarding load tests (throughput, latency and
//! bidirectional traffic), shared by the `loadtest` and
//! `datachannel_loadtest` binaries.
//!
//! The topology is self-contained: an in-process `liveion` with a channel
//! bridge plus a `livetwo::whep::from` subscriber with its own channel
//! endpoint, so no external server is required.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Which measurement to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    All,
    Throughput,
    Latency,
    Bidirectional,
}

impl FromStr for ChannelMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "throughput" => Ok(Self::Throughput),
            "latency" => Ok(Self::Latency),
            "bidirectional" => Ok(Self::Bidirectional),
            other => Err(format!("unknown mode: {other}")),
        }
    }
}

impl std::fmt::Display for ChannelMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::All => "all",
            Self::Throughput => "throughput",
            Self::Latency => "latency",
            Self::Bidirectional => "bidirectional",
        };
        f.write_str(s)
    }
}

/// Parameters for the DataChannel load tests.
#[derive(Debug, Clone)]
pub struct ChannelLoadtestParams {
    pub mode: ChannelMode,
    pub packet_size: usize,
    pub packet_count: usize,
    pub warmup_packets: usize,
    pub latency_rounds: usize,
    pub window: Option<usize>,
    pub bind_host: String,
    pub target_host: String,
}

impl Default for ChannelLoadtestParams {
    fn default() -> Self {
        Self {
            mode: ChannelMode::All,
            packet_size: 1400,
            packet_count: 10000,
            warmup_packets: 3,
            latency_rounds: 200,
            window: None,
            bind_host: "127.0.0.1".into(),
            target_host: "127.0.0.1".into(),
        }
    }
}

pub fn print_environment_hint(params: &ChannelLoadtestParams) {
    println!("══════════════════════════════════════════════");
    println!("  DataChannel UDP Load Test");
    println!("  Mode: {}", params.mode);
    println!("  Packet size: {} bytes", params.packet_size);
    println!("  Packet count: {}", params.packet_count);
    println!("  Warmup packets: {}", params.warmup_packets);
    println!("  Latency rounds: {}", params.latency_rounds);
    println!("  Bind host: {}", params.bind_host);
    println!("  Target host: {}", params.target_host);
    println!("  Build: release recommended");
    println!("  Feature: source required");
    println!("══════════════════════════════════════════════");
}

/// Run the selected measurement(s).
pub async fn run(params: &ChannelLoadtestParams) -> Result<(), Box<dyn std::error::Error>> {
    match params.mode {
        ChannelMode::All => {
            run_latency(params).await?;
            run_throughput(params).await?;
            run_bidirectional(params).await?;
        }
        ChannelMode::Throughput => run_throughput(params).await?,
        ChannelMode::Latency => run_latency(params).await?,
        ChannelMode::Bidirectional => run_bidirectional(params).await?,
    }
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────────

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn wait_for_session_connected(addr: &SocketAddr, stream_id: &str) -> bool {
    for _ in 0..200 {
        let body = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json::<Vec<api::response::Stream>>()
            .await
            .unwrap_or_default();
        if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && !stream.subscribe.sessions.is_empty()
            && stream.subscribe.sessions[0].state
                == api::response::RTCPeerConnectionState::Connected
        {
            return true;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    false
}

#[derive(Debug, Clone, Copy)]
struct ThroughputResult {
    mbps: f64,
    bytes: usize,
    sent: usize,
    received: usize,
}

impl ThroughputResult {
    fn lost(&self) -> usize {
        self.sent.saturating_sub(self.received)
    }

    fn loss_rate(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            self.lost() as f64 * 100.0 / self.sent as f64
        }
    }
}

async fn recv_udp_with_timeout(
    socket: &UdpSocket,
    buf: &mut [u8],
    timeout: Duration,
) -> Option<(usize, SocketAddr)> {
    match tokio::time::timeout(timeout, socket.recv_from(buf)).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => panic!("UDP recv_from failed: {e}"),
        Err(_) => None,
    }
}

async fn setup_topology(
    params: &ChannelLoadtestParams,
    stream_id: &str,
    whepfrom_ch_listen: u16,
    whepfrom_ch_target: u16,
    liveion_ch_listen: u16,
    liveion_ch_target: u16,
) -> (
    SocketAddr,
    CancellationToken,
    UdpSocket,
    UdpSocket,
    UdpSocket,
) {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let mut cfg = liveion::config::Config::default();
    cfg.stream.streams.insert(
        stream_id.to_string(),
        liveion::config::StreamEntry {
            sources: vec![],
            strategy: None,
            channel: Some(liveion::config::ChannelConfig {
                listen: format!("0.0.0.0:{liveion_ch_listen}").parse().unwrap(),
                target: format!("{}:{liveion_ch_target}", params.target_host)
                    .parse()
                    .unwrap(),
            }),
        },
    );
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await
        .unwrap();

    let ct = CancellationToken::new();
    let whep_channel_url = format!(
        "udp://0.0.0.0:{whepfrom_ch_listen}?host={}&port={whepfrom_ch_target}",
        params.target_host
    );
    tokio::spawn(livetwo::whep::from(
        ct.clone(),
        format!("rtp://{ip}"),
        format!("http://{addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        Some(whep_channel_url),
    ));

    assert!(
        wait_for_session_connected(&addr, stream_id).await,
        "WHEP subscriber failed to connect"
    );

    let whepfrom_target = UdpSocket::bind(format!("{}:{whepfrom_ch_target}", params.bind_host))
        .await
        .unwrap();
    let liveion_target = UdpSocket::bind(format!("{}:{liveion_ch_target}", params.bind_host))
        .await
        .unwrap();
    let udp_sender = UdpSocket::bind(format!("{}:0", params.bind_host))
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    (addr, ct, whepfrom_target, liveion_target, udp_sender)
}

/// Run a pipelined throughput test with sliding-window flow control.
/// `window` is the max number of in-flight packets — prevents UDP buffer overflow.
async fn measure_throughput(
    sender: Arc<UdpSocket>,
    target: UdpSocket,
    dest_addr: &str,
    payload: Arc<Vec<u8>>,
    pkt_count: usize,
    warmup: usize,
    window: usize,
) -> ThroughputResult {
    // Warmup is also a readiness probe for DataChannel open/detach.
    // Important: bound the *whole* warmup by elapsed time. The first fixed
    // version bounded each recv, but `warmup * 10 * 500ms` could still exceed
    // 60s and look like a hang when the path is broken.
    let mut warm_buf = vec![0u8; payload.len().max(65536)];
    let mut warmed = 0usize;
    let warmup_deadline = Instant::now() + Duration::from_secs(5);
    while warmed < warmup && Instant::now() < warmup_deadline {
        sender.send_to(&payload, dest_addr).await.unwrap();
        if recv_udp_with_timeout(&target, &mut warm_buf, Duration::from_millis(100))
            .await
            .is_some()
        {
            warmed += 1;
        }
    }
    assert!(
        warmup == 0 || warmed > 0,
        "no packets received during 5s warmup; DataChannel/UDP path is not ready: {dest_addr}"
    );

    let sem = Arc::new(Semaphore::new(window.max(1)));
    let sem_recv = sem.clone();
    let pkt_size = payload.len();

    let recv_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; pkt_size.max(65536)];
        let mut received_pkts = 0usize;
        let mut received_bytes = 0usize;
        while received_pkts < pkt_count {
            match recv_udp_with_timeout(&target, &mut buf, Duration::from_secs(2)).await {
                Some((n, _)) => {
                    received_pkts += 1;
                    received_bytes += n;
                    sem_recv.add_permits(1);
                }
                None => break,
            }
        }
        (received_pkts, received_bytes)
    });

    let start = Instant::now();
    let mut sent = 0usize;
    for _ in 0..pkt_count {
        match tokio::time::timeout(Duration::from_secs(2), sem.acquire()).await {
            Ok(Ok(permit)) => permit.forget(),
            Ok(Err(_)) | Err(_) => break,
        }
        sender.send_to(&payload, dest_addr).await.unwrap();
        sent += 1;
    }

    let (received, bytes) = recv_handle.await.unwrap();
    let elapsed = start.elapsed().max(Duration::from_nanos(1));
    let mbps = (bytes as f64 * 8.0) / elapsed.as_secs_f64() / 1_000_000.0;
    ThroughputResult {
        mbps,
        bytes,
        sent,
        received,
    }
}

// ── Throughput Load Test ─────────────────────────────────────────────────────────

async fn run_throughput(params: &ChannelLoadtestParams) -> Result<(), Box<dyn std::error::Error>> {
    let stream_id = "loadtest-dc-tp";
    let pkt_size = params.packet_size;
    let pkt_count = params.packet_count;
    let warmup = params.warmup_packets;
    let window = params
        .window
        .unwrap_or_else(|| (65536 / pkt_size.max(1)).clamp(1, 128));

    println!("\n══════════════════════════════════════════════");
    println!("  DataChannel UDP Throughput Load Test");
    println!(
        "  Packet size: {} bytes, Count: {}, Window: {}",
        pkt_size, pkt_count, window
    );
    println!("══════════════════════════════════════════════\n");

    let (_addr, ct, whepfrom_target, liveion_target, udp_sender) =
        setup_topology(params, stream_id, 8700, 8701, 8702, 8703).await;

    let sender = Arc::new(udp_sender);
    let payload = Arc::new(vec![0xABu8; pkt_size]);

    // ── Direction A: UDP → liveion → DC → whepfrom ──────────────────────────
    println!("[Dir A] UDP → liveion:8702 → DataChannel → whepfrom:8701");
    let res_a = measure_throughput(
        sender.clone(),
        whepfrom_target,
        &format!("{}:8702", params.target_host),
        payload.clone(),
        pkt_count,
        warmup,
        window,
    )
    .await;
    println!(
        "  {} bytes, {}/{} pkts, loss {:.2}% → {:.2} Mbps",
        res_a.bytes,
        res_a.received,
        res_a.sent,
        res_a.loss_rate(),
        res_a.mbps
    );

    // ── Direction B: UDP → whepfrom → DC → liveion ──────────────────────────
    println!("\n[Dir B] UDP → whepfrom:8700 → DataChannel → liveion:8703");
    let res_b = measure_throughput(
        sender.clone(),
        liveion_target,
        &format!("{}:8700", params.target_host),
        payload.clone(),
        pkt_count,
        warmup,
        window,
    )
    .await;
    println!(
        "  {} bytes, {}/{} pkts, loss {:.2}% → {:.2} Mbps",
        res_b.bytes,
        res_b.received,
        res_b.sent,
        res_b.loss_rate(),
        res_b.mbps
    );

    println!("\n──────────────────────────────────────────────");
    println!("  Throughput Summary:");
    println!(
        "    Dir A (liveion → whepfrom): {:.2} Mbps, loss {:.2}%",
        res_a.mbps,
        res_a.loss_rate()
    );
    println!(
        "    Dir B (whepfrom → liveion): {:.2} Mbps, loss {:.2}%",
        res_b.mbps,
        res_b.loss_rate()
    );
    println!("──────────────────────────────────────────────\n");

    ct.cancel();
    Ok(())
}

// ── Latency Load Test ────────────────────────────────────────────────────────────

async fn run_latency(params: &ChannelLoadtestParams) -> Result<(), Box<dyn std::error::Error>> {
    let stream_id = "loadtest-dc-lat";
    let pkt_size = params.packet_size;
    let rounds = params.latency_rounds;

    println!("\n══════════════════════════════════════════════");
    println!("  DataChannel UDP Latency Load Test");
    println!("  Packet size: {} bytes, Rounds: {}", pkt_size, rounds);
    println!("══════════════════════════════════════════════\n");

    let (_addr, ct, whepfrom_target, liveion_target, sender) =
        setup_topology(params, stream_id, 8704, 8705, 8706, 8707).await;

    let sender = Arc::new(sender);
    let payload = Arc::new(vec![0x00u8; pkt_size]);
    let mut recv_buf = vec![0u8; pkt_size + 64];
    let mut rtts_a = Vec::with_capacity(rounds);
    let mut rtts_b = Vec::with_capacity(rounds);

    // ── Direction A ──────────────────────────────────────────────────────────
    println!("[Dir A] One-way: UDP → liveion:8706 → DC → whepfrom:8705");

    let s = sender.clone();
    let p = payload.clone();
    for _ in 0..rounds {
        let ts_send = Instant::now();
        s.send_to(&p, format!("{}:8706", params.target_host))
            .await
            .unwrap();
        if recv_udp_with_timeout(&whepfrom_target, &mut recv_buf, Duration::from_secs(2))
            .await
            .is_some()
        {
            rtts_a.push(ts_send.elapsed());
        } else {
            panic!("latency Dir A timeout waiting for UDP packet");
        }
    }

    // ── Direction B ──────────────────────────────────────────────────────────
    println!("[Dir B] One-way: UDP → whepfrom:8704 → DC → liveion:8707");

    let s = sender.clone();
    let p = payload.clone();
    for _ in 0..rounds {
        let ts_send = Instant::now();
        s.send_to(&p, format!("{}:8704", params.target_host))
            .await
            .unwrap();
        if recv_udp_with_timeout(&liveion_target, &mut recv_buf, Duration::from_secs(2))
            .await
            .is_some()
        {
            rtts_b.push(ts_send.elapsed());
        } else {
            panic!("latency Dir B timeout waiting for UDP packet");
        }
    }

    // ── Stats ───────────────────────────────────────────────────────────────
    fn latency_stats(rtts: &[std::time::Duration]) -> (f64, f64, f64, f64, f64) {
        let mut sorted: Vec<f64> = rtts.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        if sorted.is_empty() {
            return (0.0, 0.0, 0.0, 0.0, 0.0);
        }
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = sorted.first().copied().unwrap_or(0.0);
        let max = sorted.last().copied().unwrap_or(0.0);
        let avg = sorted.iter().sum::<f64>() / sorted.len() as f64;
        let p50 = sorted[sorted.len() / 2];
        let p95_idx = ((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1);
        let p95 = sorted[p95_idx];
        (avg, min, max, p50, p95)
    }

    let (avg_a, min_a, max_a, p50_a, p95_a) = latency_stats(&rtts_a);
    let (avg_b, min_b, max_b, p50_b, p95_b) = latency_stats(&rtts_b);

    println!("\n──────────────────────────────────────────────────");
    println!("  Latency Results (one-way, ms):");
    println!("                     avg     min     p50     p95     max");
    println!(
        "  Dir A (→whepfrom): {:.2}   {:.2}   {:.2}   {:.2}   {:.2}",
        avg_a, min_a, p50_a, p95_a, max_a
    );
    println!(
        "  Dir B (→liveion):  {:.2}   {:.2}   {:.2}   {:.2}   {:.2}",
        avg_b, min_b, p50_b, p95_b, max_b
    );
    println!("──────────────────────────────────────────────────\n");

    ct.cancel();
    Ok(())
}

// ── Bidirectional Load Test ──────────────────────────────────────────────────────

async fn run_bidirectional(
    params: &ChannelLoadtestParams,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream_id = "loadtest-dc-bidi";
    let pkt_size = params.packet_size;
    let pkt_count = params.packet_count;
    let warmup = params.warmup_packets;
    let window = params
        .window
        .unwrap_or_else(|| (65536 / pkt_size.max(1)).clamp(1, 128));

    println!("\n══════════════════════════════════════════════");
    println!("  DataChannel UDP Bidirectional Load Test");
    println!(
        "  Packet size: {} bytes, Count: {} (each dir), Window: {}",
        pkt_size, pkt_count, window
    );
    println!("══════════════════════════════════════════════\n");

    let (_addr, ct, whepfrom_target, liveion_target, sender_a) =
        setup_topology(params, stream_id, 8708, 8709, 8710, 8711).await;
    let sender_b = UdpSocket::bind(format!("{}:0", params.bind_host))
        .await
        .unwrap();

    let sender_a = Arc::new(sender_a);
    let sender_b = Arc::new(sender_b);
    let payload = Arc::new(vec![0xABu8; pkt_size]);

    let start = Instant::now();
    let dest_a = format!("{}:8710", params.target_host);
    let dest_b = format!("{}:8708", params.target_host);
    let (res_a, res_b) = tokio::join!(
        measure_throughput(
            sender_a.clone(),
            whepfrom_target,
            &dest_a,
            payload.clone(),
            pkt_count,
            warmup,
            window,
        ),
        measure_throughput(
            sender_b.clone(),
            liveion_target,
            &dest_b,
            payload.clone(),
            pkt_count,
            warmup,
            window,
        )
    );
    let elapsed = start.elapsed().max(Duration::from_nanos(1));

    let total_bytes = res_a.bytes + res_b.bytes;
    let mbps = (total_bytes as f64 * 8.0) / elapsed.as_secs_f64() / 1_000_000.0;

    println!(
        "  Dir A: {} bytes, {}/{} pkts, loss {:.2}% → {:.2} Mbps",
        res_a.bytes,
        res_a.received,
        res_a.sent,
        res_a.loss_rate(),
        res_a.mbps
    );
    println!(
        "  Dir B: {} bytes, {}/{} pkts, loss {:.2}% → {:.2} Mbps",
        res_b.bytes,
        res_b.received,
        res_b.sent,
        res_b.loss_rate(),
        res_b.mbps
    );
    println!(
        "  Total: {} bytes in {:?} → {:.2} Mbps (bidirectional aggregate)",
        total_bytes, elapsed, mbps
    );
    println!("──────────────────────────────────────────────\n");

    ct.cancel();
    Ok(())
}
