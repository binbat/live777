use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    response::Response,
    Json,
};
use axum_extra::extract::Query;
use http::{header, StatusCode};
use tracing::warn;

use api::response::Stream;

use crate::{error::AppError, result::Result, AppState};

use super::proxy::QueryExtract;

fn get_map_server_stream(map_info: HashMap<String, Vec<Stream>>) -> HashMap<String, Stream> {
    let mut map_server_stream = HashMap::new();
    for (alias, streams) in map_info.iter() {
        for stream in streams.iter() {
            map_server_stream.insert(format!("{}:{}", alias, stream.id), stream.clone());
        }
    }
    map_server_stream
}

pub async fn index(
    State(mut state): State<AppState>,
    Query(query_extract): Query<QueryExtract>,
) -> Result<Json<Vec<api::response::Stream>>> {
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let streams = state.storage.stream_all().await;
    let mut result_streams: HashMap<String, Stream> = HashMap::new();
    for (stream_id, servers) in streams.into_iter() {
        for server_alias in servers.iter() {
            if !query_extract.nodes.is_empty() && !query_extract.nodes.contains(server_alias) {
                continue;
            }
            let alias = format!("{}:{}", server_alias, stream_id);
            match map_server_stream.get(&alias) {
                Some(s) => {
                    let new_stream = match result_streams.get(&stream_id) {
                        Some(vv) => {
                            let v = vv.clone();
                            api::response::Stream {
                                id: s.id.clone(),
                                created_at: if s.created_at < v.created_at {
                                    s.created_at
                                } else {
                                    v.created_at
                                },
                                publish: api::response::PubSub {
                                    leave_at: {
                                        if s.publish.leave_at == 0 || v.publish.leave_at == 0 {
                                            0
                                        } else if s.publish.leave_at > v.publish.leave_at {
                                            s.publish.leave_at
                                        } else {
                                            v.publish.leave_at
                                        }
                                    },
                                    sessions: {
                                        let mut arr = s.publish.sessions.clone();
                                        arr.extend(v.publish.sessions);
                                        arr
                                    },
                                },
                                subscribe: api::response::PubSub {
                                    leave_at: {
                                        if s.subscribe.leave_at == 0 || v.subscribe.leave_at == 0 {
                                            0
                                        } else if s.subscribe.leave_at > v.subscribe.leave_at {
                                            s.subscribe.leave_at
                                        } else {
                                            v.subscribe.leave_at
                                        }
                                    },
                                    sessions: {
                                        let mut arr = s.subscribe.sessions.clone();
                                        arr.extend(v.subscribe.sessions);
                                        arr
                                    },
                                },
                                codecs: vec![],
                            }
                        }
                        None => s.clone(),
                    };
                    result_streams.insert(stream_id.clone(), new_stream);
                }
                None => continue,
            }
        }
    }

    Ok(Json(
        result_streams
            .into_values()
            .collect::<Vec<api::response::Stream>>(),
    ))
}

pub async fn show(
    State(mut state): State<AppState>,
    Path(stream_id): Path<String>,
) -> Result<Json<HashMap<String, api::response::Stream>>> {
    let mut result_streams: HashMap<String, Stream> = HashMap::new();
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let servers = state.storage.get_cluster();
    for server in servers.into_iter() {
        if let Some(stream) = map_server_stream.get(&format!("{}:{}", server.alias, stream_id)) {
            result_streams.insert(server.alias, stream.clone());
        }
    }

    Ok(Json(result_streams))
}

pub async fn create(
    State(mut state): State<AppState>,
    Path(stream_id): Path<String>,
    Query(query_extract): Query<QueryExtract>,  
) -> crate::result::Result<Response<String>> {
    let mut has = false;
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let servers = state.storage.get_cluster();
    

    let server = if !query_extract.nodes.is_empty() {

        servers.iter()
            .find(|s| query_extract.nodes.contains(&s.alias))
            .ok_or(AppError::NoAvailableNode)?
            .clone()
    } else {

        servers.first()
            .ok_or(AppError::NoAvailableNode)?
            .clone()
    };

    for srv in servers.iter() {
        if let Some(stream) = map_server_stream.get(&format!("{}:{}", srv.alias, stream_id)) {
            warn!("stream: {:?} already exists", stream);
            has = true;
            break;
        }
    }

    if has {
        Err(AppError::ResourceAlreadyExists)
    } else {
        let client = reqwest::Client::new();
        client
            .post(format!("{}{}", server.url, api::path::streams(&stream_id)))
            .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
            .send()
            .await?;

        Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body("".to_string())?)
    }
}

pub async fn destroy(
    State(mut state): State<AppState>,
    Path(stream_id): Path<String>,
) -> crate::result::Result<Response<String>> {
    let map_server_stream = get_map_server_stream(state.storage.info_raw_all().await.unwrap());

    let servers = state.storage.get_cluster();
    for server in servers.into_iter() {
        if let Some(stream) = map_server_stream.get(&format!("{}:{}", server.alias, stream_id)) {
            let client = reqwest::Client::new();
            client
                .delete(format!("{}{}", server.url, api::path::streams(&stream.id)))
                .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                .send()
                .await?;
        }
    }

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}
