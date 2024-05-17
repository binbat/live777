use crate::model::{Node, Stream};
use crate::result::Result;
use axum::routing::post;
use axum::Router;
use axum::{extract::State, Json};
use axum_extra::extract::Query;
use live777_http::event::Event;
use serde::{Deserialize, Serialize};

pub fn route() -> Router<AppState> {
    Router::new().route("/webhook", post(webhook))
}

use crate::AppState;
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct WebHookQuery {
    token: String,
    reforward_maximum_idle_time: Option<u64>,
    reforward_cascade: Option<bool>,
}

impl WebHookQuery {
    fn get_reforward_maximum_idle_time(&self) -> u64 {
        if let Some(reforward_maximum_idle_time) = self.reforward_maximum_idle_time {
            reforward_maximum_idle_time
        } else {
            0
        }
    }
    fn get_reforward_cascade(&self) -> bool {
        if let Some(reforward_cascade) = self.reforward_cascade {
            reforward_cascade
        } else {
            false
        }
    }
}

async fn webhook(
    State(state): State<AppState>,
    Query(qry): Query<WebHookQuery>,
    Json(event_body): Json<live777_http::event::EventBody>,
) -> Result<String> {
    let pool = &state.pool;
    let addr = event_body.addr;
    let metrics = event_body.metrics;
    let mut node = Node {
        addr: addr.to_string(),
        stream: metrics.stream,
        publish: metrics.publish,
        subscribe: metrics.subscribe,
        reforward: metrics.reforward,
        reforward_maximum_idle_time: qry.get_reforward_maximum_idle_time(),
        reforward_cascade: qry.get_reforward_cascade(),
        ..Default::default()
    };
    match event_body.event {
        Event::Node { r#type, metadata } => {
            node.authorization = metadata.authorization;
            node.admin_authorization = metadata.admin_authorization;
            node.pub_max = metadata.pub_max;
            node.sub_max = metadata.sub_max;
            match r#type {
                live777_http::event::NodeEventType::Up => node.db_save_or_update(pool).await?,
                live777_http::event::NodeEventType::Down => {
                    node.db_remove(pool).await?;
                    Stream::db_remove_addr_stream(pool, addr.to_string()).await?
                }
                live777_http::event::NodeEventType::KeepAlive => {
                    if node.db_update_metrics(pool).await.is_err() {
                        node.db_save_or_update(pool).await?;
                    }
                }
            }
        }
        Event::Stream { r#type, stream } => {
            let _ = node.db_update_metrics(pool).await;
            let db_stream = Stream {
                stream: stream.stream,
                addr: addr.to_string(),
                publish: stream.publish,
                subscribe: stream.subscribe,
                reforward: stream.reforward,
                ..Default::default()
            };
            match r#type {
                live777_http::event::StreamEventType::StreamUp => {
                    db_stream.db_save_or_update(pool).await?
                }
                live777_http::event::StreamEventType::StreamDown => {
                    db_stream.db_remove(pool).await?
                }
                _ => {
                    db_stream.db_update_metrics(pool).await?;
                }
            }
        }
    }
    Ok("".to_string())
}
