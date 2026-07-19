//! Unified WHIP publish core shared by the RTP/RTSP bridge ([`crate::whip::into`])
//! and the synthetic publisher ([`crate::whipsynth::Publisher`]).
//!
//! The core owns peer construction (media engine, interceptors, ICE config,
//! event handler), connection-state waits and failure diagnostics. Callers add
//! their own tracks and media pumps on top of [`PublishPeer`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use rtc::peer_connection::configuration::interceptor_registry::{
    configure_nack, configure_rtcp_reports, configure_simulcast_extension_headers, configure_twcc,
};
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodecParameters, RtpCodecKind};
use rtc::statistics::StatsSelector;
use rtc::statistics::report::RTCStatsReportEntry;
use tokio::sync::{Notify, watch};
use tracing::info;
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceConnectionState, RTCIceGatheringState, RTCIceServer,
    RTCPeerConnectionState, RTCSignalingState, Registry,
};

use crate::utils;

/// Default STUN server used when the caller does not override ICE servers.
pub const DEFAULT_STUN_SERVER: &str = "stun:stun.l.google.com:19302";

/// How long to wait for the peer to reach `Connected` before giving up.
const WAIT_FOR_PEER_CONNECTED_TIMEOUT: Duration = Duration::from_secs(15);

/// Options for [`create_publish_peer`].
pub struct PublishPeerOptions {
    /// STUN server URL used for ICE gathering. `None` (or blank) disables ICE
    /// servers, which is useful on loopback-only test setups.
    pub stun_server: Option<String>,
    /// Extra video codec registrations applied *before* the default codecs so
    /// the SDP offer prefers them (e.g. H265 with sprop parameters or AV1 with
    /// a resolution-derived level-idx).
    pub extra_video_codecs: Vec<RTCRtpCodecParameters>,
}

impl Default for PublishPeerOptions {
    fn default() -> Self {
        Self {
            stun_server: Some(DEFAULT_STUN_SERVER.to_string()),
            extra_video_codecs: Vec::new(),
        }
    }
}

/// A peer built by [`create_publish_peer`]. Tracks are added by the caller.
pub struct PublishPeer {
    pub peer: Arc<dyn PeerConnection>,
    pub state_rx: watch::Receiver<RTCPeerConnectionState>,
    pub diagnostics: Arc<PublishDiagnostics>,
}

/// Create a WHIP publish peer: media engine with the default codecs (plus any
/// extra registrations), NACK/RTCP reports/TWCC interceptors, event handler
/// and ICE configuration.
///
/// Failure reporting is the caller's job: select on
/// [`wait_for_unexpected_peer_end`] next to your own cancellation token. The
/// handler intentionally does not cancel anything on failure — a token
/// cancelled by the handler races the error branch in `tokio::select!` and
/// can turn a failed session into a "successful" cancellation.
pub async fn create_publish_peer(
    gather_complete: Arc<Notify>,
    options: PublishPeerOptions,
) -> Result<PublishPeer> {
    let mut m = MediaEngine::default();
    for codec in options.extra_video_codecs {
        m.register_codec(codec, RtpCodecKind::Video)?;
    }
    m.register_default_codecs()?;

    let registry = Registry::new();
    let registry = configure_nack(registry, &mut m);
    let registry = configure_rtcp_reports(registry);
    configure_simulcast_extension_headers(&mut m)?;
    let registry = configure_twcc(registry, &mut m)?;
    info!("WHIP publish peer configured with NACK, RTCP reports, and TWCC");

    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
    let diagnostics = Arc::new(PublishDiagnostics::default());
    let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(PublishPeerHandler {
        gather_complete,
        state_tx,
        diagnostics: diagnostics.clone(),
    });

    let mut config_builder = RTCConfigurationBuilder::new();
    if let Some(stun_server) = options.stun_server.filter(|s| !s.trim().is_empty()) {
        config_builder = config_builder.with_ice_servers(vec![RTCIceServer {
            urls: vec![stun_server],
            username: String::new(),
            credential: String::new(),
        }]);
    }

    let peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_udp_addrs(utils::webrtc::ice_udp_addrs())
            .with_configuration(config_builder.build())
            .build()
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

    Ok(PublishPeer {
        peer,
        state_rx,
        diagnostics,
    })
}

/// Wait until the peer reaches `Connected`, with diagnostics on failure.
pub async fn wait_for_peer_connected(
    peer: Arc<dyn PeerConnection>,
    state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<PublishDiagnostics>,
) -> Result<()> {
    wait_for_peer_connected_with_timeout(
        state_rx,
        diagnostics,
        WAIT_FOR_PEER_CONNECTED_TIMEOUT,
        move || {
            let peer = peer.clone();
            async move { format_ice_stats(peer).await }
        },
    )
    .await
}

