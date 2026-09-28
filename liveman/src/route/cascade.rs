use std::collections::HashSet;
use std::time::{Duration, Instant};

use tracing::{error, info};

use crate::config::CascadeMode;
use crate::route::utils::{cascade_pull, cascade_push, session_delete};
use crate::store::{CascadeIntent, Server};
use crate::{AppState, error::AppError, result::Result};

/// First retry delay after a failed (or unconfirmed) cascade issue; doubles
/// per attempt up to `CASCADE_RETRY_MAX` — same pacing as liveion's source
/// and target supervisors.
const CASCADE_RETRY_INITIAL: Duration = Duration::from_secs(5);
const CASCADE_RETRY_MAX: Duration = Duration::from_secs(60);
/// How often the supervisor re-evaluates intents between notifications.
const SUPERVISOR_TICK: Duration = Duration::from_millis(500);

/// Called from the WHEP path when every node hosting the stream is full.
/// Records (or reuses) a cascade *intent* and returns the destination node
/// the viewer should be proxied to; the supervisor establishes the actual
/// hop asynchronously.
pub async fn cascade_new_node(
    mut state: AppState,
    nodes: Vec<Server>,
    stream: String,
) -> Result<Server> {
    // A stream cascades to at most one destination; an existing intent pins
    // the choice so rapid successive viewers never spawn duplicate hops.
    if let Some(intent) = state.storage.cascade_intent_get(&stream) {
        return state
            .storage
            .get_map_server()
            .get(&intent.dst)
            .cloned()
            .ok_or(AppError::NoAvailableNode);
    }

    let set_all: HashSet<Server> = state.storage.nodes().await.into_iter().collect();
    let set_src: HashSet<Server> = nodes.clone().into_iter().collect();

    let server_src = nodes.first().unwrap().clone();
    let server_dst = pick_cascade_target(&set_all, &set_src).ok_or(AppError::NoAvailableNode)?;

    info!(
        "cascade intent: stream {}, from: {:?}, to: {:?}",
        stream, server_src, server_dst
    );
    state.storage.cascade_intent_insert(CascadeIntent::new(
        stream,
        server_src.alias,
        server_dst.alias.clone(),
    ));
    Ok(server_dst)
}

/// Pick the cascade destination among the nodes that do not host the stream
/// yet: the one with the largest subscriber capacity (`sub_max` comes from
/// the node's `[[nodes]] sub_max` config), ties broken by alias so the
/// choice is deterministic across calls.
fn pick_cascade_target(set_all: &HashSet<Server>, set_src: &HashSet<Server>) -> Option<Server> {
    set_all
        .difference(set_src)
        .max_by(|a, b| a.sub_max.cmp(&b.sub_max).then(a.alias.cmp(&b.alias)))
        .cloned()
}

fn retry_delay(attempts: u32) -> Duration {
    let shift = attempts.saturating_sub(1).min(10);
    (CASCADE_RETRY_INITIAL * 2u32.pow(shift)).min(CASCADE_RETRY_MAX)
}

/// Keeps every cascade intent's actual hop converged to the desired state:
/// (re)issues the cascade with exponential backoff while the destination
/// lacks a connected publisher for the stream, and runs the one-time
/// `close_other_sub` cleanup when the hop first establishes. Health is read
/// from the SSE/poll-fed storage snapshots — no extra HTTP probing.
pub async fn cascade_supervisor(mut state: AppState) {
    let notify = state.storage.cascade_notify_waiter();
    loop {
        tokio::select! {
            _ = notify.notified() => {}
            _ = tokio::time::sleep(SUPERVISOR_TICK) => {}
        }
        for mut intent in state.storage.cascade_intents() {
            supervise_intent(&mut state, &mut intent).await;
            state.storage.cascade_intent_update(intent);
        }
    }
}

