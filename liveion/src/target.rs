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
//! as standing demand: the supervisor starts its sources at startup (the
//! established push session then keeps them alive as a subscriber).

use std::sync::Arc;
use std::time::Duration;

use libwish::Client;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::TargetConfig;
use crate::event::{Event, StreamDeleteReason};
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
        validate_target_url(&target.url)
            .map_err(|e| anyhow::anyhow!("[{}] invalid WHIP target: {}", stream, e))?;
        // Validated just above; parsing cannot fail here.
        let (url, token) = parse_whip_url(&target.url)
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
        // A configured target on an on-demand stream is standing demand: kick
        // its sources up once at startup. The established push session then
        // keeps them alive as a subscriber. No re-kick afterwards: a stream
        // that went idle returns to standby (its designed state) instead of
        // being power-cycled while e.g. the downstream is unreachable.
        #[cfg(feature = "source")]
        let mut want_kick = self.manager.is_on_demand_stream(&self.stream);
        #[cfg(not(feature = "source"))]
        let mut want_kick = false;

        info!("[target] [{}] pushing to {}", self.stream, self.url);

        loop {
            if want_kick {
                want_kick = false;
                #[cfg(feature = "source")]
                if let Err(e) = self.manager.ensure_on_demand_source(&self.stream).await {
                    failures = failures.saturating_add(1);
                    let delay = reconnect_delay(failures);
                    want_kick = true;
                    warn!(
                        "[target] [{}] on-demand source start failed: {:?}; retrying in {:?}",
                        self.stream, e, delay
                    );
                    if self.wait(delay).await {
                        break;
                    }
                    continue;
                }
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
                        // Same retry pacing as the SubscribeStopped path.
                        if lost
                            && desired
                            && self.wait(reconnect_delay(1)).await
                        {
                            break;
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

/// Delay before reconnect `attempt` (1-based): exponential backoff from a
/// 5 s base, capped at 60 s (5 s, 10 s, 20 s, 40 s, 60 s, …) — the same
/// policy the RTSP/WHEP sources apply.
fn reconnect_delay(attempt: u32) -> Duration {
    const RECONNECT_BASE_MS: u64 = 5_000;
    const RECONNECT_MAX_MS: u64 = 60_000;
    let shift = attempt.saturating_sub(1).min(4);
    Duration::from_millis(
        RECONNECT_BASE_MS
            .saturating_mul(1u64 << shift)
            .min(RECONNECT_MAX_MS),
    )
}

/// Map a `whip://` / `whips://` target URL to the `http(s)://` URL the WHIP
/// client POSTs to. A Bearer token can be carried as userinfo:
/// `whip://token@host:port/whip/stream`. Mirrors the WHEP source's URL
/// handling (`stream::source::whep_source`).
fn parse_whip_url(raw: &str) -> anyhow::Result<(String, Option<String>)> {
    // Scheme matching is case-insensitive (RFC 3986). The replacement itself
    // is done textually: `whip` is not a WHATWG "special" scheme, so
    // `Url::set_scheme` refuses the conversion to `http(s)`.
    let http_url = match raw.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("whip") => format!("http://{rest}"),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("whips") => format!("https://{rest}"),
        _ => anyhow::bail!("Unsupported WHIP target URL: {}", redact_url(raw)),
    };

    let mut url = url::Url::parse(&http_url)?;
    if url.host_str().is_none() {
        anyhow::bail!("Invalid WHIP target URL (no host): {}", redact_url(raw));
    }

    // Only token-in-username is supported. A password means the user:pass
    // form, which has no mapping onto Bearer auth — fail fast instead of
    // silently dropping it (the error must not echo the URL: it contains
    // the credential).
    if url.password().is_some() {
        anyhow::bail!(
            "WHIP target URL must not carry a password; use whip://token@host… for Bearer auth"
        );
    }

    // `Url::username` is still percent-encoded; decode so tokens containing
    // reserved characters reach the Bearer header in their original form.
    let token = (!url.username().is_empty()).then(|| {
        percent_encoding::percent_decode_str(url.username())
            .decode_utf8_lossy()
            .into_owned()
    });

    // Strip userinfo unconditionally: the URL is used for requests and log
    // lines, neither of which may see the credential.
    url.set_username("")
        .map_err(|_| anyhow::anyhow!("Invalid WHIP target URL"))?;
    url.set_password(None)
        .map_err(|_| anyhow::anyhow!("Invalid WHIP target URL"))?;

    Ok((url.to_string(), token))
}

/// `url` with any userinfo credentials stripped, safe for log lines.
/// Falls back to a scheme-only placeholder when the URL cannot be parsed
/// (an unparseable URL may still embed credentials). Copy of the `source`
/// feature's helper so this module does not depend on it.
fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => match raw.split_once("://") {
            Some((scheme, _)) => format!("{scheme}://<redacted>"),
            None => "<redacted>".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_whip_url_maps_scheme() {
        let (url, token) = parse_whip_url("whip://example.com:7777/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whip/cam1");
        assert_eq!(token, None);
    }

    #[test]
    fn parse_whip_url_scheme_is_case_insensitive() {
        let (url, _) = parse_whip_url("WHIP://example.com:7777/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whip/cam1");
        let (url, _) = parse_whip_url("Whips://example.com/whip/cam1").unwrap();
        assert_eq!(url, "https://example.com/whip/cam1");
    }

    #[test]
    fn parse_whips_url_maps_to_https() {
        let (url, token) = parse_whip_url("whips://example.com/whip/cam1").unwrap();
        assert_eq!(url, "https://example.com/whip/cam1");
        assert_eq!(token, None);
    }

    #[test]
    fn parse_whip_url_extracts_userinfo_token() {
        let (url, token) = parse_whip_url("whip://secret@example.com/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com/whip/cam1");
        assert_eq!(token, Some("secret".to_string()));
    }

    #[test]
    fn parse_whip_url_decodes_percent_encoded_token() {
        let (url, token) = parse_whip_url("whip://tok%2Fen%3D@example.com/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com/whip/cam1");
        assert_eq!(token, Some("tok/en=".to_string()));
    }

    #[test]
    fn parse_whip_url_rejects_other_schemes() {
        assert!(parse_whip_url("rtsp://example.com/stream").is_err());
        assert!(parse_whip_url("whep://example.com/whep/cam1").is_err());
    }

    #[test]
    fn parse_whip_url_rejects_password_without_leaking_it() {
        for raw in [
            "whip://user:s3cret@example.com/whip/cam1",
            "whip://:s3cret@example.com/whip/cam1",
        ] {
            let err = parse_whip_url(raw).unwrap_err();
            assert!(
                !err.to_string().contains("s3cret"),
                "error leaks the credential: {err}"
            );
        }
    }

    #[test]
    fn parse_whip_url_error_redacts_credentials() {
        let err = parse_whip_url("whop://secret@example.com/whip/cam1").unwrap_err();
        assert!(!err.to_string().contains("secret"));
    }

    #[test]
    fn redact_url_strips_userinfo() {
        assert_eq!(
            redact_url("whip://token@edge-0:7777/whip/cam1"),
            "whip://edge-0:7777/whip/cam1"
        );
        assert_eq!(redact_url("whip://tok en@not a host"), "whip://<redacted>");
        assert_eq!(redact_url("not-a-url"), "<redacted>");
    }

    #[test]
    fn reconnect_delay_doubles_with_cap() {
        assert_eq!(reconnect_delay(1), Duration::from_millis(5_000));
        assert_eq!(reconnect_delay(2), Duration::from_millis(10_000));
        assert_eq!(reconnect_delay(3), Duration::from_millis(20_000));
        assert_eq!(reconnect_delay(4), Duration::from_millis(40_000));
        assert_eq!(reconnect_delay(5), Duration::from_millis(60_000));
        // Capped afterwards, and saturating on huge attempt counts.
        assert_eq!(reconnect_delay(6), Duration::from_millis(60_000));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_millis(60_000));
    }
}
