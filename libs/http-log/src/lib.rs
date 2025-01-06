use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use tracing::{error, info, trace, warn};

pub async fn print_request_response(
    req: Request,
    next: Next,
) -> std::result::Result<impl IntoResponse, (StatusCode, String)> {
    let start = Instant::now();
    let method = req.method().clone();
    let uri = req.uri().clone();

    let req_headers = req.headers().clone();
    let (parts, body) = req.into_parts();
    let bytes = buffer_and_print("request", req_headers, body).await?;
    let req = Request::from_parts(parts, Body::from(bytes));

    let res = next.run(req).await;
    let res_headers = res.headers().clone();
    let (parts, body) = res.into_parts();
    let bytes = buffer_and_print("response", res_headers, body).await?;
    let res = Response::from_parts(parts, Body::from(bytes));

    let duration = start.elapsed();

    if res.status().is_success() {
        if duration.as_millis() > 500 {
            warn!(
                "[{} {}] [{}] {}ms",
                method,
                uri,
                res.status().as_u16(),
                duration.as_millis()
            );
        } else {
            info!(
                "[{} {}] [{}] {}ms",
                method,
                uri,
                res.status().as_u16(),
                duration.as_millis()
            );
        }
    } else {
        error!(
            "[{} {}] [{}] {}ms",
            method,
            uri,
            res.status().as_u16(),
            duration.as_millis()
        );
    }

    Ok(res)
}

pub async fn buffer_and_print<B>(
    direction: &str,
    headers: HeaderMap,
    body: B,
) -> std::result::Result<Bytes, (StatusCode, String)>
where
    B: axum::body::HttpBody<Data = Bytes>,
    B::Error: std::fmt::Display,
{
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("failed to read {direction} body: {err}"),
            ));
        }
    };

    if let Ok(body) = std::str::from_utf8(&bytes) {
        trace!("{direction} headers = {headers:?} body = {body:?}");
    }

    Ok(bytes)
}
