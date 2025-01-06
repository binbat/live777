use std::{collections::HashMap, time::Duration};

use chrono::Utc;
use tracing::{error, info};
use url::Url;

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
