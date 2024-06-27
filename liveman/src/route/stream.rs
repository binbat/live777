use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    Json,
};

use api::response::Stream;

use crate::{result::Result, AppState};

pub async fn index(State(mut state): State<AppState>) -> Result<Json<Vec<api::response::Stream>>> {
    let map_info = state.storage.info_raw_all().await.unwrap();
    let mut map_server_stream = HashMap::new();
    for (alias, streams) in map_info.iter() {
        for stream in streams.iter() {
            map_server_stream.insert(format!("{}:{}", alias, stream.id), stream.clone());
        }
    }

    let streams = state.storage.stream_all().await;
    let mut result_streams: HashMap<String, Stream> = HashMap::new();
    for (stream_id, servers) in streams.into_iter() {
        for server in servers.iter() {
            let alias = format!("{}:{}", server.alias, stream_id);
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
    let map_info = state.storage.info_raw_all().await.unwrap();
    let mut map_server_stream = HashMap::new();
    for (alias, streams) in map_info.iter() {
        for stream in streams.iter() {
            map_server_stream.insert(format!("{}:{}", alias, stream.id), stream.clone());
        }
    }

    let servers = state.storage.get_cluster();
    for server in servers.into_iter() {
        if let Some(stream) = map_server_stream.get(&format!("{}:{}", server.alias, stream_id)) {
            result_streams.insert(server.alias, stream.clone());
        }
    }

    Ok(Json(result_streams))
}
