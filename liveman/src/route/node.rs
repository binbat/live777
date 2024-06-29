use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

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
    pub alias: String,
    pub url: String,
    pub pub_max: u16,
    pub sub_max: u16,
    pub status: NodeState,
}

pub async fn index(State(mut state): State<AppState>) -> Result<Json<Vec<Node>>> {
    let map_info = state.storage.info_raw_all().await.unwrap();

    Ok(Json(
        state
            .storage
            .get_cluster()
            .into_iter()
            .map(|x| Node {
                alias: x.alias.clone(),
                url: x.url,
                pub_max: x.pub_max,
                sub_max: x.sub_max,
                status: match map_info.get(&x.alias) {
                    Some(_) => NodeState::Running,
                    None => NodeState::Stopped,
                },
            })
            .collect(),
    ))
}
