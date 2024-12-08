use crate::result::Result;
use crate::AppState;
use axum::extract::{Path, Request, State};
use axum::routing::put;
use axum::Router;
use http::StatusCode;
use opendal::{services, Operator};
use tokio_stream::StreamExt;
use tracing::info;
pub fn route() -> Router<AppState> {
    Router::new().route(
        &api::path::record_file(":stream", ":dir", ":file"),
        put(record_file),
    )
}

async fn record_file(
    State(_state): State<AppState>,
    Path((stream, dir, file)): Path<(String, String, String)>,
    req: Request,
) -> Result<StatusCode> {
    info!("request : {:?}", req);
    let mut data = req.into_body().into_data_stream();
    let mut builder = services::Fs::default();
    builder = builder.root(&format!("./{}/{}", stream, dir));
    let op = Operator::new(builder)?.finish();
    let mut writer = op.writer(&file).await?;
    while let Some(Ok(data)) = data.next().await {
        writer.write(data).await?;
    }
    Ok(StatusCode::OK)
}
