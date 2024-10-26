use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use api::strategy::Strategy;

use crate::{result::Result, AppState};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    strategy: Option<Strategy>,
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
                status: match node.strategy {
                    Some(_) => NodeState::Running,
                    None => NodeState::Stopped,
                },
                strategy: node.strategy,
                duration: match node.duration {
                    Some(s) => format!("{}ms", s.as_millis()),
                    None => "-".to_string(),
                },
            })
            .collect(),
    ))
}
