use axum::{extract::State, response::Json, routing::post, Router};
use tracing::{error, info};

use crate::dto::recorder::{SegmentReportRequest, SegmentReportResponse};
use crate::service::segments::{SegmentData, SegmentsService};
use crate::{error::AppError, result::Result, AppState};

pub fn route() -> Router<AppState> {
    Router::new().route("/api/segments/report", post(report_segments))
}

async fn report_segments(
    State(state): State<AppState>,
    Json(request): Json<SegmentReportRequest>,
) -> Result<Json<SegmentReportResponse>> {
    info!(
        "Received segment report from node '{}' for stream '{}' with {} segments",
        request.node_alias,
        request.stream,
        request.segments.len()
    );

    if request.segments.is_empty() {
        return Ok(Json(SegmentReportResponse {
            success: false,
            message: "No segments provided".to_string(),
            processed_count: 0,
        }));
    }

    // Convert DTO segments to service format
    let service_request = crate::service::segments::SegmentReportRequest {
        node_alias: request.node_alias.clone(),
        stream: request.stream.clone(),
        segments: request
            .segments
            .into_iter()
            .map(|seg| SegmentData {
                node_alias: request.node_alias.clone(),
                stream: request.stream.clone(),
                start_ts: seg.start_ts,
                end_ts: seg.end_ts,
                duration_ms: seg.duration_ms,
                path: seg.path,
                is_keyframe: seg.is_keyframe,
            })
            .collect(),
    };

    match SegmentsService::create_segments(state.database.get_connection(), service_request).await {
        Ok(created_segments) => {
            info!(
                "Successfully stored {} segments for stream '{}' from node '{}'",
                created_segments.len(),
                request.stream,
                request.node_alias
            );

            Ok(Json(SegmentReportResponse {
                success: true,
                message: "Segments processed successfully".to_string(),
                processed_count: created_segments.len(),
            }))
        }
        Err(e) => {
            error!(
                "Failed to store segments for stream '{}' from node '{}': {}",
                request.stream, request.node_alias, e
            );

            Err(AppError::DatabaseError(e.to_string()))
        }
    }
}