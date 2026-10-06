//! Output targets (declarative cascade-push / RTP sender / RTSP push),
//! static and runtime-managed.
//!
//! A `[[stream.<name>.targets]]` config entry with a `whip://`/`whips://` URL
//! pushes the stream to a downstream WHIP endpoint (typically another
//! live777 node) — the static counterpart of `POST /api/cascade/{stream}`
//! with a `target_url`, just as the WHEP source is the static counterpart
//! of a cascade pull. An `rtp://` URL instead sends the media out as plain
//! RTP over UDP, see [`crate::target_rtp`]. An `rtsp://` URL pushes the
//! media to an RTSP server as a client (ANNOUNCE/SETUP/RECORD), see
//! [`crate::target_rtsp`]. The same three schemes can also be added, listed
//! and removed at runtime through `POST`/`GET`/`DELETE /api/targets/...`
//! (live777#473): runtime targets share the supervisor semantics below, are
//! registered in the manager's target registry next to the static ones, and
//! — like cascade pushes — do not persist across restarts.
//!
//! The push is media-driven: one supervisor task per target establishes the
//! cascade-push session when the stream gains a publisher (`PublishStarted`,
//! real WHIP or a source's virtual one) and tears it down when the publisher
//! goes away (`PublishStopped`). Negotiating per media epoch keeps the push
//! session's codecs matched to the current publisher — a session negotiated
//! before the codec is known could not carry a later, different codec.
//! Downstream nodes therefore see ordinary publisher attach/detach cycles
//! and their own `auto_delete_*`/on-demand strategies keep working.
//!
//! A failed push (downstream down, ICE failure, session loss) is retried
//! with exponential backoff, mirroring the reconnect policy of the
//! RTSP/WHEP sources. For an `on_demand` stream the configured target acts
//! as standing demand: the supervisor registers with the manager's
//! on-demand idle accounting for its whole lifetime, so the sources are
//! never idle-stopped underneath it (live777#481), and whenever the stream
//! has neither a publisher nor a push session the supervisor starts its
//! sources, retried with the same backoff — so the relay recovers on its
//! own once an unreachable downstream is back.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
#[cfg(feature = "target-whip")]
use std::time::Duration;

#[cfg(feature = "target-whip")]
use libwish::{Client, parse_whip_url};
#[cfg(feature = "target-whip")]
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "target-whip")]
use tracing::{debug, warn};
use tracing::{error, info};

use crate::config::TargetConfig;
#[cfg(feature = "target-whip")]
use crate::event::{Event, StreamDeleteReason};
#[cfg(feature = "target-whip")]
use crate::reconnect::reconnect_delay;
use crate::stream::manager::Manager;

/// Where a registered target came from: static config entries are owned by
/// the config file (the runtime API cannot remove them); runtime entries
/// were added through the API and vanish on restart, like cascade pushes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetOrigin {
    Config,
    Runtime,
}

impl TargetOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetOrigin::Config => "config",
            TargetOrigin::Runtime => "runtime",
        }
    }
}

/// One registered output target: its config and the per-target cancel token
/// stopping its supervisor (a child of the manager's shutdown token).
pub(crate) struct TargetEntry {
    pub config: TargetConfig,
    pub origin: TargetOrigin,
    pub cancel: CancellationToken,
}

/// Why adding a runtime target failed.
#[derive(Debug)]
pub(crate) enum StartTargetError {
    /// The stream does not exist (not provisioned, not live).
    StreamNotFound(String),
    /// A target with the same URL is already registered for the stream.
    Duplicate(String),
    /// URL or option validation failed.
    Invalid(anyhow::Error),
}

/// Why removing a runtime target failed.
#[derive(Debug)]
pub(crate) enum RemoveTargetError {
    /// No target with the URL is registered for the stream.
    NotFound,
    /// The target is a static config entry, not removable through the API.
    ConfigOwned,
}

/// The target URL as it may appear in API responses: credentials (the
/// WHIP token rides as userinfo) stripped.
pub(crate) fn redact_target_url(raw: &str) -> String {
    let raw = raw.trim();
    match raw.split_once("://") {
        Some((scheme, rest)) => {
            let rest = rest
                .rsplit_once('@')
                .map(|(_, after)| after)
                .unwrap_or(rest);
            format!("{scheme}://{rest}")
        }
        None => raw.to_string(),
    }
}

