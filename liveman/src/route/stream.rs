use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::{
    Json,
    extract::{Path, State},
    response::{
        Response, Sse,
        sse::{Event, KeepAlive},
    },
};
// https://docs.rs/axum/latest/axum/extract/struct.Query.html
// For handling multiple values for the same query parameter, in a ?foo=1&foo=2&foo=3 fashion, use axum_extra::extract::Query instead.
use axum_extra::extract::Query;
use http::{StatusCode, header};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{trace, warn};

use api::response::Stream;

use crate::{AppState, error::AppError, result::Result};

use super::proxy::QueryExtract;

fn get_map_server_stream(map_info: HashMap<String, Vec<Stream>>) -> HashMap<String, Stream> {
    let mut map_server_stream = HashMap::new();
    for (alias, streams) in map_info.iter() {
        for stream in streams.iter() {
            map_server_stream.insert(format!("{}:{}", alias, stream.id), stream.clone());
        }
    }
    map_server_stream
}

/// Media statistics merge across nodes serving the same stream. The merged
/// value is node-level work across the cluster, not cluster edge traffic:
/// cascade hops are counted as work on each relay node.
fn merge_stats(a: &api::response::Stats, b: &api::response::Stats) -> api::response::Stats {
    api::response::Stats {
        bytes: a.bytes + b.bytes,
        packets: a.packets + b.packets,
        bitrate: a.bitrate + b.bitrate,
    }
}

pub async fn index(
    State(mut state): State<AppState>,
    Query(query_extract): Query<QueryExtract>,
) -> Result<Json<Vec<api::response::Stream>>> {
    Ok(Json(merged_streams(&mut state, &query_extract.nodes).await))
}

/// How long the SSE loop sleeps between storage reads when no change
/// notification arrived. The read itself drives the throttled lazy poll of
/// poll-mode nodes (`Storage::update`), so this cadence keeps poll-mode
/// snapshots flowing while a dashboard is connected; SSE-mode nodes push
/// their updates through the change watch as they arrive.
const SSE_IDLE_INTERVAL: Duration = Duration::from_secs(3);

/// The cluster-wide streams view behind `GET /api/streams/` and the SSE
/// stream: per-node snapshots merged by stream id, optionally restricted to
/// the given node aliases. The output order is deterministic (per-stream
/// node aliases merged in alias order, streams sorted by id, sessions by
/// id) so the SSE payload dedup compares a stable serialization.
async fn merged_streams(state: &mut AppState, nodes: &[String]) -> Vec<Stream> {
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let streams = state.storage.stream_all().await;
    let mut result_streams: HashMap<String, Stream> = HashMap::new();
    for (stream_id, mut servers) in streams.into_iter() {
        servers.sort();
        for server_alias in servers.iter() {
            if !nodes.is_empty() && !nodes.contains(server_alias) {
                continue;
            }
            let alias = format!("{server_alias}:{stream_id}");
            match map_server_stream.get(&alias) {
                Some(s) => {
                    let new_stream = match result_streams.get(&stream_id) {
                        Some(vv) => {
                            let v = vv.clone();
                            api::response::Stream {
                                id: s.id.clone(),
                                created_at: if s.created_at < v.created_at {
                                    s.created_at
                                } else {
                                    v.created_at
                                },
                                publish: api::response::PubSub {
                                    leave_at: {
                                        if s.publish.leave_at == 0 || v.publish.leave_at == 0 {
                                            0
                                        } else if s.publish.leave_at > v.publish.leave_at {
                                            s.publish.leave_at
                                        } else {
                                            v.publish.leave_at
                                        }
                                    },
                                    sessions: {
                                        let mut arr = s.publish.sessions.clone();
                                        arr.extend(v.publish.sessions);
                                        arr
                                    },
                                },
                                subscribe: api::response::PubSub {
                                    leave_at: {
                                        if s.subscribe.leave_at == 0 || v.subscribe.leave_at == 0 {
                                            0
                                        } else if s.subscribe.leave_at > v.subscribe.leave_at {
                                            s.subscribe.leave_at
                                        } else {
                                            v.subscribe.leave_at
                                        }
                                    },
                                    sessions: {
                                        let mut arr = s.subscribe.sessions.clone();
                                        arr.extend(v.subscribe.sessions);
                                        arr
                                    },
                                },
                                codecs: vec![],
                                // Config flags: true if true on any node.
                                provisioned: s.provisioned || v.provisioned,
                                on_demand: s.on_demand || v.on_demand,
                                stats: api::response::StreamStats {
                                    publish: merge_stats(&s.stats.publish, &v.stats.publish),
                                    subscribe: merge_stats(&s.stats.subscribe, &v.stats.subscribe),
                                },
                                stats_scope: api::response::StatsScope::ClusterNodeWork,
                            }
                        }
                        None => {
                            let mut stream = s.clone();
                            stream.stats_scope = api::response::StatsScope::Node;
                            stream
                        }
                    };
                    result_streams.insert(stream_id.clone(), new_stream);
                }
                None => continue,
            }
        }
    }

    let mut result: Vec<Stream> = result_streams.into_values().collect();
    result.sort_by(|a, b| a.id.cmp(&b.id));
    for stream in &mut result {
        stream.publish.sessions.sort_by(|a, b| a.id.cmp(&b.id));
        stream.subscribe.sessions.sort_by(|a, b| a.id.cmp(&b.id));
    }
    result
}

