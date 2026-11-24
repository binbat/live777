#[cfg(feature = "recorder")]
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use http::StatusCode;
#[cfg(feature = "recorder")]
use http::{HeaderMap, header};

use crate::AppState;
#[cfg(feature = "recorder")]
use crate::error::AppError;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(
            &api::path::record("{stream}"),
            post(record_stream).get(record_status).delete(stop_record),
        )
        .route(
            api::path::recordings(),
            get(pull_recordings)
                .patch(ack_recordings)
                .delete(delete_recordings),
        )
        .route("/api/playback", get(list_playback_streams))
        .route("/api/playback/{stream}", get(list_playback_entries))
        .route("/api/record/object/{*path}", get(get_record_object))
}
#[cfg(feature = "recorder")]
async fn record_stream(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    body: Option<Json<api::recorder::StartRecordRequest>>,
) -> crate::result::Result<Response<String>> {
    let base_dir = body.and_then(|Json(req)| req.base_dir);
    let recording = crate::recorder::start(state.stream_manager.clone(), stream.clone(), base_dir)
        .await
        .map_err(
            |e| match e.downcast_ref::<crate::recorder::UnsupportedVideoCodec>() {
                Some(codec_err) => AppError::BadRequest(codec_err.to_string()),
                None => AppError::InternalServerError(e),
            },
        )?;

    let mpd_path = format!("{}/manifest.mpd", recording.record_dir);
    let record_id_str = if recording.record_id > 0 {
        recording.record_id.to_string()
    } else {
        let ts = (recording.start_ts_micros / 1_000_000).max(0);
        ts.to_string()
    };
    let resp = api::recorder::StartRecordResponse {
        id: stream.clone(),
        record_id: record_id_str,
        record_dir: recording.record_dir,
        mpd_path,
    };
    match serde_json::to_string(&resp) {
        Ok(json_body) => Ok(Response::builder().status(StatusCode::OK).body(json_body)?),
        Err(e) => Err(AppError::InternalServerError(anyhow::anyhow!(
            "Failed to serialize response: {}",
            e
        ))),
    }
}

#[cfg(not(feature = "recorder"))]
async fn record_stream(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn record_status(
    State(_state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Json<serde_json::Value>> {
    let recording = crate::recorder::is_recording(&stream).await;
    Ok(Json(serde_json::json!({ "recording": recording })))
}

#[cfg(not(feature = "recorder"))]
async fn record_status(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn stop_record(
    State(_state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    crate::recorder::stop(stream.clone()).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body("".to_string())?)
}

#[cfg(not(feature = "recorder"))]
async fn stop_record(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn pull_recordings(
    Query(req): Query<api::recorder::PullRecordingsRequest>,
) -> crate::result::Result<Json<api::recorder::PullRecordingsResponse>> {
    let resp = crate::recorder::pull_recordings(req).await?;
    Ok(Json(resp))
}

#[cfg(not(feature = "recorder"))]
async fn pull_recordings(
    Query(_req): Query<api::recorder::PullRecordingsRequest>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn ack_recordings(
    Json(req): Json<api::recorder::AckRecordingsRequest>,
) -> crate::result::Result<Json<api::recorder::AckRecordingsResponse>> {
    let resp = crate::recorder::ack_recordings(req).await?;
    Ok(Json(resp))
}

#[cfg(not(feature = "recorder"))]
async fn ack_recordings(
    Json(_req): Json<api::recorder::AckRecordingsRequest>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn delete_recordings(
    Json(req): Json<api::recorder::DeleteRecordingsRequest>,
) -> crate::result::Result<Json<api::recorder::DeleteRecordingsResponse>> {
    let resp = crate::recorder::delete_recordings(req).await?;
    Ok(Json(resp))
}

#[cfg(not(feature = "recorder"))]
async fn delete_recordings(
    Json(_req): Json<api::recorder::DeleteRecordingsRequest>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn list_playback_streams() -> crate::result::Result<Json<Vec<String>>> {
    Ok(Json(crate::recorder::list_playback_streams().await?))
}

#[cfg(not(feature = "recorder"))]
async fn list_playback_streams() -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn list_playback_entries(
    Path(stream): Path<String>,
) -> crate::result::Result<Json<Vec<crate::recorder::PlaybackIndexEntry>>> {
    Ok(Json(crate::recorder::list_playback_entries(&stream).await?))
}

#[cfg(not(feature = "recorder"))]
async fn list_playback_entries(Path(_stream): Path<String>) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn get_record_object(
    Path(path): Path<String>,
    headers: HeaderMap,
) -> crate::result::Result<Response> {
    let Some((size, reader)) = crate::recorder::open_object(&path).await? else {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            "Recorder storage is not available",
        )
            .into_response());
    };

    // RFC 7233 single-range support; a malformed or multi-range header is
    // ignored and the whole object is served.
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_bytes_range(value, size));

    let (status, content_range, body_range) = match range {
        Some(BytesRange::Unsatisfiable) => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
            )
                .into_response());
        }
        Some(BytesRange::Satisfiable(range)) => (
            StatusCode::PARTIAL_CONTENT,
            Some(format!("bytes {}-{}/{}", range.start, range.end - 1, size)),
            range,
        ),
        None => (StatusCode::OK, None, 0..size),
    };

    let body_len = body_range.end - body_range.start;
    let stream = reader.into_bytes_stream(body_range).await?;
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, record_object_content_type(&path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, body_len);
    if let Some(content_range) = content_range {
        builder = builder.header(header::CONTENT_RANGE, content_range);
    }
    Ok(builder.body(Body::from_stream(stream))?)
}

#[cfg(not(feature = "recorder"))]
async fn get_record_object(Path(_path): Path<String>) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(not(feature = "recorder"))]
fn recorder_not_enabled() -> Response {
    (StatusCode::NOT_IMPLEMENTED, "feature recorder not enabled").into_response()
}

#[cfg(feature = "recorder")]
#[derive(Debug)]
enum BytesRange {
    Satisfiable(std::ops::Range<u64>),
    Unsatisfiable,
}

/// Parse a single `bytes=` range against the object size. `None` means the
/// header is malformed (or names a non-bytes unit) and must be ignored.
#[cfg(feature = "recorder")]
fn parse_bytes_range(value: &str, size: u64) -> Option<BytesRange> {
    if size == 0 {
        return None;
    }
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        // Multi-range requests would need a multipart body; ignore instead.
        return None;
    }
    let (start_s, end_s) = spec.trim().split_once('-')?;
    if start_s.is_empty() {
        // Suffix form: the last N bytes.
        let suffix: u64 = end_s.parse().ok()?;
        if suffix == 0 {
            return Some(BytesRange::Unsatisfiable);
        }
        Some(BytesRange::Satisfiable(size.saturating_sub(suffix)..size))
    } else {
        let start: u64 = start_s.parse().ok()?;
        if start >= size {
            return Some(BytesRange::Unsatisfiable);
        }
        if end_s.is_empty() {
            return Some(BytesRange::Satisfiable(start..size));
        }
        let end: u64 = end_s.parse().ok()?;
        if end < start {
            return None;
        }
        Some(BytesRange::Satisfiable(
            start..end.saturating_add(1).min(size),
        ))
    }
}

