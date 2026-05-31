/// Benchmark: DataChannel <-> UDP forwarding throughput and latency.
///
/// Usage:
///   cargo test --features source --release --test channel_bench -- --nocapture
///
/// Custom params (env vars):
///   BENCH_PACKET_SIZE=1400  BENCH_PACKET_COUNT=5000  cargo test ...
///   BENCH_LATENCY_ROUNDS=500                           cargo test ...

#[cfg(feature = "source")]
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(feature = "source")]
use std::sync::Arc;
#[cfg(feature = "source")]
use std::time::{Duration, Instant};

#[cfg(feature = "source")]
use tokio::net::{TcpListener, UdpSocket};
#[cfg(feature = "source")]
use tokio::sync::Semaphore;
#[cfg(feature = "source")]
use tokio_util::sync::CancellationToken;

#[cfg(feature = "source")]
mod common;
#[cfg(feature = "source")]
use common::shutdown_signal;

// ── Helpers ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "source")]
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

#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy)]
struct ThroughputResult {
    mbps: f64,
    bytes: usize,
    sent: usize,
    received: usize,
}

#[cfg(feature = "source")]
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

#[cfg(feature = "source")]
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

#[cfg(feature = "source")]
fn bench_params() -> (usize, usize, usize) {
    let pkt_size: usize = std::env::var("BENCH_PACKET_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        // Keep the default below the current UDP forward buffer size (1500).
        .unwrap_or(1400);
    let pkt_count: usize = std::env::var("BENCH_PACKET_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000);
    let warmup: usize = std::env::var("BENCH_WARMUP_PKTS")
        .ok()
        .and_then(|s| s.parse().ok())
        // This is only a readiness probe. A large warmup can make a broken path
        // look like a hang because every lost probe waits for recv timeout.
        .unwrap_or(3);
    (pkt_size, pkt_count, warmup)
}

#[cfg(feature = "source")]
async fn setup_topology(
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
    cfg.channel.streams.insert(
        stream_id.to_string(),
        liveion::config::ChannelStream {
            url: format!(
                "udp://0.0.0.0:{liveion_ch_listen}?host=127.0.0.1&port={liveion_ch_target}"
            ),
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
    let whep_channel_url =
        format!("udp://0.0.0.0:{whepfrom_ch_listen}?host=127.0.0.1&port={whepfrom_ch_target}");
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

    let whepfrom_target = UdpSocket::bind(format!("127.0.0.1:{whepfrom_ch_target}"))
        .await
        .unwrap();
    let liveion_target = UdpSocket::bind(format!("127.0.0.1:{liveion_ch_target}"))
        .await
        .unwrap();
    let udp_sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    (addr, ct, whepfrom_target, liveion_target, udp_sender)
}

/// Run a pipelined throughput test with sliding-window flow control.
/// `window` is the max number of in-flight packets — prevents UDP buffer overflow.
#[cfg(feature = "source")]
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

// ── Throughput Benchmark ────────────────────────────────────────────────────────

#[cfg(feature = "source")]
#[tokio::test]
async fn bench_datachannel_throughput() {
    let stream_id = "bench-dc-tp";
    let (pkt_size, pkt_count, warmup) = bench_params();
    // Window sized to keep ~64KB in-flight (safe for default UDP buffers)
    let window = (65536 / pkt_size.max(1)).min(128).max(1);

    println!("\n══════════════════════════════════════════════");
    println!("  DataChannel UDP Throughput Benchmark");
    println!(
        "  Packet size: {} bytes, Count: {}, Window: {}",
        pkt_size, pkt_count, window
    );
    println!("══════════════════════════════════════════════\n");

    let (_addr, ct, whepfrom_target, liveion_target, udp_sender) =
        setup_topology(stream_id, 8700, 8701, 8702, 8703).await;

    let sender = Arc::new(udp_sender);
    let payload = Arc::new(vec![0xABu8; pkt_size]);

    // ── Direction A: UDP → liveion → DC → whepfrom ──────────────────────────
    println!("[Dir A] UDP → liveion:8702 → DataChannel → whepfrom:8701");
    let res_a = measure_throughput(
        sender.clone(),
        whepfrom_target,
        "127.0.0.1:8702",
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
        "127.0.0.1:8700",
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
}

// ── Latency Benchmark ───────────────────────────────────────────────────────────

#[cfg(feature = "source")]
#[tokio::test]
async fn bench_datachannel_latency() {
    let stream_id = "bench-dc-lat";
    let pkt_size: usize = std::env::var("BENCH_PACKET_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1400);
    let rounds: usize = std::env::var("BENCH_LATENCY_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    println!("\n══════════════════════════════════════════════");
    println!("  DataChannel UDP Latency Benchmark");
    println!("  Packet size: {} bytes, Rounds: {}", pkt_size, rounds);
    println!("══════════════════════════════════════════════\n");

    let (_addr, ct, whepfrom_target, liveion_target, sender) =
        setup_topology(stream_id, 8704, 8705, 8706, 8707).await;

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
        s.send_to(&p, "127.0.0.1:8706").await.unwrap();
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
        s.send_to(&p, "127.0.0.1:8704").await.unwrap();
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
}

// ── Bidirectional Throughput ────────────────────────────────────────────────────

#[cfg(feature = "source")]
#[tokio::test]
async fn bench_datachannel_bidirectional() {
    let stream_id = "bench-dc-bidi";
    let (pkt_size, pkt_count, warmup) = bench_params();
    let window = (65536 / pkt_size.max(1)).min(128).max(1);

    println!("\n══════════════════════════════════════════════");
    println!("  DataChannel UDP Bidirectional Benchmark");
    println!(
        "  Packet size: {} bytes, Count: {} (each dir), Window: {}",
        pkt_size, pkt_count, window
    );
    println!("══════════════════════════════════════════════\n");

    let (_addr, ct, whepfrom_target, liveion_target, sender_a) =
        setup_topology(stream_id, 8708, 8709, 8710, 8711).await;
    let sender_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let sender_a = Arc::new(sender_a);
    let sender_b = Arc::new(sender_b);
    let payload = Arc::new(vec![0xABu8; pkt_size]);

    let start = Instant::now();
    let (res_a, res_b) = tokio::join!(
        measure_throughput(
            sender_a.clone(),
            whepfrom_target,
            "127.0.0.1:8710",
            payload.clone(),
            pkt_count,
            warmup,
            window,
        ),
        measure_throughput(
            sender_b.clone(),
            liveion_target,
            "127.0.0.1:8708",
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
}