/// Add a target through the runtime API: the stream must exist (provisioned
/// or live), the URL must validate, and no target with the same URL may be
/// registered for the stream yet.
pub(crate) async fn start_runtime_target(
    manager: &Arc<Manager>,
    stream: String,
    target: TargetConfig,
) -> Result<(), StartTargetError> {
    if manager.info(vec![stream.clone()]).await.is_empty() {
        return Err(StartTargetError::StreamNotFound(stream));
    }
    start_target(manager, stream, target, TargetOrigin::Runtime)
}

/// Register one output target and spawn its supervisor. Shared by
/// [`init`] (static config) and the runtime API: the supervisor gets a
/// per-target child of the manager's shutdown token, so a runtime target
/// can be stopped individually, and the registry entry is dropped when the
/// supervisor exits (stream deleted, or removed via the API).
pub(crate) fn start_target(
    manager: &Arc<Manager>,
    stream: String,
    target: TargetConfig,
    origin: TargetOrigin,
) -> Result<(), StartTargetError> {
    target
        .validate()
        .map_err(|e| StartTargetError::Invalid(anyhow::anyhow!("[{stream}] {e}")))?;

    // Config validation already rejected unknown schemes; stay defensive so
    // a programmatic caller cannot panic the dispatch.
    let scheme = target
        .url
        .trim()
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let cancel = manager.cancel_token().child_token();
    let run: Pin<Box<dyn Future<Output = ()> + Send>> = match scheme.as_str() {
        #[cfg(feature = "target-whip")]
        "whip" | "whips" => {
            match TargetContext::new(
                manager.clone(),
                stream.clone(),
                target.clone(),
                cancel.clone(),
            ) {
                Ok(ctx) => Box::pin(ctx.run()),
                Err(e) => return Err(StartTargetError::Invalid(anyhow::anyhow!(e))),
            }
        }
        #[cfg(feature = "target-rtp")]
        "rtp" => match crate::target_rtp::RtpTargetContext::new(
            manager.clone(),
            stream.clone(),
            target.clone(),
            cancel.clone(),
        ) {
            Ok(ctx) => Box::pin(ctx.run()),
            Err(e) => return Err(StartTargetError::Invalid(anyhow::anyhow!(e))),
        },
        #[cfg(feature = "target-rtsp")]
        "rtsp" => match crate::target_rtsp::RtspTargetContext::new(
            manager.clone(),
            stream.clone(),
            target.clone(),
            cancel.clone(),
        ) {
            Ok(ctx) => Box::pin(ctx.run()),
            Err(e) => return Err(StartTargetError::Invalid(anyhow::anyhow!(e))),
        },
        other => {
            return Err(StartTargetError::Invalid(anyhow::anyhow!(
                "[{stream}] unsupported target url scheme: {other}"
            )));
        }
    };

    let key = (stream, target.url.trim().to_string());
    if !manager.register_target(
        key.clone(),
        TargetEntry {
            config: target,
            origin,
            cancel,
        },
    ) {
        return Err(StartTargetError::Duplicate(key.1));
    }

    let cleanup = manager.clone();
    tokio::spawn(async move {
        run.await;
        cleanup.unregister_target(&key);
    });
    Ok(())
}

/// Session id under which a static target registers as a virtual subscriber
/// (mirroring the source's `virtual-source` publisher): derived from the
/// credential-free target URL with the scheme separator collapsed and
/// path-hostile characters flattened, e.g.
/// `virtual-target-rtp-230.1.1.2:1720`.
pub(crate) fn virtual_target_session_id(display: &str) -> String {
    let display = display.replacen("://", "-", 1);
    let sanitized: String = display
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("virtual-target-{sanitized}")
}

/// Spawn one supervisor per configured static target. Must run after
/// `Manager::provision_streams`: the supervisors snapshot stream state from
/// the manager (the event bus does not replay).
pub fn init(manager: Arc<Manager>) {
    let targets = manager.static_targets();
    if targets.is_empty() {
        return;
    }
    info!(
        "[Server] Starting {} configured target(s)...",
        targets.len()
    );
    for (stream, target) in targets {
        if let Err(e) = start_target(&manager, stream, target, TargetOrigin::Config) {
            error!("[target] {:?}", e);
        }
    }
}

#[cfg(feature = "target-whip")]
struct TargetContext {
    manager: Arc<Manager>,
    stream: String,
    /// `http(s)://` URL handed to the WHIP client, credentials stripped (the
    /// token travels separately), so it is safe for log lines.
    url: String,
    token: Option<String>,
    cancel: CancellationToken,
}

