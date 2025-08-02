use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tracing::{error, info};
use std::collections::BTreeMap;

use crate::dto::recorder::{SegmentReportRequest, SegmentReportResponse, StreamsListResponse, TimelineResponse, TimelineSegment};
use crate::service::segments::{SegmentData, SegmentsService, TimelineQueryParams};
use crate::{error::AppError, result::Result, AppState};

#[derive(Deserialize)]
struct TimelineQuery {
    start_ts: Option<i64>,
    end_ts: Option<i64>,
    limit: Option<u64>,
    offset: Option<u64>,
}

#[derive(Deserialize)]
struct MpdQuery {
    start_ts: Option<i64>,
    end_ts: Option<i64>,
}

pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/segments/report", post(report_segments))
        .route("/api/record/streams", get(get_streams))
        .route("/api/record/:stream/timeline", get(get_timeline))
        .route("/api/record/:stream/mpd", get(get_mpd))
        .route("/api/record/object/*path", get(get_segment))
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

async fn get_streams(State(state): State<AppState>) -> Result<Json<StreamsListResponse>> {
    match SegmentsService::get_streams(state.database.get_connection()).await {
        Ok(streams) => {
            info!("Retrieved {} recorded streams", streams.len());
            Ok(Json(StreamsListResponse { streams }))
        }
        Err(e) => {
            error!("Failed to retrieve recorded streams: {}", e);
            Err(AppError::DatabaseError(e.to_string()))
        }
    }
}

async fn get_timeline(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> Result<Json<TimelineResponse>> {
    let params = TimelineQueryParams {
        stream: stream.clone(),
        start_ts: query.start_ts,
        end_ts: query.end_ts,
        limit: query.limit,
        offset: query.offset,
    };

    match SegmentsService::get_timeline(state.database.get_connection(), params).await {
        Ok(segments) => {
            let timeline_segments: Vec<TimelineSegment> = segments
                .into_iter()
                .map(|seg| TimelineSegment {
                    id: seg.id.to_string(),
                    start_ts: seg.start_ts,
                    end_ts: seg.end_ts,
                    duration_ms: seg.duration_ms,
                    path: seg.path,
                    is_keyframe: seg.is_keyframe,
                    created_at: seg.created_at.to_string(),
                })
                .collect();

            let total_count = timeline_segments.len() as u64;
            
            info!(
                "Retrieved {} timeline segments for stream '{}'",
                total_count, stream
            );

            Ok(Json(TimelineResponse {
                stream,
                segments: timeline_segments,
                total_count,
            }))
        }
        Err(e) => {
            error!("Failed to retrieve timeline for stream '{}': {}", stream, e);
            Err(AppError::DatabaseError(e.to_string()))
        }
    }
}

async fn get_mpd(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Query(query): Query<MpdQuery>,
) -> Result<Response> {
    let params = TimelineQueryParams {
        stream: stream.clone(),
        start_ts: query.start_ts,
        end_ts: query.end_ts,
        limit: None,
        offset: None,
    };

    match SegmentsService::get_timeline(state.database.get_connection(), params).await {
        Ok(segments) => {
            if segments.is_empty() {
                return Ok((StatusCode::NOT_FOUND, "No segments found for stream").into_response());
            }

            let mpd_xml = generate_mpd_xml(&stream, &segments, &state.config.playback)?;
            
            info!("Generated MPD for stream '{}' with {} segments", stream, segments.len());
            
            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/dash+xml")],
                mpd_xml,
            ).into_response())
        }
        Err(e) => {
            error!("Failed to retrieve segments for MPD generation: {}", e);
            Err(AppError::DatabaseError(e.to_string()))
        }
    }
}