/// Browser-facing counterpart of liveion's `/api/sse/streams`: pushes the
/// merged cluster view (the same payload `index` serves) to dashboards.
/// Sends an initial snapshot immediately, then re-reads storage on every
/// change notification and on the idle cadence; identical consecutive
/// payloads are suppressed like liveion's.
pub async fn sse(
    State(mut state): State<AppState>,
    Query(query_extract): Query<QueryExtract>,
) -> Result<Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>>> {
    let (send, recv) = tokio::sync::mpsc::channel(16);
    let mut change_recv = state.storage.change_subscribe();
    let cancel = state.cancel.clone();
    tokio::spawn(async move {
        let mut last_payload: Option<String> = None;

        async fn send_snapshot(
            state: &mut AppState,
            nodes: &[String],
            last_payload: &mut Option<String>,
            send: &tokio::sync::mpsc::Sender<Vec<Stream>>,
        ) -> bool {
            let streams = merged_streams(state, nodes).await;
            let Ok(payload) = serde_json::to_string(&streams) else {
                // Plain-data structs cannot realistically fail to serialize;
                // keep the stream alive if they ever do.
                return true;
            };
            if last_payload.as_deref() == Some(payload.as_str()) {
                return true;
            }
            trace!("sse send merged snapshot with {} streams", streams.len());
            *last_payload = Some(payload);
            send.send(streams).await.is_ok()
        }

        // Send an initial snapshot so the client has current state immediately.
        if !send_snapshot(&mut state, &query_extract.nodes, &mut last_payload, &send).await {
            return;
        }

        loop {
            tokio::select! {
                // Storage mutation: `watch` is level-triggered, so a burst
                // of snapshot applies coalesces into one wakeup, and a
                // closed channel ends the loop.
                result = change_recv.changed() => {
                    if result.is_err() {
                        break;
                    }
                }
                _ = tokio::time::sleep(SSE_IDLE_INTERVAL) => {}
                // End the stream on shutdown: a never-ending response
                // would otherwise hold graceful shutdown open.
                _ = cancel.cancelled() => break,
            }
            if !send_snapshot(&mut state, &query_extract.nodes, &mut last_payload, &send).await {
                break;
            }
        }
    });
    let stream =
        ReceiverStream::new(recv).map(|streams| Ok(Event::default().json_data(streams).unwrap()));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

pub async fn show(
    State(mut state): State<AppState>,
    Path(stream_id): Path<String>,
) -> Result<Json<HashMap<String, api::response::Stream>>> {
    let mut result_streams: HashMap<String, Stream> = HashMap::new();
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let servers = state.storage.get_cluster();
    for server in servers.into_iter() {
        if let Some(stream) = map_server_stream.get(&format!("{}:{}", server.alias, stream_id)) {
            result_streams.insert(server.alias, stream.clone());
        }
    }

    Ok(Json(result_streams))
}

pub async fn create(
    State(mut state): State<AppState>,
    Path(stream_id): Path<String>,
    Query(query_extract): Query<QueryExtract>,
) -> crate::result::Result<Response<String>> {
    let mut has = false;
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let servers = state.storage.get_cluster();

    let server = if !query_extract.nodes.is_empty() {
        servers
            .iter()
            .find(|s| query_extract.nodes.contains(&s.alias))
            .ok_or(AppError::NoAvailableNode)?
            .clone()
    } else {
        servers.first().ok_or(AppError::NoAvailableNode)?.clone()
    };

    for srv in servers.iter() {
        if let Some(stream) = map_server_stream.get(&format!("{}:{}", srv.alias, stream_id)) {
            warn!("stream: {:?} already exists", stream);
            has = true;
            break;
        }
    }

    if has {
        Err(AppError::ResourceAlreadyExists)
    } else {
        let client = reqwest::Client::new();
        client
            .post(format!("{}{}", server.url, api::path::streams(&stream_id)))
            .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
            .send()
            .await?;

        Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body("".to_string())?)
    }
}

pub async fn destroy(
    State(mut state): State<AppState>,
    Path(stream_id): Path<String>,
) -> crate::result::Result<Response<String>> {
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let servers = state.storage.get_cluster();
    for server in servers.into_iter() {
        if let Some(stream) = map_server_stream.get(&format!("{}:{}", server.alias, stream_id)) {
            let client = reqwest::Client::new();
            client
                .delete(format!("{}{}", server.url, api::path::streams(&stream.id)))
                .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                .send()
                .await?;
        }
    }

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}
