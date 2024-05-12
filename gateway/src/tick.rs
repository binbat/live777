use std::{net::SocketAddr, time::Duration};

use crate::{error::AppError, model::Node, result::Result};
use anyhow::anyhow;
use chrono::Utc;
use futures_util::StreamExt;
use live777_http::response::{RTCPeerConnectionState, StreamInfo};
use sqlx::{MySql, Pool};
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
    let reforward_nodes = Node::db_find_reforward_nodes(&state.pool).await?;
    if reforward_nodes.is_empty() {
        return Ok(());
    }
    futures_util::stream::iter(reforward_nodes)
        .for_each_concurrent(None, |node| async {
            let _ = node_reforward_check(node, state.pool.clone()).await;
        })
        .await;
    Ok(())
}

async fn node_reforward_check(node: Node, pool: Pool<MySql>) -> Result<()> {
    let streams = node.stream_infos(vec![]).await?;
    if streams.is_empty() {
        return Ok(());
    }
    futures_util::stream::iter(streams)
        .for_each_concurrent(None, |stream_info| async {
            let _ = node_stream_reforward_check(node.clone(), pool.clone(), stream_info).await;
        })
        .await;
    Ok(())
}

async fn node_stream_reforward_check(
    node: Node,
    pool: Pool<MySql>,
    stream_info: StreamInfo,
) -> Result<()> {
    for session_info in &stream_info.subscribe_session_infos {
        if let Some(reforward_info) = &session_info.reforward {
            if session_info.connect_state != RTCPeerConnectionState::Connected {
                if Utc::now().timestamp_millis()
                    >= session_info.create_time + node.reforward_maximum_idle_time as i64
                {
                    let _ = node
                        .resource_delete(stream_info.id.clone(), session_info.id.clone())
                        .await;
                    info!(
                        ?node,
                        ?session_info,
                        "reforward not connected for a long time"
                    )
                }
                continue;
            }
            if let Ok((target_addr, target_stream)) =
                parse_node_and_stream(reforward_info.target_url.clone())
            {
                let target_node = Node::db_find_by_addr(&pool, target_addr).await;
                if let Ok(Some(target_node)) = target_node {
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
                                .resource_delete(stream_info.id.clone(), session_info.id.clone())
                                .await;
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