async fn get_segment(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Response> {
    #[cfg(feature = "recorder")]
    {
        if let Some(ref operator) = state.file_storage {
            match operator.read(&path).await {
                Ok(bytes) => {
                    info!("Successfully served segment: {}", path);
                    
                    // Determine content type based on file extension
                    let content_type = if path.ends_with(".m4s") || path.ends_with(".mp4") {
                        "video/mp4"
                    } else if path.ends_with(".mpd") {
                        "application/dash+xml"
                    } else {
                        "application/octet-stream"
                    };

                    Ok((
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, content_type)],
                        bytes.to_vec(),
                    ).into_response())
                }
                Err(e) => {
                    error!("Failed to read segment file '{}': {}", path, e);
                    Ok((StatusCode::NOT_FOUND, "Segment not found").into_response())
                }
            }
        } else {
            error!("File storage not configured for segment access");
            Ok((StatusCode::SERVICE_UNAVAILABLE, "File storage not available").into_response())
        }
    }
    
    #[cfg(not(feature = "recorder"))]
    {
        // Avoid unused variable warnings
        let _ = state;
        let _ = path;
        Ok((StatusCode::NOT_IMPLEMENTED, "Recorder feature not enabled").into_response())
    }
}

fn generate_mpd_xml(
    _stream: &str,
    segments: &[crate::entity::segments::Model],
    playback_config: &crate::config::Playback,
) -> Result<String> {
    if segments.is_empty() {
        return Err(AppError::DatabaseError("No segments available".to_string()));
    }

    // Calculate timeline boundaries
    let min_timestamp = segments.iter().map(|s| s.start_ts).min().unwrap();
    let max_timestamp = segments.iter().map(|s| s.end_ts).max().unwrap();
    let total_duration = (max_timestamp - min_timestamp) as f64 / 1_000_000.0; // Convert microseconds to seconds

    // Group segments by path prefix to identify different representations
    let mut representations: BTreeMap<String, Vec<&crate::entity::segments::Model>> = BTreeMap::new();
    
    for segment in segments {
        // Extract representation ID from path (e.g., "camera01/2024/01/01/seg_0001.m4s" -> "video")
        // For now, we'll assume all segments are video. In a real implementation,
        // you'd parse the path to distinguish video/audio tracks
        let repr_id = if segment.path.contains("audio") {
            "audio".to_string()
        } else {
            "video".to_string()
        };
        
        representations.entry(repr_id).or_default().push(segment);
    }

    // Generate MPD XML
    let mut mpd = String::new();
    mpd.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    mpd.push('\n');
    mpd.push_str(r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration=""#);
    mpd.push_str(&format!("PT{total_duration:.3}S"));
    mpd.push_str(r#"">"#);
    mpd.push('\n');
    
    // Period
    mpd.push_str(r#"  <Period>"#);
    mpd.push('\n');

    // Generate AdaptationSets for each representation
    for (repr_id, repr_segments) in representations {
        let is_video = repr_id == "video";
        
        if is_video {
            mpd.push_str(r#"    <AdaptationSet mimeType="video/mp4" codecs="avc1.42c01e">"#);
        } else {
            mpd.push_str(r#"    <AdaptationSet mimeType="audio/mp4" codecs="mp4a.40.2">"#);
        }
        mpd.push('\n');

        // Representation
        mpd.push_str(&format!(r#"      <Representation id="{repr_id}" bandwidth="1000000">"#));
        mpd.push('\n');

        // SegmentList
        mpd.push_str(r#"        <SegmentList>"#);
        mpd.push('\n');

        // Generate segment URLs
        for segment in repr_segments {
            let segment_url = if playback_config.signed_redirect {
                // Generate signed URL (placeholder for now)
                format!("/api/record/object/{}?expires={}", 
                    segment.path,
                    chrono::Utc::now().timestamp() + playback_config.signed_ttl_seconds as i64
                )
            } else {
                format!("/api/record/object/{}", segment.path)
            };

            mpd.push_str(&format!(
                r#"          <SegmentURL media="{segment_url}"/>"#
            ));
            mpd.push('\n');
        }

        mpd.push_str(r#"        </SegmentList>"#);
        mpd.push('\n');
        mpd.push_str(r#"      </Representation>"#);
        mpd.push('\n');
        mpd.push_str(r#"    </AdaptationSet>"#);
        mpd.push('\n');
    }

    mpd.push_str(r#"  </Period>"#);
    mpd.push('\n');
    mpd.push_str(r#"</MPD>"#);

    Ok(mpd)
}