use std::{collections::HashMap, net::SocketAddr, time::Duration, vec};

use crate::{
    error::AppError,
    model::{Node, Stream},
    result::Result,
};
use anyhow::anyhow;
use chrono::Utc;
use futures_util::StreamExt;
use live777_http::response::StreamInfo;
use sqlx::{MySql, Pool};
use tracing::info;
use url::Url;

use crate::AppState;

pub async fn run(state: AppState) {
    tokio::spawn(node_sync(state.clone()));
    tokio::spawn(reforward_check(state.clone()));
}

pub async fn node_sync(state: AppState) {
    loop {
        let _ = do_node_sync(state.clone()).await;
        tokio::time::sleep(Duration::from_millis(state.config.node_sync_tick_time.0)).await;
    }
}

pub async fn do_node_sync(state: AppState) -> Result<()> {
    let active_time_point = Node::active_time_point();
    let nodes: Vec<Node> = Node::db_find_not_deactivate_nodes(&state.pool).await?;
    let mut active_nodes = vec![];
    let mut inactivate_nodes = vec![];
    for node in nodes {
        if node.updated_at >= active_time_point {
            active_nodes.push(node);
        } else {
            inactivate_nodes.push(node);
        }
    }
    //inactivate_nodes remove
    futures_util::stream::iter(inactivate_nodes)
        .for_each_concurrent(None, |node| async {
            let _ = node.db_remove(&state.pool).await;
            let _ = Stream::db_remove_addr_stream(&state.pool, node.addr).await;
        })
        .await;
    //active_nodes  sync info
    futures_util::stream::iter(active_nodes)
        .for_each_concurrent(None, |node| async {
            let _ = node_sync_info(node, &state.pool).await;
        })
        .await;
    Ok(())
}

impl From<StreamInfo> for Stream {
    fn from(value: StreamInfo) -> Self {
        Stream {
            stream: value.id,
            publish: if value.publish_session_info.is_some() {
                1
            } else {
                0
            },
            subscribe: value.subscribe_session_infos.len() as u64,
            reforward: value
                .subscribe_session_infos
                .iter()
                .filter(|session| session.reforward.is_some())
                .count() as u64,
            ..Default::default()
        }
    }
}

pub async fn node_sync_info(node: Node, pool: &Pool<MySql>) -> Result<()> {
    let current_stream_map: &HashMap<String, Stream> = &node
        .stream_infos(vec![])
        .await?
        .into_iter()
        .map(|stream_info| {
            let mut stream = Stream::from(stream_info);
            stream.addr = node.addr.clone();
            (stream.stream.clone(), stream.clone())
        })
        .collect();
    let stream_map: &HashMap<String, Stream> =
        &Stream::db_find_node_stream(pool, node.addr.clone())
            .await?
            .into_iter()
            .map(|stream| (stream.stream.clone(), stream.clone()))
            .collect();
    // delete
    futures_util::stream::iter(stream_map)
        .for_each_concurrent(None, |(stream_id, stream)| async move {
            if !current_stream_map.contains_key(stream_id) {
                let _ = stream.db_remove(pool).await;
            }
        })
        .await;
    // save or update
    futures_util::stream::iter(current_stream_map)
        .for_each_concurrent(None, |(stream_id, current_stream)| async move {
            if let Some(stream) = stream_map.get(stream_id) {
                if stream.publish != current_stream.publish
                    || stream.subscribe != current_stream.subscribe
                    || stream.reforward != current_stream.reforward
                {
                    let _ = current_stream.db_update_metrics(pool).await;
                }
            } else {
                let _ = current_stream.db_save_or_update(pool).await;
            }
        })
        .await;
    Ok(())
}

pub async fn reforward_check(state: AppState) {
    loop {
        let _ = do_reforward_check(state.clone()).await;
        tokio::time::sleep(Duration::from_millis(
            state.config.reforward.check_tick_time.0,
        ))
        .await;
    }
}

async fn do_reforward_check(state: AppState) -> Result<()> {
    let reforward_nodes = Node::db_find_reforward_nodes(&state.pool).await?;
    if reforward_nodes.is_empty() {
        return Ok(());
    }
    futures_util::stream::iter(reforward_nodes)
        .for_each_concurrent(None, |node| async {
            let _ = node_reforward_check(node, &state.pool).await;
        })
        .await;
    Ok(())
}

async fn node_reforward_check(node: Node, pool: &Pool<MySql>) -> Result<()> {
    let streams = node.stream_infos(vec![]).await?;
    if streams.is_empty() {
        return Ok(());
    }
    futures_util::stream::iter(streams)
        .for_each_concurrent(None, |stream_info| async {
            let _ = node_stream_reforward_check(node.clone(), pool, stream_info).await;
        })
        .await;
    Ok(())
}

async fn node_stream_reforward_check(
    node: Node,
    pool: &Pool<MySql>,
    stream_info: StreamInfo,
) -> Result<()> {
    for session_info in &stream_info.subscribe_session_infos {
        if let Some(reforward_info) = &session_info.reforward {
            if let Ok((target_addr, target_stream)) =
                parse_node_and_stream(reforward_info.target_url.clone())
            {
                let target_node = Node::db_find_by_addr(pool, target_addr.to_string()).await;
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
