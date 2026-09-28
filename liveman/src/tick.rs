use std::{collections::HashMap, time::Duration};

use chrono::Utc;
use glob::Pattern;
use http::header;
use tracing::{error, info, warn};
use url::Url;

use crate::service::recordings_index::RecordingsIndexService;
use crate::store::Server;
use crate::{AppState, result::Result, route::utils::session_delete};

use api::recorder::{
    AckRecordingsRequest, DeleteRecordingsRequest, PullRecordingsRequest, RecordingKey,
};

pub async fn cascade_check(state: AppState) {
    loop {
        let timeout = tokio::time::sleep(Duration::from_millis(
            state.config.cascade.check_tick_time.0,
        ));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;
        let _ = do_cascade_check(state.clone()).await;
    }
}

async fn do_cascade_check(mut state: AppState) -> Result<()> {
    let servers = state.storage.nodes().await;

    let mut map_url_server = HashMap::new();
    for s in servers.clone() {
        map_url_server.insert(s.url.clone(), s.clone());
    }

    let map_server = state.storage.get_map_server();
    let nodes = state.storage.info_raw_all().await?;
    if nodes.is_empty() {
        return Ok(());
    }

    for (alias, streams) in nodes.iter() {
        let server = map_server.get(alias).unwrap();
        for stream_info in streams {
            for session_info in &stream_info.subscribe.sessions {
                let Some(cascade_info) = &session_info.cascade else {
                    continue;
                };
                let Some(target_url) = cascade_info.target_url.clone() else {
                    continue;
                };
                let Some((target_node, target_stream)) =
                    resolve_cascade_target(&target_url, &map_server, &map_url_server)
                else {
                    continue;
                };
                let Some(target_stream_info) = nodes
                    .get(&target_node.alias)
                    .and_then(|streams| streams.iter().find(|i| i.id == target_stream))
                else {
                    continue;
                };
                if target_stream_info.subscribe.leave_at != 0
                    && Utc::now().timestamp_millis()
                        >= target_stream_info.subscribe.leave_at
                            + state.config.cascade.maximum_idle_time as i64
                {
                    info!(
                        ?server,
                        stream_info.id,
                        session_info.id,
                        ?target_stream_info,
                        "cascade idle for long periods of time"
                    );
                    match session_delete(
                        state.client.clone(),
                        server.clone(),
                        stream_info.id.clone(),
                        session_info.id.clone(),
                    )
                    .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            error!("cascade session delete error: {:?}", e)
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Resolve a cascade `target_url` to its destination node and stream. Push
/// cascades point at liveman's node-pinned endpoint
/// (`{public}/api/whip/{alias}/{stream}`), so the node must be resolved by
/// alias; any other form is treated as a direct node address
/// (`{node}/whip/{stream}`) and resolved by origin.
fn resolve_cascade_target(
    target_url: &str,
    map_server: &HashMap<String, Server>,
    map_url_server: &HashMap<String, Server>,
) -> Option<(Server, String)> {
    let url = Url::parse(target_url).ok()?;
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        ["api", "whip", alias, stream] => map_server
            .get(*alias)
            .cloned()
            .map(|node| (node, stream.to_string())),
        ["whip", stream] => {
            let addr = format!("{}://{}:{}", url.scheme(), url.host_str()?, url.port()?);
            map_url_server
                .get(&addr)
                .cloned()
                .map(|node| (node, stream.to_string()))
        }
        _ => None,
    }
}

/// Liveman Auto Record Check
pub async fn auto_record_check(state: AppState) {
    if !state.config.auto_record.enabled {
        info!("auto_record is disabled, skip auto_record_check loop");
        return;
    }
    loop {
        let timeout = tokio::time::sleep(Duration::from_millis(state.config.auto_record.tick_ms));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;
        let _ = do_auto_record_check(state.clone()).await;
    }
}

async fn do_auto_record_check(mut state: AppState) -> Result<()> {
    let patterns = state.config.auto_record.auto_streams.clone();
    if patterns.is_empty() {
        return Ok(());
    }

    let streams = state.storage.stream_all().await;
    let base_prefix = state.config.auto_record.base_prefix.clone();

    for (stream_id, nodes) in streams.into_iter() {
        if !should_record(&patterns, &stream_id) {
            continue;
        }
        if let Some(first_node_alias) = nodes.first() {
            let node = state
                .storage
                .get_map_server()
                .get(first_node_alias)
                .cloned();
            if let Some(server) = node {
                let record_url = format!("{}{}", server.url, api::path::record(&stream_id));
                let is_recording = match state
                    .client
                    .get(record_url.as_str())
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if !resp.status().is_success() {
                            error!(
                                stream = %stream_id,
                                status = %resp.status(),
                                "record status request failed"
                            );
                            false
                        } else {
                            match resp.json::<serde_json::Value>().await {
                                Ok(v) => v
                                    .get("recording")
                                    .and_then(|b| b.as_bool())
                                    .unwrap_or(false),
                                Err(e) => {
                                    error!(stream = %stream_id, error = ?e, "parse record status failed");
                                    false
                                }
                            }
                        }
                    }
                    Err(_) => false,
                };

                if !is_recording {
                    let requested_ts = crate::utils::timestamp_dir();
                    let base_dir = if base_prefix.is_empty() {
                        None
                    } else {
                        Some(format!("{base_prefix}/{requested_ts}"))
                    };
                    let body = api::recorder::StartRecordRequest { base_dir };
                    let start_url = format!("{}{}", server.url, api::path::record(&stream_id));
                    let resp = state
                        .client
                        .post(start_url)
                        .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                        .json(&body)
                        .send()
                        .await;

                    if let Ok(r) = resp {
                        if r.status().is_success() {
                            // Prefer server-returned metadata, fallback to deterministic values
                            let fallback_mpd_path = if let Some(prefix) = &body.base_dir {
                                format!("{prefix}/manifest.mpd")
                            } else {
                                format!("{stream_id}/{requested_ts}/manifest.mpd")
                            };

                            let mut record_ts = String::new();
                            let mut mpd_path = fallback_mpd_path;

                            if let Ok(v) = r.json::<api::recorder::StartRecordResponse>().await {
                                if !v.mpd_path.is_empty() {
                                    mpd_path = v.mpd_path;
                                }
                                if !v.record_id.is_empty() {
                                    record_ts = v.record_id;
                                }
                            }

                            if record_ts.is_empty() {
                                error!(stream = %stream_id, "record_id is required from liveion");
                                continue;
                            }

                            if let Err(err) = RecordingsIndexService::upsert(
                                state.database.get_connection(),
                                &stream_id,
                                &record_ts,
                                &mpd_path,
                            )
                            .await
                            {
                                tracing::error!("{}", err);
                            }
                        } else {
                            let status = r.status();
                            let text = r.text().await.unwrap_or_default();
                            error!(
                                stream = %stream_id,
                                %status,
                                body = %text,
                                "record start failed"
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn should_record(patterns: &[String], stream: &str) -> bool {
    for p in patterns {
        if let Ok(pat) = Pattern::new(p)
            && pat.matches(stream)
        {
            return true;
        }
    }
    false
}

/// Rotate recordings when they exceed the configured max duration
pub async fn auto_record_rotate(state: AppState) {
    if !state.config.auto_record.enabled || state.config.auto_record.max_recording_seconds == 0 {
        return;
    }

    loop {
        let timeout = tokio::time::sleep(Duration::from_secs(
            state.config.auto_record.max_recording_seconds,
        ));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;

        let _ = do_auto_record_rotate(state.clone()).await;
    }
}

/// Pull recording index from liveion nodes and ack after syncing to DB
pub async fn record_sync(state: AppState) {
    if !state.config.record_sync.enabled {
        info!("record_sync is disabled, skip record_sync loop");
        return;
    }

    loop {
        let timeout = tokio::time::sleep(Duration::from_millis(state.config.record_sync.tick_ms));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;
        let _ = do_record_sync(state.clone()).await;
    }
}

async fn do_record_sync(mut state: AppState) -> Result<()> {
    let servers = state.storage.nodes().await;
    if servers.is_empty() {
        return Ok(());
    }

    for server in servers {
        let since_ts = {
            let guard = state.record_sync_cursor.read().await;
            guard.get(&server.alias).copied()
        };

        let req = PullRecordingsRequest {
            stream: None,
            since_ts,
            limit: state.config.record_sync.limit,
        };

        let url = format!("{}{}", server.url, api::path::recordings());
        let resp = match state
            .client
            .get(url)
            .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
            .query(&req)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                warn!(node = %server.alias, error = ?e, "record_sync pull failed");
                continue;
            }
        };

        if !resp.status().is_success() {
            warn!(
                node = %server.alias,
                status = %resp.status(),
                "record_sync pull failed"
            );
            continue;
        }

        let pull = match resp.json::<api::recorder::PullRecordingsResponse>().await {
            Ok(v) => v,
            Err(e) => {
                warn!(node = %server.alias, error = ?e, "record_sync parse failed");
                continue;
            }
        };

        if pull.sessions.is_empty() {
            if let Some(last_ts) = pull.last_ts {
                let mut guard = state.record_sync_cursor.write().await;
                guard.insert(server.alias.clone(), last_ts);
            }
            continue;
        }

        let mut ack_records: Vec<RecordingKey> = Vec::new();

        for session in pull.sessions.iter() {
            let record = if let Some(id) = session.id.as_ref()
                && !id.trim().is_empty()
            {
                id.clone()
            } else {
                warn!(
                    node = %server.alias,
                    stream = %session.stream,
                    mpd_path = %session.mpd_path,
                    "record_sync missing record id"
                );
                continue;
            };

            if let Err(err) = RecordingsIndexService::upsert(
                state.database.get_connection(),
                &session.stream,
                &record,
                &session.mpd_path,
            )
            .await
            {
                error!("{}", err);
                continue;
            }

            ack_records.push(RecordingKey {
                stream: session.stream.clone(),
                record,
            });
        }

        let mut should_advance = false;

        if ack_records.is_empty() {
            should_advance = pull.last_ts.is_some();
        } else {
            let ack_url = format!("{}{}", server.url, api::path::recordings_ack());
            let ack_req = AckRecordingsRequest {
                records: ack_records,
            };
            match state
                .client
                .patch(ack_url)
                .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                .json(&ack_req)
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    should_advance = true;
                }
                Ok(r) => {
                    warn!(
                        node = %server.alias,
                        status = %r.status(),
                        "record_sync ack failed"
                    );
                }
                Err(e) => {
                    warn!(node = %server.alias, error = ?e, "record_sync ack failed");
                }
            }

            if should_advance {
                let delete_url = format!("{}{}", server.url, api::path::recordings_delete());
                let delete_req = DeleteRecordingsRequest {
                    records: ack_req.records,
                };
                match state
                    .client
                    .delete(delete_url)
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .json(&delete_req)
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => {
                        warn!(
                            node = %server.alias,
                            status = %r.status(),
                            "record_sync delete failed"
                        );
                    }
                    Err(e) => {
                        warn!(node = %server.alias, error = ?e, "record_sync delete failed");
                    }
                }
            }
        }

        if should_advance && let Some(last_ts) = pull.last_ts {
            let mut guard = state.record_sync_cursor.write().await;
            guard.insert(server.alias.clone(), last_ts);
        }
    }

    Ok(())
}

async fn do_auto_record_rotate(mut state: AppState) -> Result<()> {
    let patterns = state.config.auto_record.auto_streams.clone();
    if patterns.is_empty() {
        return Ok(());
    }

    let streams = state.storage.stream_all().await; // HashMap<stream, Vec<alias>>
    let base_prefix = state.config.auto_record.base_prefix.clone();
    let map_server = state.storage.get_map_server();

    // Build new timestamp-based prefix for the next recording window
    let requested_ts = crate::utils::timestamp_dir();
    let base_dir = if base_prefix.is_empty() {
        None
    } else {
        Some(format!("{base_prefix}/{requested_ts}"))
    };

    for (stream_id, aliases) in streams.iter() {
        if !should_record(&patterns, stream_id) {
            continue;
        }

        // Stop recording on all nodes where it's active
        for alias in aliases {
            if let Some(server) = map_server.get(alias) {
                let record_url = format!("{}{}", server.url, api::path::record(stream_id));
                let is_recording = match state
                    .client
                    .get(record_url.as_str())
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(v) => v
                            .get("recording")
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false),
                        Err(_) => false,
                    },
                    Err(_) => false,
                };

                if is_recording {
                    let _ = state
                        .client
                        .delete(record_url)
                        .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                        .send()
                        .await;
                }
            }
        }

        // Start new recording on the preferred node (first alias or any available)
        let target_server = if let Some(first_alias) = aliases.first() {
            map_server.get(first_alias).cloned()
        } else {
            state.storage.get_cluster().first().cloned()
        };

        if let Some(server) = target_server {
            let url = format!("{}{}", server.url, api::path::record(stream_id));
            let body = api::recorder::StartRecordRequest {
                base_dir: base_dir.clone(),
            };
            let resp = state
                .client
                .post(url)
                .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                .json(&body)
                .send()
                .await;

            if let Ok(r) = resp
                && r.status().is_success()
            {
                let fallback_mpd_path = if let Some(prefix) = &body.base_dir {
                    format!("{prefix}/manifest.mpd")
                } else {
                    format!("{stream_id}/{requested_ts}/manifest.mpd")
                };

                let mut record_ts = String::new();
                let mut mpd_path = fallback_mpd_path;

                if let Ok(v) = r.json::<api::recorder::StartRecordResponse>().await {
                    if !v.mpd_path.is_empty() {
                        mpd_path = v.mpd_path;
                    }
                    if !v.record_id.is_empty() {
                        record_ts = v.record_id;
                    }
                }

                if record_ts.is_empty() {
                    error!(stream = %stream_id, "record_id is required from liveion");
                    continue;
                }

                // Upsert index
                if let Err(err) = RecordingsIndexService::upsert(
                    state.database.get_connection(),
                    stream_id,
                    &record_ts,
                    &mpd_path,
                )
                .await
                {
                    tracing::error!("{}", err);
                };
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(alias: &str, url: &str) -> Server {
        Server {
            alias: alias.to_string(),
            url: url.to_string(),
            ..Default::default()
        }
    }

    fn maps() -> (HashMap<String, Server>, HashMap<String, Server>) {
        let cloud = server("cloud", "http://127.0.0.1:7782");
        let map_server = HashMap::from([("cloud".to_string(), cloud.clone())]);
        let map_url_server = HashMap::from([(cloud.url.clone(), cloud)]);
        (map_server, map_url_server)
    }

    #[test]
    fn resolve_pinned_push_target_by_alias() {
        let (map_server, map_url_server) = maps();
        let (node, stream) = resolve_cascade_target(
            "http://127.0.0.1:8890/api/whip/cloud/cam0",
            &map_server,
            &map_url_server,
        )
        .unwrap();
        assert_eq!(node.alias, "cloud");
        assert_eq!(stream, "cam0");
    }

    #[test]
    fn resolve_direct_push_target_by_origin() {
        let (map_server, map_url_server) = maps();
        let (node, stream) = resolve_cascade_target(
            "http://127.0.0.1:7782/whip/cam0",
            &map_server,
            &map_url_server,
        )
        .unwrap();
        assert_eq!(node.alias, "cloud");
        assert_eq!(stream, "cam0");
    }

    #[test]
    fn resolve_unknown_target_returns_none() {
        let (map_server, map_url_server) = maps();
        assert!(
            resolve_cascade_target(
                "http://127.0.0.1:8890/api/whip/ghost/cam0",
                &map_server,
                &map_url_server,
            )
            .is_none()
        );
        assert!(
            resolve_cascade_target(
                "http://9.9.9.9:7782/whip/cam0",
                &map_server,
                &map_url_server,
            )
            .is_none()
        );
    }
}