#[cfg(feature = "recorder")]
fn record_object_content_type(path: &str) -> &'static str {
    if path.ends_with(".mpd") {
        "application/dash+xml"
    } else if path.ends_with(".m4s") || path.ends_with(".mp4") {
        if path.contains("audio_") {
            "audio/mp4"
        } else {
            "video/mp4"
        }
    } else {
        "application/octet-stream"
    }
}

#[cfg(all(test, feature = "recorder"))]
mod tests {
    use super::{BytesRange, parse_bytes_range};

    fn satisfiable(value: &str, size: u64) -> std::ops::Range<u64> {
        match parse_bytes_range(value, size) {
            Some(BytesRange::Satisfiable(range)) => range,
            other => panic!("{value} should be satisfiable, got {other:?}"),
        }
    }

    #[test]
    fn parse_bytes_range_satisfiable_forms() {
        assert_eq!(satisfiable("bytes=5-9", 24), 5..10);
        assert_eq!(satisfiable("bytes=5-", 24), 5..24);
        assert_eq!(satisfiable("bytes=-4", 24), 20..24);
        assert_eq!(satisfiable("bytes=-100", 24), 0..24);
        // End beyond the object is clamped.
        assert_eq!(satisfiable("bytes=20-100", 24), 20..24);
        assert_eq!(satisfiable("bytes=0-18446744073709551615", 24), 0..24);
    }

    #[test]
    fn parse_bytes_range_unsatisfiable_forms() {
        assert!(matches!(
            parse_bytes_range("bytes=24-30", 24),
            Some(BytesRange::Unsatisfiable)
        ));
        assert!(matches!(
            parse_bytes_range("bytes=-0", 24),
            Some(BytesRange::Unsatisfiable)
        ));
    }

    #[test]
    fn parse_bytes_range_ignored_forms() {
        assert!(parse_bytes_range("items=1-2", 24).is_none());
        assert!(parse_bytes_range("bytes=9-5", 24).is_none());
        assert!(parse_bytes_range("bytes=abc-def", 24).is_none());
        assert!(parse_bytes_range("bytes=0-1,3-4", 24).is_none());
        assert!(parse_bytes_range("bytes=5", 24).is_none());
        assert!(parse_bytes_range("bytes=0-", 0).is_none());
    }
}