#[cfg(feature = "target-whip")]
impl TargetContext {
    fn new(
        manager: Arc<Manager>,
        stream: String,
        target: TargetConfig,
        cancel: CancellationToken,
    ) -> anyhow::Result<Self> {
        let (url, token) = parse_whip_url(&target.url)
            .map_err(|e| anyhow::anyhow!("[{}] invalid WHIP target: {}", stream, e))?;
        // The token reaches the Authorization header verbatim on every push;
        // reject an invalid header value now instead of inside the retry
        // loop.
        Client::get_authorization_header_map(token.clone())
            .map_err(|e| anyhow::anyhow!("[{}] invalid WHIP target: {}", stream, e))?;
        Ok(Self {
            manager,
            stream,
            url,
            token,
            cancel,
        })
    }

    async fn run(self) {
        // Subscribe before the initial snapshot/kick so media transitions
        // happening in between are still observed.
        let mut events = self.manager.subscribe_event();
        let mut session: Option<String> = None;
        // Consecutive failed kick/push attempts, reset once a session is up.
        // The attempt spacing mirrors the RTSP/WHEP source reconnect policy.
        let mut failures: u32 = 0;
        // The bus does not replay: media that became available before this
        // task started (always-on sources, early publishers) is only visible
        // through the manager snapshot.
        let mut desired = self.manager.has_publisher(&self.stream).await;
        // A configured target on an on-demand stream is standing demand:
        // whenever the stream has neither a publisher nor a push session,
        // kick its sources. Retried with the same backoff as push failures,
        // so an unreachable downstream caps at roughly one source restart
        // per minute — and the relay recovers on its own once the
        // downstream is back.
        #[cfg(feature = "source")]
        let standing_demand = self.manager.is_on_demand_stream(&self.stream);
        // Register as a virtual subscriber for the supervisor's whole
        // lifetime: the push session only exists while the downstream is up,
        // so during push retries nothing would otherwise keep the on-demand
        // idle check from stopping the sources between attempts
        // (live777#481). The registration also lists the target in the
        // stream's `subscribe.sessions`. `self.url` is the credential-free
        // http(s) form of the configured whip(s) URL; map the scheme back
        // for the session id.
        let virtual_id = virtual_target_session_id(&self.url.replacen("http", "whip", 1));
        self.manager
            .add_virtual_subscriber(&self.stream, virtual_id.clone())
            .await;

        info!("[target] [{}] pushing to {}", self.stream, self.url);

        loop {
            #[cfg(feature = "source")]
            if standing_demand && !desired && session.is_none() {
                if let Err(e) = self.manager.ensure_on_demand_source(&self.stream).await {
                    failures = failures.saturating_add(1);
                    let delay = reconnect_delay(failures);
                    warn!(
                        "[target] [{}] on-demand source start failed: {:?}; retrying in {:?}",
                        self.stream, e, delay
                    );
                    if self.wait(delay).await {
                        break;
                    }
                    // The wait is event-blind: a real publisher may have
                    // arrived meanwhile.
                    desired = self.manager.has_publisher(&self.stream).await;
                    continue;
                }
                // The kick blocks until the source bridge is up, so the
                // virtual publisher is already visible in the snapshot — no
                // need to wait for PublishStarted.
                desired = self.manager.has_publisher(&self.stream).await;
            }

            if desired && session.is_none() {
                match self
                    .manager
                    .cascade_push(self.stream.clone(), self.url.clone(), self.token.clone())
                    .await
                {
                    Ok(id) => {
                        info!(
                            "[target] [{}] push session {} established towards {}",
                            self.stream, id, self.url
                        );
                        session = Some(id);
                        failures = 0;
                    }
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        let delay = reconnect_delay(failures);
                        warn!(
                            "[target] [{}] push to {} failed: {:?}; retrying in {:?}",
                            self.stream, self.url, e, delay
                        );
                        if self.wait(delay).await {
                            break;
                        }
                        // The backoff wait is event-blind: the media may have
                        // gone away mid-sleep, and pushing now would
                        // negotiate the session with the wrong codecs.
                        desired = self.manager.has_publisher(&self.stream).await;
                        continue;
                    }
                }
            } else if !desired && session.is_some() {
                let id = session.take().expect("session checked above");
                debug!(
                    "[target] [{}] media gone; removing push session {}",
                    self.stream, id
                );
                let _ = self
                    .manager
                    .remove_stream_session(self.stream.clone(), id)
                    .await;
                continue;
            }

            tokio::select! {
                _ = self.cancel.cancelled() => break,
                event = events.recv() => match event {
                    Ok(Event::PublishStarted { stream, .. }) => {
                        if stream == self.stream {
                            desired = true;
                        }
                    }
                    Ok(Event::PublishStopped { stream, .. }) => {
                        if stream == self.stream {
                            desired = false;
                        }
                    }
                    Ok(Event::SubscribeStopped { stream, session: id, reason }) => {
                        if stream != self.stream || session.as_deref() != Some(id.as_str()) {
                            continue;
                        }
                        info!(
                            "[target] [{}] push session {} ended ({:?})",
                            self.stream, id, reason
                        );
                        session = None;
                        // A session that was up resets the backoff, but the
                        // first retry still waits the base delay — same as a
                        // connected source dropping.
                        failures = 1;
                        if desired {
                            if self.wait(reconnect_delay(1)).await {
                                break;
                            }
                            // The wait is event-blind: re-check the media is
                            // still there before re-establishing.
                            desired = self.manager.has_publisher(&self.stream).await;
                        }
                    }
                    Ok(Event::StreamDeleted { stream, reason }) => {
                        if stream != self.stream {
                            continue;
                        }
                        // Only an outright removal ends the target. A
                        // provisioned stream cannot be removed (its resets
                        // arrive as a Reset pair), so this is defensive.
                        if reason != StreamDeleteReason::Reset {
                            info!(
                                "[target] [{}] stream deleted, stopping push to {}",
                                self.stream, self.url
                            );
                            break;
                        }
                    }
                    // Missed events may have lost a publish/subscribe
                    // transition: reconcile both state halves against the
                    // manager's actual state.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            "[target] [{}] dropped {} stream events, reconciling",
                            self.stream, n
                        );
                        desired = self.manager.has_publisher(&self.stream).await;
                        let mut lost = false;
                        if let Some(id) = &session
                            && !self.session_alive(id).await
                        {
                            warn!(
                                "[target] [{}] push session {} lost during event lag",
                                self.stream, id
                            );
                            session = None;
                            failures = 1;
                            lost = true;
                        }
                        // Same retry pacing as the SubscribeStopped path,
                        // including the post-wait media re-check.
                        if lost && desired {
                            if self.wait(reconnect_delay(1)).await {
                                break;
                            }
                            desired = self.manager.has_publisher(&self.stream).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                },
            }
        }

