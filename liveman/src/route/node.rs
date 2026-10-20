use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{AppState, result::Result};

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    #[default]
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopped")]
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    alias: String,
    url: String,
    status: NodeState,
    duration: String,
    /// The node's own `GET /api/info` (version/git hash/build
    /// time/features), cached by `tick::node_info_check`; absent until the
    /// first successful fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    info: Option<api::response::ServerInfo>,
}

pub async fn index(State(mut state): State<AppState>) -> Result<Json<Vec<Node>>> {
    state.storage.nodes().await;
    Ok(Json(
        state
            .storage
            .get_map_nodes()
            .into_iter()
            .map(|(alias, node)| Node {
                alias,
                url: node.url,
                status: match node.online {
                    true => NodeState::Running,
                    false => NodeState::Stopped,
                },
                duration: match node.duration {
                    Some(s) => format!("{}ms", s.as_millis()),
                    None => "-".to_string(),
                },
                info: node.info,
            })
            .collect(),
    ))
}
