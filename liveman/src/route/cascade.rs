use std::collections::HashSet;

use tracing::{error, info};

use crate::config::CascadeMode;
use crate::route::utils::{cascade_pull, cascade_push, force_check_times, session_delete};
use crate::store::Server;
use crate::{AppState, error::AppError, result::Result};

pub async fn cascade_new_node(
    mut state: AppState,
    nodes: Vec<Server>,
    stream: String,
) -> Result<Server> {
    let set_all: HashSet<Server> = state.storage.nodes().await.into_iter().collect();
    let set_src: HashSet<Server> = nodes.clone().into_iter().collect();

    let server_src = nodes.first().unwrap().clone();
    let server_ds0 = pick_cascade_target(&set_all, &set_src).ok_or(AppError::NoAvailableNode)?;
    let server_dst = server_ds0.clone();

    let mode = state.config.cascade.mode.clone();
    let public = state.config.http.public.clone();
    let client = state.client.clone();

    info!(
        "cascade mode: {:?}, from: {:?}, to: {:?}",
        mode, server_src, server_dst
    );

    tokio::spawn(async move {
        let cascade_result = match mode {
            CascadeMode::Push => {
                cascade_push(
                    public,
                    client.clone(),
                    server_src.clone(),
                    server_dst.clone(),
                    stream.clone(),
                )
                .await
            }
            CascadeMode::Pull => {
                cascade_pull(
                    state.client.clone(),
                    server_src.clone(),
                    server_dst.clone(),
                    stream.clone(),
                )
                .await
            }
        };
        match cascade_result {
            Ok(()) => {
                match force_check_times(
                    state.client.clone(),
                    server_dst.clone(),
                    stream.clone(),
                    state.config.cascade.check_attempts.0,
                )
                .await
                {
                    Ok(count) => {
                        if state.config.cascade.close_other_sub {
                            // Pull mode: the destination's outgoing pull
                            // arrives at the source as an unmarked, ordinary
                            // subscriber. Learn the hop's source-side session
                            // id from the destination's publish session
                            // (`cascade.session_url`) so the cleanup spares
                            // the hop itself. Push mode needs nothing: the
                            // source marks its own outgoing push session.
                            let known_hop = match mode {
                                CascadeMode::Pull => {
                                    hop_session_on_source(
                                        state.client.clone(),
                                        server_dst.clone(),
                                        &stream,
                                    )
                                    .await
                                }
                                CascadeMode::Push => None,
                            };
                            cascade_close_other_sub(state, server_src, stream, known_hop).await
                        }
                        info!("cascade {:?} success, checked attempts: {}", mode, count)
                    }
                    Err(e) => error!("cascade check error: {:?}", e),
                }
            }
            Err(e) => error!("cascade {:?} error: {:?}", mode, e),
        }
    });

    Ok(server_ds0)
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
}
