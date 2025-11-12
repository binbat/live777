use std::{collections::HashMap, time::Duration};

use chrono::Utc;
use glob::Pattern;
use http::header;
use tracing::{error, info};
use url::Url;

use crate::service::recordings_index::RecordingsIndexService;
use crate::{AppState, error::AppError, result::Result, route::utils::session_delete};

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
                if let Some(cascade_info) = &session_info.cascade
                    && let Ok((target_node_addr, target_stream)) =
                        parse_node_and_stream(cascade_info.target_url.clone().unwrap())
                    && let Some(target_node) = map_url_server.get(&target_node_addr)
                    && let Some(target_stream_info) = nodes
                        .get(&target_node.alias)
                        .unwrap()
                        .iter()
                        .find(|i| i.id == target_stream)
                    && target_stream_info.subscribe.leave_at != 0
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

fn parse_node_and_stream(url: String) -> Result<(String, String)> {
    let url = Url::parse(&url)?;
    let split: Vec<&str> = url.path().split('/').collect();
    Ok((
        format!(
            "{}://{}:{}",
            url.scheme(),
            url.host_str()
                .ok_or(AppError::InternalServerError(anyhow::anyhow!("host error")))?,
            url.port()
                .ok_or(AppError::InternalServerError(anyhow::anyhow!("port error")))?
        ),
        split
            .last()
            .cloned()
            .ok_or(AppError::InternalServerError(anyhow::anyhow!(
                "url path split error"
            )))?
            .to_string(),
    ))
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

                            let mut record_ts = requested_ts.clone();
                            let mut mpd_path = fallback_mpd_path;

                            if let Ok(v) = r.json::<api::recorder::StartRecordResponse>().await {
                                if !v.mpd_path.is_empty() {
                                    mpd_path = v.mpd_path;
                                }
                                if !v.record_id.is_empty() {
                                    record_ts = v.record_id;
                                } else if !v.record_dir.is_empty()
                                    && let Some(ts) =
                                        crate::utils::extract_timestamp_from_record_dir(
                                            &v.record_dir,
                                        )
                                {
                                    record_ts = ts;
                                }
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

                let mut record_ts = requested_ts.clone();
                let mut mpd_path = fallback_mpd_path;

                if let Ok(v) = r.json::<api::recorder::StartRecordResponse>().await {
                    if !v.mpd_path.is_empty() {
                        mpd_path = v.mpd_path;
                    }
                    if !v.record_id.is_empty() {
                        record_ts = v.record_id;
                    } else if !v.record_dir.is_empty()
                        && let Some(ts) =
                            crate::utils::extract_timestamp_from_record_dir(&v.record_dir)
                    {
                        record_ts = ts;
                    }
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
