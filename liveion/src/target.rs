//! Static WHIP push targets (declarative cascade-push).
//!
//! A `[[stream.<name>.targets]]` config entry pushes the stream to a
//! downstream WHIP endpoint (typically another live777 node) — the static
//! counterpart of `POST /api/cascade/{stream}` with a `target_url`, just as
//! the WHEP source is the static counterpart of a cascade pull.
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
//! as standing demand: whenever the stream has neither a publisher nor a
//! push session, the supervisor starts its sources, retried with the same
//! backoff — so the relay recovers on its own once an unreachable
//! downstream is back, capped at roughly one source restart per minute.

use std::sync::Arc;
use std::time::Duration;

use libwish::{Client, parse_whip_url};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::TargetConfig;
use crate::event::{Event, StreamDeleteReason};
use crate::reconnect::reconnect_delay;
use crate::stream::manager::Manager;

/// Spawn one supervisor per configured static target. Must run after
/// `Manager::provision_streams`: the supervisors snapshot stream state from
/// the manager (the event bus does not replay).
pub fn init(manager: Arc<Manager>) {
    let targets = manager.static_targets();
    if targets.is_empty() {
        return;
    }
    info!(
        "[Server] Starting {} configured WHIP target(s)...",
        targets.len()
    );
    for (stream, target) in targets {
        match TargetContext::new(manager.clone(), stream, target) {
            Ok(ctx) => {
                tokio::spawn(ctx.run());
            }
            Err(e) => error!("[target] {}", e),
        }
    }
}

struct TargetContext {
    manager: Arc<Manager>,
    stream: String,
    /// `http(s)://` URL handed to the WHIP client, credentials stripped (the
    /// token travels separately), so it is safe for log lines.
    url: String,
    token: Option<String>,
    cancel: CancellationToken,
}

impl TargetContext {
    fn new(manager: Arc<Manager>, stream: String, target: TargetConfig) -> anyhow::Result<Self> {
        let (url, token) = parse_whip_url(&target.url)
            .map_err(|e| anyhow::anyhow!("[{}] invalid WHIP target: {}", stream, e))?;
        // The token reaches the Authorization header verbatim on every push;
        // reject an invalid header value now instead of inside the retry
        // loop.
        Client::get_authorization_header_map(token.clone())
            .map_err(|e| anyhow::anyhow!("[{}] invalid WHIP target: {}", stream, e))?;
        let cancel = manager.cancel_token();
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
        let mut desired = self.has_publisher().await;
        // A configured target on an on-demand stream is standing demand:
        // whenever the stream has neither a publisher nor a push session,
        // kick its sources. Retried with the same backoff as push failures,
        // so an unreachable downstream caps at roughly one source restart
        // per minute — and the relay recovers on its own once the
        // downstream is back.
        #[cfg(feature = "source")]
        let standing_demand = self.manager.is_on_demand_stream(&self.stream);

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
                    desired = self.has_publisher().await;
                    continue;
                }
                // The kick blocks until the source bridge is up, so the
                // virtual publisher is already visible in the snapshot — no
                // need to wait for PublishStarted.
                desired = self.has_publisher().await;
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
                        desired = self.has_publisher().await;
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
                            desired = self.has_publisher().await;
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
                        desired = self.has_publisher().await;
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
                            desired = self.has_publisher().await;
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
        info!("[target] [{}] stopped push to {}", self.stream, self.url);
    }

    /// Sleep for `delay`, returning `true` early when shutdown is requested.
    async fn wait(&self, delay: Duration) -> bool {
        tokio::select! {
            _ = self.cancel.cancelled() => true,
            _ = tokio::time::sleep(delay) => false,
        }
    }

    /// Whether the stream currently has a live publisher session (a real
    /// WHIP publisher or a source bridge's virtual one).
    async fn has_publisher(&self) -> bool {
        self.manager
            .info(vec![self.stream.clone()])
            .await
            .first()
            .is_some_and(|s| s.publish.sessions.iter().any(|x| x.leave_at == 0))
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
pub(crate) fn validate_target_url(raw: &str) -> anyhow::Result<()> {
    let (_, token) = parse_whip_url(raw)?;
    // The token reaches the Authorization header verbatim on every push.
    Client::get_authorization_header_map(token)?;
    Ok(())
}
