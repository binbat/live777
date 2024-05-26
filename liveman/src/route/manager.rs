use crate::AppState;
use crate::{model::Node, result::Result};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::QueryBuilder;
use sqlx::{MySql, MySqlPool};

pub fn route() -> Router<AppState> {
    Router::new()
        .route("/manager/nodes", get(nodes))
        .route("/manager/nodes/:addr/detail", get(node_detail))
        .route("/manager/streams", get(streams))
        .route(
            "/manager/streams/:stream/:addr/detail",
            get(stream_node_detail),
        )
}

async fn nodes(
    State(state): State<AppState>,
    Valid(Query(qry)): Valid<Query<NodeQuery>>,
) -> Result<Json<Page<Node>>> {
    let count = Node::db_query_count(&qry, &state.pool).await?;
    let mut page = Page::new(qry.page_no, qry.page_size, count);
    if page.has_next_data() {
        page.data = Node::db_query(&qry, &state.pool).await?;
    }
    Ok(Json(page))
}

async fn node_detail(
    State(state): State<AppState>,
    Path(addr): Path<String>,
) -> Result<Json<Vec<StreamInfo>>> {
    let addr = Node::db_find_by_addr(&state.pool, addr)
        .await?
        .ok_or_else(|| AppError::ResourceNotFound)?;
    let stream_infos = addr.stream_infos(vec![]).await?;
    Ok(Json(stream_infos))
}

async fn streams(
    State(state): State<AppState>,
    Valid(Query(qry)): Valid<Query<StreamQuery>>,
) -> Result<Json<Page<Stream>>> {
    let count = Stream::db_query_count(&qry, &state.pool).await?;
    let mut page = Page::new(qry.page_no, qry.page_size, count);
    if page.has_next_data() {
        page.data = Stream::db_query(&qry, &state.pool).await?;
    }
    Ok(Json(page))
}

async fn stream_node_detail(
    State(state): State<AppState>,
    Path((stream, addr)): Path<(String, String)>,
) -> Result<Json<Option<StreamInfo>>> {
    let addr = Node::db_find_by_addr(&state.pool, addr)
        .await?
        .ok_or_else(|| AppError::ResourceNotFound)?;
    let stream_info = addr.stream_infos(vec![stream]).await?.pop();
    Ok(Json(stream_info))
}

use crate::error::AppError;
use crate::model::Stream;
use crate::route::Page;
use axum::extract::{Path, Query, State};
use axum_valid::Valid;
use live777_http::response::StreamInfo;
use validator::Validate;

#[derive(Serialize, Deserialize, Validate, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodeQuery {
    #[validate(range(min = 1))]
    page_no: u64,
    #[validate(range(min = 1, max = 50))]
    page_size: u64,
    addr: Option<String>,
    state: Option<NodeState>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub enum NodeState {
    Active,
    Inactive,
    Deactivated,
}

#[derive(Serialize, Deserialize, Validate, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuery {
    #[validate(range(min = 1))]
    page_no: u64,
    #[validate(range(min = 1, max = 50))]
    page_size: u64,
    addr: Option<String>,
    stream: Option<String>,
}

impl Node {
    async fn db_query_count(node_query: &NodeQuery, pool: &MySqlPool) -> Result<u64> {
        let mut query_build = QueryBuilder::new("select count(*) from nodes where 1 = 1 ");
        Self::db_query_where(&mut query_build, node_query, false);
        let count: (i64,) = query_build.build_query_as().fetch_one(pool).await?;
        Ok(count.0 as u64)
    }

    async fn db_query(node_query: &NodeQuery, pool: &MySqlPool) -> Result<Vec<Node>> {
        let mut query_build = QueryBuilder::new("select * from nodes where 1 = 1 ");
        Self::db_query_where(&mut query_build, node_query, true);
        let nodes = query_build.build_query_as().fetch_all(pool).await?;
        Ok(nodes)
    }

    fn db_query_where(query_build: &mut QueryBuilder<MySql>, node_query: &NodeQuery, limit: bool) {
        if let Some(addr) = &node_query.addr {
            query_build.push(" and addr = ").push_bind(addr.clone());
        }
        if let Some(state) = &node_query.state {
            match state {
                NodeState::Active => query_build
                    .push(" and updated_at >= ")
                    .push_bind(Node::active_time_point()),
                NodeState::Inactive => query_build
                    .push(" and updated_at > ")
                    .push_bind(Node::deactivate_time())
                    .push(" and updated_at < ")
                    .push_bind(Node::active_time_point()),
                NodeState::Deactivated => query_build
                    .push(" and updated_at = ")
                    .push_bind(Node::deactivate_time()),
            };
        }
        if limit {
            query_build
                .push(" limit ")
                .push_bind((node_query.page_no - 1) * node_query.page_size)
                .push(" , ")
                .push_bind(node_query.page_size);
        }
    }
}

impl Stream {
    async fn db_query_count(node_query: &StreamQuery, pool: &MySqlPool) -> Result<u64> {
        let mut query_build = QueryBuilder::new("select count(*) from streams where 1 = 1 ");
        Self::db_query_where(&mut query_build, node_query, false);
        let count: (i64,) = query_build.build_query_as().fetch_one(pool).await?;
        Ok(count.0 as u64)
    }

    async fn db_query(node_query: &StreamQuery, pool: &MySqlPool) -> Result<Vec<Stream>> {
        let mut query_build = QueryBuilder::new("select * from streams where 1 = 1 ");
        Self::db_query_where(&mut query_build, node_query, true);
        let nodes = query_build.build_query_as().fetch_all(pool).await?;
        Ok(nodes)
    }

    fn db_query_where(
        query_build: &mut QueryBuilder<MySql>,
        node_query: &StreamQuery,
        limit: bool,
    ) {
        if let Some(addr) = &node_query.addr {
            query_build.push(" and addr = ").push_bind(addr.clone());
        }
        if let Some(stream) = &node_query.stream {
            query_build.push(" and stream = ").push_bind(stream.clone());
        }
        if limit {
            query_build
                .push(" limit ")
                .push_bind((node_query.page_no - 1) * node_query.page_size)
                .push(" , ")
                .push_bind(node_query.page_size);
        }
    }
}