async fn supervise_intent(state: &mut AppState, intent: &mut CascadeIntent) {
    let map_server = state.storage.get_map_server();
    let (src, dst) = match (map_server.get(&intent.src), map_server.get(&intent.dst)) {
        (Some(src), Some(dst)) => (src.clone(), dst.clone()),
        // A node is gone: drop the intent. The hop (if any) dies with the
        // node's own session cleanup, and a future viewer re-triggers a
        // fresh cascade onto a live node.
        _ => {
            info!(
                "cascade intent dropped (node gone): stream {}, {} -> {}",
                intent.stream, intent.src, intent.dst
            );
            state.storage.cascade_intent_remove(&intent.stream);
            return;
        }
    };

    let (healthy, has_viewers) = match state.storage.info_get(dst.alias.clone()).await {
        Ok(streams) => match streams.iter().find(|s| s.id == intent.stream) {
            Some(s) => (
                s.publish
                    .sessions
                    .iter()
                    .any(|p| p.state == api::response::RTCPeerConnectionState::Connected),
                s.subscribe
                    .sessions
                    .iter()
                    .any(|x| x.state != api::response::RTCPeerConnectionState::Closed),
            ),
            None => (false, false),
        },
        Err(_) => (false, false),
    };

    if healthy {
        intent.attempts = 0;
        intent.next_attempt = Instant::now();
        intent.was_healthy = true;
        if !intent.subs_closed && state.config.cascade.close_other_sub {
            // Pull mode: the destination's outgoing pull arrives at the
            // source as an unmarked, ordinary subscriber. Learn the hop's
            // source-side session id from the destination's publish session
            // (`cascade.session_url`) so the cleanup spares the hop itself.
            // Push mode needs nothing: the source marks its own outgoing
            // push session.
            let known_hop = match state.config.cascade.mode {
                CascadeMode::Pull => {
                    hop_session_on_source(state.client.clone(), dst.clone(), &intent.stream).await
                }
                CascadeMode::Push => None,
            };
            cascade_close_other_sub(state.clone(), src, intent.stream.clone(), known_hop).await;
            intent.subs_closed = true;
        }
        return;
    }

    // Nobody is watching the destination and the hop was healthy before its
    // death (its death dropped everyone): do not re-establish it. The idle
    // reaper owns the teardown; a viewer showing up later clears the idle
    // mark and resumes the hop here. Keying off `was_healthy` instead of
    // `attempts` keeps the gate closed after a healthy observation reset
    // the counter — otherwise a dead hop would be rebuilt exactly once into
    // a viewer-less stream and thrash with the destination's
    // auto_delete_whep.
    if intent.was_healthy && !has_viewers {
        return;
    }

    if Instant::now() < intent.next_attempt {
        return;
    }
    let mode = state.config.cascade.mode.clone();
    let result = match mode {
        CascadeMode::Push => {
            cascade_push(
                state.config.http.public.clone(),
                state.client.clone(),
                src.clone(),
                dst.clone(),
                intent.stream.clone(),
            )
            .await
        }
        CascadeMode::Pull => {
            cascade_pull(
                state.client.clone(),
                src.clone(),
                dst.clone(),
                intent.stream.clone(),
            )
            .await
        }
    };
    intent.attempts += 1;
    intent.next_attempt = Instant::now() + retry_delay(intent.attempts);
    match result {
        Ok(()) => info!(
            "cascade {:?} issued: stream {}, {} -> {} (attempt {})",
            mode, intent.stream, intent.src, intent.dst, intent.attempts
        ),
        Err(e) => error!(
            "cascade {:?} issue error: stream {}, {} -> {}: {:?} (retry in {:?})",
            mode,
            intent.stream,
            intent.src,
            intent.dst,
            e,
            retry_delay(intent.attempts)
        ),
    }
}