pub async fn wait_for_peer_connected_with_timeout<F, Fut>(
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<PublishDiagnostics>,
    timeout: Duration,
    ice_stats: F,
) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = String>,
{
    let wait_result = tokio::time::timeout(timeout, async {
        loop {
            let state = *state_rx.borrow_and_update();
            match state {
                RTCPeerConnectionState::Connected => return Ok(()),
                RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected => {
                    return Err(anyhow!(
                        "WHIP peer connection ended before becoming connected: state={state}"
                    ));
                }
                _ => {}
            }

            state_rx
                .changed()
                .await
                .map_err(|_| anyhow!("WHIP peer connection state channel closed"))?;
        }
    })
    .await;

    match wait_result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let ice_stats = ice_stats().await;
            Err(anyhow!(
                "{error}, {}, ice_stats=[{}]",
                diagnostics.format(),
                ice_stats
            ))
        }
        Err(_) => {
            let ice_stats = ice_stats().await;
            Err(anyhow!(
                "WHIP peer connection timed out waiting for connected after {:?}: {}, ice_stats=[{}]",
                timeout,
                diagnostics.format(),
                ice_stats
            ))
        }
    }
}

/// Watch a connected peer and return an error if it ends before shutdown.
pub async fn wait_for_unexpected_peer_end(
    peer: Arc<dyn PeerConnection>,
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<PublishDiagnostics>,
) -> Result<()> {
    let mut saw_connected = *state_rx.borrow() == RTCPeerConnectionState::Connected;

    loop {
        let state = *state_rx.borrow();
        if state == RTCPeerConnectionState::Connected {
            saw_connected = true;
        }

        if matches!(
            state,
            RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
        ) {
            let ice_stats = format_ice_stats(peer.clone()).await;
            return Err(anyhow!(
                "WHIP peer connection ended before shutdown: state={state}, connected_before={saw_connected}, {}, ice_stats=[{}]",
                diagnostics.format(),
                ice_stats
            ));
        }

        state_rx
            .changed()
            .await
            .map_err(|_| anyhow!("WHIP peer connection state channel closed"))?;
    }
}

/// Connection diagnostics captured during a publish session for error reports.
#[derive(Default)]
pub struct PublishDiagnostics {
    connection_states: Mutex<Vec<String>>,
    ice_connection_states: Mutex<Vec<String>>,
    ice_gathering_states: Mutex<Vec<String>>,
    signaling_states: Mutex<Vec<String>>,
    local_sdp_summary: Mutex<Option<String>>,
    remote_sdp_summary: Mutex<Option<String>>,
}

impl PublishDiagnostics {
    pub fn set_sdp_summaries(&self, local: String, remote: String) {
        if let Ok(mut summary) = self.local_sdp_summary.lock() {
            *summary = Some(local);
        }
        if let Ok(mut summary) = self.remote_sdp_summary.lock() {
            *summary = Some(remote);
        }
    }

    pub fn format(&self) -> String {
        format!(
            "connection_states=[{}], ice_connection_states=[{}], ice_gathering_states=[{}], signaling_states=[{}], local_sdp_summary=[{}], remote_sdp_summary=[{}]",
            join_states(&self.connection_states),
            join_states(&self.ice_connection_states),
            join_states(&self.ice_gathering_states),
            join_states(&self.signaling_states),
            optional_summary(&self.local_sdp_summary),
            optional_summary(&self.remote_sdp_summary),
        )
    }
}

fn push_state(states: &Mutex<Vec<String>>, state: impl std::fmt::Display) {
    match states.lock() {
        Ok(mut states) => states.push(state.to_string()),
        Err(poisoned) => {
            // Recover the inner data after a panic so diagnostics are still
            // available for error reporting.
            let mut states = poisoned.into_inner();
            states.push(state.to_string());
        }
    }
}

fn join_states(states: &Mutex<Vec<String>>) -> String {
    states
        .lock()
        .map(|states| states.join(" -> "))
        .unwrap_or_else(|poisoned| format!("{}(poisoned)", poisoned.into_inner().join(" -> ")))
}

fn optional_summary(summary: &Mutex<Option<String>>) -> String {
    summary
        .lock()
        .map(|summary| {
            summary
                .as_deref()
                .unwrap_or("<not captured>")
                .replace('\n', " | ")
        })
        .unwrap_or_else(|_| "<poisoned>".to_string())
}

