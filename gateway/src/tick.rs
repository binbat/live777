use std::{collections::HashMap, net::SocketAddr, time::Duration};

use crate::{error::AppError, model::Node, result::Result};
use anyhow::anyhow;
use chrono::Utc;
use tracing::info;
use url::Url;

use crate::AppState;

pub async fn reforward_check(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_millis(
            state.config.reforward.check_tick_time.0,
        ))
        .await;
        let _ = do_reforward_check(state.clone()).await;
    }
}

async fn do_reforward_check(state: AppState) -> Result<()> {
    let nodes = Node::nodes(&state.pool).await?;
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
    for (addr, streams) in node_streams_map.iter() {
        let node = node_map.get(addr).unwrap();
        for stream_info in streams {
            for session_info in &stream_info.subscribe_session_infos {
                if let Some(reforward_info) = &session_info.reforward {
                    if let Ok((target_addr, target_stream)) =
                        parse_node_and_stream(reforward_info.target_url.clone())
                    {
                        if let Some(target_node) = node_map.get(&target_addr.to_string()) {
                            if let Ok(Some(target_stream_info)) =
                                target_node.stream_info(target_stream).await
                            {
                                if target_stream_info.subscribe_leave_time != 0
                                    && Utc::now().timestamp_millis()
                                        >= target_stream_info.subscribe_leave_time
                                            + node.reforward_maximum_idle_time as i64
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

fn parse_node_and_stream(url: String) -> Result<(SocketAddr, String)> {
    let url = Url::parse(&url)?;
    let split: Vec<&str> = url.path().split('/').collect();
    let scheme = url.scheme();
    let addr = url
        .socket_addrs(move || Some(if scheme == "http" { 80 } else { 443 }))?
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("get socket addr error"))?;
    Ok((
        addr,
        split
            .last()
            .cloned()
            .ok_or(AppError::InternalServerError(anyhow::anyhow!(
                "url path split error"
            )))?
            .to_string(),
    ))
}