/// Find the session id that a pull-mode cascade created on the source node.
/// The destination's publish session records the upstream WHEP session URL
/// (`cascade.session_url`, e.g. `http://edge0:7780/session/cam0/<uuid>`),
/// whose last path segment is that session id.
async fn hop_session_on_source(
    client: reqwest::Client,
    server_dst: Server,
    stream: &str,
) -> Option<String> {
    let url = format!("{}{}", server_dst.url, api::path::streams(""));
    let mut req = client.get(url);
    if !server_dst.token.is_empty() {
        req = req.header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", server_dst.token),
        );
    }
    let body = req.send().await.ok()?.text().await.ok()?;
    let streams: Vec<api::response::Stream> = serde_json::from_str(&body).ok()?;
    let stream_info = streams.into_iter().find(|s| s.id == stream)?;
    let session_url = stream_info
        .publish
        .sessions
        .into_iter()
        .next()?
        .cascade
        .and_then(|c| c.session_url)?;
    hop_session_id(&session_url).map(str::to_string)
}

fn hop_session_id(session_url: &str) -> Option<&str> {
    session_url.rsplit('/').next().filter(|s| !s.is_empty())
}

async fn cascade_close_other_sub(
    mut state: AppState,
    server: Server,
    stream: String,
    known_hop_session: Option<String>,
) {
    match state.storage.info_get(server.clone().alias).await {
        Ok(streams) => {
            for stream_info in streams.into_iter() {
                if stream_info.id == stream {
                    for sub_info in stream_info.subscribe.sessions.into_iter() {
                        // A hop is either self-marked by the node (push mode)
                        // or identified by liveman (pull mode, see above).
                        let is_hop = sub_info.cascade.is_some()
                            || known_hop_session.as_deref() == Some(sub_info.id.as_str());
                        if is_hop {
                            info!("Skip. Is cascade hop: {:?}", sub_info.id);
                        } else {
                            match session_delete(
                                state.client.clone(),
                                server.clone(),
                                stream.clone(),
                                sub_info.id,
                            )
                            .await
                            {
                                Ok(_) => {}
                                Err(e) => error!("cascade close other sub error: {:?}", e),
                            }
                        }
                    }
                }
            }
        }
        Err(e) => error!("cascade don't closed other sub: {:?}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(alias: &str, sub_max: u16) -> Server {
        Server {
            alias: alias.to_string(),
            sub_max,
            ..Default::default()
        }
    }

    #[test]
    fn pick_target_prefers_largest_sub_max() {
        let all: HashSet<_> = [
            server("edge0", 1),
            server("edge1", 1),
            server("cloud", u16::MAX),
        ]
        .into_iter()
        .collect();
        let src: HashSet<_> = [server("edge0", 1)].into_iter().collect();
        let target = pick_cascade_target(&all, &src).unwrap();
        assert_eq!(target.alias, "cloud");
    }

    #[test]
    fn pick_target_tie_breaks_by_alias() {
        let all: HashSet<_> = [server("b", 10), server("a", 10), server("src", 1)]
            .into_iter()
            .collect();
        let src: HashSet<_> = [server("src", 1)].into_iter().collect();
        let target = pick_cascade_target(&all, &src).unwrap();
        assert_eq!(target.alias, "b");
    }

    #[test]
    fn pick_target_none_when_every_node_hosts_the_stream() {
        let all: HashSet<_> = [server("edge0", 1)].into_iter().collect();
        let src = all.clone();
        assert!(pick_cascade_target(&all, &src).is_none());
    }

    #[test]
    fn hop_session_id_parses_last_path_segment() {
        assert_eq!(
            hop_session_id("http://127.0.0.1:7780/session/cam0/abc-123"),
            Some("abc-123")
        );
        assert_eq!(hop_session_id("/session/cam0/abc-123"), Some("abc-123"));
        assert_eq!(hop_session_id("http://127.0.0.1:7780/"), None);
    }

    #[test]
    fn retry_delay_doubles_with_cap() {
        assert_eq!(retry_delay(1), CASCADE_RETRY_INITIAL);
        assert_eq!(retry_delay(2), CASCADE_RETRY_INITIAL * 2);
        assert_eq!(retry_delay(3), CASCADE_RETRY_INITIAL * 4);
        assert_eq!(retry_delay(100), CASCADE_RETRY_MAX);
    }
}