struct PublishPeerHandler {
    gather_complete: Arc<Notify>,
    state_tx: watch::Sender<RTCPeerConnectionState>,
    diagnostics: Arc<PublishDiagnostics>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PublishPeerHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("WHIP publish connection state changed: {}", state);
        push_state(&self.diagnostics.connection_states, state);
        let _ = self.state_tx.send(state);
    }

    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        info!("WHIP publish ICE connection state changed: {}", state);
        push_state(&self.diagnostics.ice_connection_states, state);
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        info!("WHIP publish ICE gathering state changed: {}", state);
        push_state(&self.diagnostics.ice_gathering_states, state);
        if state == RTCIceGatheringState::Complete {
            info!("WHIP publish ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }

    async fn on_signaling_state_change(&self, state: RTCSignalingState) {
        info!("WHIP publish signaling state changed: {}", state);
        push_state(&self.diagnostics.signaling_states, state);
    }
}

pub async fn format_ice_stats(peer: Arc<dyn PeerConnection>) -> String {
    let report = peer
        .get_stats(std::time::Instant::now(), StatsSelector::None)
        .await;
    let mut lines = Vec::new();

    for entry in report.iter() {
        match entry {
            RTCStatsReportEntry::IceCandidatePair(pair) => {
                lines.push(format!(
                    "candidate_pair id={} local={} remote={} state={:?} nominated={} packets_sent={} packets_received={} bytes_sent={} bytes_received={} requests_sent={} requests_received={} responses_sent={} responses_received={}",
                    pair.stats.id,
                    pair.local_candidate_id,
                    pair.remote_candidate_id,
                    pair.state,
                    pair.nominated,
                    pair.packets_sent,
                    pair.packets_received,
                    pair.bytes_sent,
                    pair.bytes_received,
                    pair.requests_sent,
                    pair.requests_received,
                    pair.responses_sent,
                    pair.responses_received,
                ));
            }
            RTCStatsReportEntry::LocalCandidate(candidate) => {
                lines.push(format!(
                    "local_candidate id={} address={} port={} protocol={} type={:?} foundation={} related={}:{}",
                    candidate.stats.id,
                    candidate.address.as_deref().unwrap_or("<redacted>"),
                    candidate.port,
                    candidate.protocol,
                    candidate.candidate_type,
                    candidate.foundation,
                    candidate.related_address,
                    candidate.related_port,
                ));
            }
            RTCStatsReportEntry::RemoteCandidate(candidate) => {
                lines.push(format!(
                    "remote_candidate id={} address={} port={} protocol={} type={:?} foundation={} related={}:{}",
                    candidate.stats.id,
                    candidate.address.as_deref().unwrap_or("<redacted>"),
                    candidate.port,
                    candidate.protocol,
                    candidate.candidate_type,
                    candidate.foundation,
                    candidate.related_address,
                    candidate.related_port,
                ));
            }
            _ => {}
        }
    }

    if lines.is_empty() {
        "<no ice candidate stats>".to_string()
    } else {
        lines.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn waits_for_connected_before_starting_media_transport() {
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let diagnostics = Arc::new(PublishDiagnostics::default());
        let started = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));

        let task = {
            let started = started.clone();
            let order = order.clone();
            tokio::spawn(async move {
                wait_for_peer_connected_with_timeout(
                    state_rx.clone(),
                    diagnostics,
                    Duration::from_secs(1),
                    || async { "ice-stats".to_string() },
                )
                .await?;

                started.fetch_add(1, Ordering::SeqCst);
                order.lock().unwrap().push("stats");
                started.fetch_add(1, Ordering::SeqCst);
                order.lock().unwrap().push("transport");
                Result::<()>::Ok(())
            })
        };

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(started.load(Ordering::SeqCst), 0);

        state_tx.send(RTCPeerConnectionState::Connected).unwrap();

        task.await.unwrap().unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 2);
        assert_eq!(order.lock().unwrap().as_slice(), ["stats", "transport"]);
    }

    #[tokio::test]
    async fn returns_error_with_diagnostics_when_peer_fails_before_connected() {
        for state in [
            RTCPeerConnectionState::Failed,
            RTCPeerConnectionState::Closed,
            RTCPeerConnectionState::Disconnected,
        ] {
            let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
            let diagnostics = Arc::new(PublishDiagnostics::default());

            state_tx.send(state).unwrap();

            let error = wait_for_peer_connected_with_timeout(
                state_rx,
                diagnostics,
                Duration::from_secs(1),
                || async { "candidate_pair state=failed".to_string() },
            )
            .await
            .unwrap_err()
            .to_string();

            assert!(error.contains("before becoming connected"));
            assert!(error.contains("connection_states="));
            assert!(error.contains("candidate_pair state=failed"));
        }
    }

    #[tokio::test]
    async fn returns_error_with_diagnostics_when_wait_for_connected_times_out() {
        let (_state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let diagnostics = Arc::new(PublishDiagnostics::default());

        let error = wait_for_peer_connected_with_timeout(
            state_rx,
            diagnostics,
            Duration::from_millis(10),
            || async { "<no ice candidate stats>".to_string() },
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("timed out waiting"));
        assert!(error.contains("connection_states="));
        assert!(error.contains("<no ice candidate stats>"));
    }
}
