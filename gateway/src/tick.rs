use std::{collections::HashMap, time::Duration};

use crate::{error::AppError, result::Result};
use chrono::Utc;
use tracing::info;
use url::Url;

use crate::AppState;

pub async fn reforward_check(state: AppState) {
    loop {
        let timeout = tokio::time::sleep(Duration::from_millis(
            state.config.reforward.check_reforward_tick_time.0,
        ));
        tokio::pin!(timeout);
        let _ = timeout.as_mut().await;
        let _ = do_reforward_check(state.clone()).await;
    }
}

async fn do_reforward_check(state: AppState) -> Result<()> {
    let nodes = state.storage.nodes().await?;
    if nodes.is_empty() {
        return Ok(());
    }
    let mut node_map = HashMap::new();
    let mut node_streams_map = HashMap::new();
    for node in nodes.iter() {
        node_map.insert(node.addr.clone(), node.clone());
        if let Ok(streams) = node.stream_infos(vec![]).await {
            node_streams_map.insert(node.addr.clone(), streams);
        }
    }
    for (node_addr, streams) in node_streams_map.iter() {
        let node = node_map.get(node_addr).unwrap();
        for stream_info in streams {
            for session_info in &stream_info.subscribe_session_infos {
                if let Some(reforward_info) = &session_info.reforward {
                    if let Ok((target_node_addr, target_stream)) =
                        parse_node_and_stream(reforward_info.target_url.clone())
                    {
                        if let Some(target_node) = node_map.get(&target_node_addr) {
                            if let Ok(Some(target_stream_info)) =
                                target_node.stream_info(target_stream).await
                            {
                                if target_stream_info.subscribe_leave_time != 0
                                    && Utc::now().timestamp_millis()
                                        >= target_stream_info.subscribe_leave_time
                                            + node.metadata.stream_info.reforward_maximum_idle_time
                                                as i64
                                {
                                    info!(
                                        ?node,
                                        stream_info.id,
                                        session_info.id,
                                        ?target_stream_info,
                                        "reforward idle for long periods of time"
                                    );
                                    let _ = node
                                        .resource_delete(
                                            stream_info.id.clone(),
                                            session_info.id.clone(),
                                        )
                                        .await;
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
            "{}:{}",
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
