use std::{collections::HashMap, time::Duration};

use chrono::Utc;
use glob::Pattern;
use http::header;
use tracing::{error, info};
use url::Url;

use crate::service::recordings_index::RecordingsIndexService;
use crate::{error::AppError, result::Result, route::utils::session_delete, AppState};

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
                if let Some(cascade_info) = &session_info.cascade {
                    if let Ok((target_node_addr, target_stream)) =
                        parse_node_and_stream(cascade_info.target_url.clone().unwrap())
                    {
                        if let Some(target_node) = map_url_server.get(&target_node_addr) {
                            if let Some(target_stream_info) = nodes
                                .get(&target_node.alias)
                                .unwrap()
                                .iter()
                                .find(|i| i.id == target_stream)
                            {
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
                let status_url = format!("{}{}", server.url, api::path::record_status(&stream_id));
                let is_recording = match state
                    .client
                    .get(status_url)
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

                if !is_recording {
                    let now = chrono::Utc::now();
                    let date_path = now.format("%Y/%m/%d").to_string();
                    let base_dir = if base_prefix.is_empty() {
                        None
                    } else {
                        Some(format!("{base_prefix}/{date_path}"))
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
                            // Prefer server-returned mpd_path, fallback to deterministic path
                            let mpd_path =
                                match r.json::<api::recorder::StartRecordResponse>().await {
                                    Ok(v) => v.mpd_path,
                                    Err(_) => {
                                        if let Some(prefix) = &body.base_dir {
                                            format!("{}/manifest.mpd", prefix)
                                        } else {
                                            format!("{}/{}/manifest.mpd", stream_id, date_path)
                                        }
                                    }
                                };

                            // extract yyyy/MM/dd from date_path
                            let parts: Vec<&str> = date_path.split('/').collect();
                            if parts.len() == 3 {
                                let year = parts[0].parse::<i32>().unwrap_or(0);
                                let month = parts[1].parse::<i32>().unwrap_or(0);
                                let day = parts[2].parse::<i32>().unwrap_or(0);
                                let _ = RecordingsIndexService::upsert(
                                    state.database.get_connection(),
                                    &stream_id,
                                    year,
                                    month,
                                    day,
                                    &mpd_path,
                                )
                                .await;
                            }
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
        if let Ok(pat) = Pattern::new(p) {
            if pat.matches(stream) {
                return true;
            }
        }
    }
    false
}
