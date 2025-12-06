use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info};
use webrtc::{
    peer_connection::RTCPeerConnection,
    stats::{InboundRTPStats, OutboundRTPStats, StatsReportType},
};
pub struct RtcpStats {
    pub fir_count: AtomicU64,
    pub pli_count: AtomicU64,
    pub nack_count: AtomicU64,

    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub packets_lost: AtomicU64,

    last_send_bitrate_update: Arc<Mutex<Instant>>,
    last_receive_bitrate_update: Arc<Mutex<Instant>>,
    last_send_packet_rate_update: Arc<Mutex<Instant>>,
    last_receive_packet_rate_update: Arc<Mutex<Instant>>,

    last_bytes_sent: Arc<Mutex<u64>>,
    last_bytes_received: Arc<Mutex<u64>>,
    last_packets_sent: Arc<Mutex<u64>>,
    last_packets_received: Arc<Mutex<u64>>,
}

impl RtcpStats {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            fir_count: AtomicU64::new(0),
            pli_count: AtomicU64::new(0),
            nack_count: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            packets_lost: AtomicU64::new(0),
            last_send_bitrate_update: Arc::new(Mutex::new(now)),
            last_receive_bitrate_update: Arc::new(Mutex::new(now)),
            last_send_packet_rate_update: Arc::new(Mutex::new(now)),
            last_receive_packet_rate_update: Arc::new(Mutex::new(now)),
            last_bytes_sent: Arc::new(Mutex::new(0)),
            last_bytes_received: Arc::new(Mutex::new(0)),
            last_packets_sent: Arc::new(Mutex::new(0)),
            last_packets_received: Arc::new(Mutex::new(0)),
        }
    }

    pub fn increment_fir(&self) {
        self.fir_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_pli(&self) {
        self.pli_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_nack(&self) {
        self.nack_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_fir_count(&self) -> u64 {
        self.fir_count.load(Ordering::Relaxed)
    }

    pub fn get_pli_count(&self) -> u64 {
        self.pli_count.load(Ordering::Relaxed)
    }

    pub fn get_nack_count(&self) -> u64 {
        self.nack_count.load(Ordering::Relaxed)
    }

    pub fn add_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_bytes_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_packets_sent(&self, count: u64) {
        self.packets_sent.fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_packets_received(&self, count: u64) {
        self.packets_received.fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_packets_lost(&self, count: u64) {
        self.packets_lost.fetch_add(count, Ordering::Relaxed);
    }

    pub fn set_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.store(bytes, Ordering::Relaxed);
    }

    pub fn set_bytes_received(&self, bytes: u64) {
        self.bytes_received.store(bytes, Ordering::Relaxed);
    }

    pub fn set_packets_sent(&self, count: u64) {
        self.packets_sent.store(count, Ordering::Relaxed);
    }

    pub fn set_packets_received(&self, count: u64) {
        self.packets_received.store(count, Ordering::Relaxed);
    }

    pub fn set_packets_lost(&self, count: u64) {
        self.packets_lost.store(count, Ordering::Relaxed);
    }

    pub fn update_from_inbound_stats(&self, stats: &InboundRTPStats) {
        self.set_bytes_received(stats.bytes_received);
        self.set_packets_received(stats.packets_received);

        if let Some(fir) = stats.fir_count {
            self.fir_count.store(fir, Ordering::Relaxed);
        }
        if let Some(pli) = stats.pli_count {
            self.pli_count.store(pli, Ordering::Relaxed);
        }
        self.nack_count.store(stats.nack_count, Ordering::Relaxed);
    }

    pub fn update_from_outbound_stats(&self, stats: &OutboundRTPStats) {
        self.set_bytes_sent(stats.bytes_sent);
        self.set_packets_sent(stats.packets_sent);

        if let Some(fir) = stats.fir_count {
            self.fir_count.store(fir, Ordering::Relaxed);
        }
        if let Some(pli) = stats.pli_count {
            self.pli_count.store(pli, Ordering::Relaxed);
        }
        self.nack_count.store(stats.nack_count, Ordering::Relaxed);
    }

    pub async fn get_send_bitrate(&self) -> f64 {
        let mut last_update = self.last_send_bitrate_update.lock().await;
        let mut last_bytes = self.last_bytes_sent.lock().await;

        let now = Instant::now();
        let elapsed = now.duration_since(*last_update).as_secs_f64();

        if elapsed < 0.1 {
            return 0.0;
        }

        let current_bytes = self.bytes_sent.load(Ordering::Relaxed);
        let bytes_diff = current_bytes.saturating_sub(*last_bytes);

        let bitrate = (bytes_diff as f64 * 8.0) / elapsed;

        *last_update = now;
        *last_bytes = current_bytes;

        bitrate
    }

    pub async fn get_receive_bitrate(&self) -> f64 {
        let mut last_update = self.last_receive_bitrate_update.lock().await;
        let mut last_bytes = self.last_bytes_received.lock().await;

        let now = Instant::now();
        let elapsed = now.duration_since(*last_update).as_secs_f64();

        if elapsed < 0.1 {
            return 0.0;
        }

        let current_bytes = self.bytes_received.load(Ordering::Relaxed);
        let bytes_diff = current_bytes.saturating_sub(*last_bytes);

        let bitrate = (bytes_diff as f64 * 8.0) / elapsed;

        *last_update = now;
        *last_bytes = current_bytes;

        bitrate
    }

    pub async fn get_send_packet_rate(&self) -> f64 {
        let mut last_update = self.last_send_packet_rate_update.lock().await;
        let mut last_packets = self.last_packets_sent.lock().await;

        let now = Instant::now();
        let elapsed = now.duration_since(*last_update).as_secs_f64();

        if elapsed < 0.1 {
            return 0.0;
        }

        let current_packets = self.packets_sent.load(Ordering::Relaxed);
        let packets_diff = current_packets.saturating_sub(*last_packets);

        let packet_rate = packets_diff as f64 / elapsed;

        *last_update = now;
        *last_packets = current_packets;

        packet_rate
    }

    pub async fn get_receive_packet_rate(&self) -> f64 {
        let mut last_update = self.last_receive_packet_rate_update.lock().await;
        let mut last_packets = self.last_packets_received.lock().await;

        let now = Instant::now();
        let elapsed = now.duration_since(*last_update).as_secs_f64();

        if elapsed < 0.1 {
            return 0.0;
        }

        let current_packets = self.packets_received.load(Ordering::Relaxed);
        let packets_diff = current_packets.saturating_sub(*last_packets);

        let packet_rate = packets_diff as f64 / elapsed;

        *last_update = now;
        *last_packets = current_packets;

        packet_rate
    }

    pub fn get_packet_loss_rate(&self) -> f64 {
        let received = self.packets_received.load(Ordering::Relaxed);
        let lost = self.packets_lost.load(Ordering::Relaxed);

        let total = received + lost;
        if total == 0 {
            return 0.0;
        }

        (lost as f64 / total as f64) * 100.0
    }

    pub async fn get_summary(&self) -> StatsSummary {
        let send_bitrate = self.get_send_bitrate().await;
        let receive_bitrate = self.get_receive_bitrate().await;
        let send_pkt_rate = self.get_send_packet_rate().await;
        let receive_pkt_rate = self.get_receive_packet_rate().await;

        StatsSummary {
            fir_count: self.fir_count.load(Ordering::Relaxed),
            pli_count: self.pli_count.load(Ordering::Relaxed),
            nack_count: self.nack_count.load(Ordering::Relaxed),

            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            packets_received: self.packets_received.load(Ordering::Relaxed),
            packets_lost: self.packets_lost.load(Ordering::Relaxed),

            send_bitrate_mbps: send_bitrate / 1_000_000.0,
            receive_bitrate_mbps: receive_bitrate / 1_000_000.0,
            send_packet_rate: send_pkt_rate,
            receive_packet_rate: receive_pkt_rate,
            packet_loss_rate: self.get_packet_loss_rate(),
        }
    }

    pub fn reset(&self) {
        self.fir_count.store(0, Ordering::Relaxed);
        self.pli_count.store(0, Ordering::Relaxed);
        self.nack_count.store(0, Ordering::Relaxed);
        self.bytes_sent.store(0, Ordering::Relaxed);
        self.bytes_received.store(0, Ordering::Relaxed);
        self.packets_sent.store(0, Ordering::Relaxed);
        self.packets_received.store(0, Ordering::Relaxed);
        self.packets_lost.store(0, Ordering::Relaxed);
    }
}

impl Default for RtcpStats {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct StatsSummary {
    pub fir_count: u64,
    pub pli_count: u64,
    pub nack_count: u64,

    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,

    pub send_bitrate_mbps: f64,
    pub receive_bitrate_mbps: f64,
    pub send_packet_rate: f64,
    pub receive_packet_rate: f64,
    pub packet_loss_rate: f64,
}

impl StatsSummary {
    pub fn format(&self) -> String {
        format!(
            r#"
=== Livetwo Stats ===
RTCP:
  FIR:  {}
  PLI:  {}
  NACK: {}

Traffic:
  Sent:     {} bytes ({:.2} MB), {} packets
  Received: {} bytes ({:.2} MB), {} packets
  Lost:     {} packets ({:.2}%)

Rates:
  Send:    {:.2} Mbps, {:.0} pps
  Receive: {:.2} Mbps, {:.0} pps
===================="#,
            self.fir_count,
            self.pli_count,
            self.nack_count,
            self.bytes_sent,
            self.bytes_sent as f64 / 1_000_000.0,
            self.packets_sent,
            self.bytes_received,
            self.bytes_received as f64 / 1_000_000.0,
            self.packets_received,
            self.packets_lost,
            self.packet_loss_rate,
            self.send_bitrate_mbps,
            self.send_packet_rate,
            self.receive_bitrate_mbps,
            self.receive_packet_rate,
        )
    }
}

pub async fn start_stats_monitor(
    peer: Arc<RTCPeerConnection>,
    stats: Arc<RtcpStats>,
    shutdown: crate::utils::shutdown::ShutdownSignal,
) {
    tokio::spawn(async move {
        info!("WebRTC stats monitor started");
        let mut interval = tokio::time::interval(Duration::from_secs(2));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let stats_report = peer.get_stats().await;

                    for report in stats_report.reports.values() {
                        match report {
                            StatsReportType::InboundRTP(inbound) => {
                                stats.update_from_inbound_stats(inbound);
                                debug!(
                                    "InboundRTP - SSRC: {}, bytes: {}, packets: {}, FIR: {:?}, PLI: {:?}, NACK: {}",
                                    inbound.ssrc,
                                    inbound.bytes_received,
                                    inbound.packets_received,
                                    inbound.fir_count,
                                    inbound.pli_count,
                                    inbound.nack_count
                                );
                            }
                            StatsReportType::OutboundRTP(outbound) => {
                                stats.update_from_outbound_stats(outbound);
                                debug!(
                                    "OutboundRTP - SSRC: {}, bytes: {}, packets: {}, FIR: {:?}, PLI: {:?}, NACK: {}",
                                    outbound.ssrc,
                                    outbound.bytes_sent,
                                    outbound.packets_sent,
                                    outbound.fir_count,
                                    outbound.pli_count,
                                    outbound.nack_count
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ = shutdown.wait() => {
                    info!("WebRTC stats monitor shutting down");
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rtcp_stats() {
        let stats = RtcpStats::new();

        stats.increment_fir();
        stats.increment_pli();
        assert_eq!(stats.get_fir_count(), 1);
        assert_eq!(stats.get_pli_count(), 1);

        stats.add_bytes_sent(1000);
        stats.add_packets_sent(10);
        assert_eq!(stats.bytes_sent.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.packets_sent.load(Ordering::Relaxed), 10);

        stats.add_packets_received(90);
        stats.add_packets_lost(10);
        let loss_rate = stats.get_packet_loss_rate();
        assert!((loss_rate - 10.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_bitrate_calculation() {
        let stats = RtcpStats::new();

        stats.add_bytes_sent(1_000_000);
        tokio::time::sleep(Duration::from_millis(200)).await;

        let bitrate1 = stats.get_send_bitrate().await;
        assert!(bitrate1 > 0.0);

        let bitrate2 = stats.get_send_bitrate().await;
        assert_eq!(bitrate2, 0.0);

        tokio::time::sleep(Duration::from_millis(200)).await;
        stats.add_bytes_sent(1_000_000);
        let bitrate3 = stats.get_send_bitrate().await;
        assert!(bitrate3 > 0.0);
    }

    #[tokio::test]
    async fn test_independent_rates() {
        let stats = RtcpStats::new();

        stats.add_bytes_sent(1_000_000);
        stats.add_bytes_received(2_000_000);
        stats.add_packets_sent(1000);
        stats.add_packets_received(2000);

        tokio::time::sleep(Duration::from_millis(200)).await;

        let send_bitrate = stats.get_send_bitrate().await;
        let receive_bitrate = stats.get_receive_bitrate().await;
        let send_packet_rate = stats.get_send_packet_rate().await;
        let receive_packet_rate = stats.get_receive_packet_rate().await;

        assert!(send_bitrate > 0.0);
        assert!(receive_bitrate > 0.0);
        assert!(send_packet_rate > 0.0);
        assert!(receive_packet_rate > 0.0);

        assert!((receive_bitrate / send_bitrate - 2.0).abs() < 0.5);
        assert!((receive_packet_rate / send_packet_rate - 2.0).abs() < 0.5);
    }
}