        if let Some(id) = session.take() {
            debug!("[target] [{}] removing push session {}", self.stream, id);
            let _ = self
                .manager
                .remove_stream_session(self.stream.clone(), id)
                .await;
        }
        self.manager
            .remove_virtual_subscriber(&self.stream, &virtual_id)
            .await;
        info!("[target] [{}] stopped push to {}", self.stream, self.url);
    }

    /// Sleep for `delay`, returning `true` early when shutdown is requested.
    async fn wait(&self, delay: Duration) -> bool {
        tokio::select! {
            _ = self.cancel.cancelled() => true,
            _ = tokio::time::sleep(delay) => false,
        }
    }

    /// Whether the manager still lists `id` as a live subscribe session of
    /// this target's stream.
    async fn session_alive(&self, id: &str) -> bool {
        self.manager
            .info(vec![self.stream.clone()])
            .await
            .first()
            .is_some_and(|s| {
                s.subscribe
                    .sessions
                    .iter()
                    .any(|x| x.id == id && x.leave_at == 0)
            })
    }
}

/// Validate a configured target URL: scheme, parseability, host presence,
/// userinfo rules, and that the token (if any) is usable as a Bearer header
/// value. Called from `Config::validate` so misconfiguration fails at
/// startup instead of surfacing once in a supervisor log line.
#[cfg(feature = "target-whip")]
pub(crate) fn validate_target_url(raw: &str) -> anyhow::Result<()> {
    let (_, token) = parse_whip_url(raw)?;
    // The token reaches the Authorization header verbatim on every push.
    Client::get_authorization_header_map(token)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::virtual_target_session_id;

    #[test]
    fn virtual_target_session_id_is_path_safe_and_readable() {
        assert_eq!(
            virtual_target_session_id("rtp://230.1.1.2:1720"),
            "virtual-target-rtp-230.1.1.2:1720"
        );
        assert_eq!(
            virtual_target_session_id("rtsp://mediamtx.example.com:8554/live/dog"),
            "virtual-target-rtsp-mediamtx.example.com:8554_live_dog"
        );
        assert_eq!(
            virtual_target_session_id("whips://edge.example.com/whip/cam?x=1"),
            "virtual-target-whips-edge.example.com_whip_cam_x_1"
        );
    }

    #[cfg(feature = "target-rtp")]
    mod runtime_registry {
        use std::sync::Arc;

        use tokio_util::sync::CancellationToken;

        use super::super::{
            RemoveTargetError, StartTargetError, TargetOrigin, init, redact_target_url,
            start_runtime_target,
        };
        use crate::config::{Config, StreamConfig, StreamEntry, TargetConfig};
        use crate::stream::manager::Manager;

        fn rtp_target(url: &str) -> TargetConfig {
            TargetConfig {
                url: url.to_string(),
                multicast_interface: None,
                ttl: None,
                payload_type: None,
                sdp_file: None,
            }
        }

        #[test]
        fn redact_target_url_strips_userinfo() {
            assert_eq!(
                redact_target_url("whip://secret-token@edge.example.com/whip/cam"),
                "whip://edge.example.com/whip/cam"
            );
            assert_eq!(
                redact_target_url("rtp://230.1.1.1:1720"),
                "rtp://230.1.1.1:1720"
            );
        }

        /// The runtime lifecycle: add, list, duplicate and invalid
        /// rejection, remove, not-found. Unknown streams are rejected.
        #[tokio::test]
        async fn runtime_target_registry_lifecycle() {
            let cancel = CancellationToken::new();
            let manager = Arc::new(Manager::new(Config::default(), cancel.clone()).await);
            manager.stream_create("cam".to_string()).await.unwrap();

            let err = start_runtime_target(
                &manager,
                "ghost".to_string(),
                rtp_target("rtp://127.0.0.1:5004"),
            )
            .await;
            assert!(matches!(err, Err(StartTargetError::StreamNotFound(_))));

            start_runtime_target(
                &manager,
                "cam".to_string(),
                rtp_target("rtp://127.0.0.1:5004"),
            )
            .await
            .unwrap();
            let targets = manager.list_targets();
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].0, "cam");
            assert_eq!(targets[0].2, TargetOrigin::Runtime);

            // Same URL on the same stream conflicts; on another stream it
            // does not.
            let err = start_runtime_target(
                &manager,
                "cam".to_string(),
                rtp_target("rtp://127.0.0.1:5004"),
            )
            .await;
            assert!(matches!(err, Err(StartTargetError::Duplicate(_))));

            let err =
                start_runtime_target(&manager, "cam".to_string(), rtp_target("rtp://127.0.0.1:0"))
                    .await;
            assert!(matches!(err, Err(StartTargetError::Invalid(_))));

            let entry = manager
                .remove_runtime_target("cam", "rtp://127.0.0.1:5004")
                .unwrap();
            entry.cancel.cancel();
            assert!(manager.list_targets().is_empty());
            assert!(matches!(
                manager.remove_runtime_target("cam", "rtp://127.0.0.1:5004"),
                Err(RemoveTargetError::NotFound)
            ));

            cancel.cancel();
        }

        /// Static config targets register at startup with origin `config`;
        /// the runtime path cannot remove them, and a runtime target with
        /// the same URL conflicts.
        #[tokio::test]
        async fn config_target_is_registered_and_not_removable() {
            let mut streams = std::collections::HashMap::new();
            streams.insert(
                "cam".to_string(),
                StreamEntry {
                    targets: vec![rtp_target("rtp://127.0.0.1:5004")],
                    ..Default::default()
                },
            );
            let config = Config {
                stream: StreamConfig { streams },
                ..Default::default()
            };
            let cancel = CancellationToken::new();
            let manager = Arc::new(Manager::new(config, cancel.clone()).await);
            manager.provision_streams().await;
            init(manager.clone());

            let targets = manager.list_targets();
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].2, TargetOrigin::Config);

            assert!(matches!(
                manager.remove_runtime_target("cam", "rtp://127.0.0.1:5004"),
                Err(RemoveTargetError::ConfigOwned)
            ));
            assert_eq!(manager.list_targets().len(), 1);

            let err = start_runtime_target(
                &manager,
                "cam".to_string(),
                rtp_target("rtp://127.0.0.1:5004"),
            )
            .await;
            assert!(matches!(err, Err(StartTargetError::Duplicate(_))));

            cancel.cancel();
        }
    }
}
