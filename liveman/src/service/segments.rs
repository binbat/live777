use anyhow::Result;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::segments::{self, Entity as Segments};

#[derive(Debug, Serialize, Deserialize)]
pub struct SegmentData {
    pub node_alias: String,
    pub stream: String,
    pub start_ts: i64,
    pub end_ts: i64,
    pub duration_ms: i32,
    pub path: String,
    pub is_keyframe: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SegmentReportRequest {
    pub node_alias: String,
    pub stream: String,
    pub segments: Vec<SegmentData>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimelineQueryParams {
    pub stream: String,
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

pub struct SegmentsService;

impl SegmentsService {
    pub async fn create_segments(
        db: &DatabaseConnection,
        request: SegmentReportRequest,
    ) -> Result<Vec<segments::Model>> {
        let mut created_segments = Vec::new();

        for segment_data in request.segments {
            let segment_model = segments::ActiveModel {
                id: Set(Uuid::new_v4()),
                node_alias: Set(request.node_alias.clone()),
                stream: Set(request.stream.clone()),
                start_ts: Set(segment_data.start_ts),
                end_ts: Set(segment_data.end_ts),
                duration_ms: Set(segment_data.duration_ms),
                path: Set(segment_data.path),
                is_keyframe: Set(segment_data.is_keyframe),
                created_at: Set(chrono::DateTime::<chrono::FixedOffset>::from(Utc::now())),
            };

            let inserted = segment_model.insert(db).await?;
            created_segments.push(inserted);
        }

        Ok(created_segments)
    }

    pub async fn get_timeline(
        db: &DatabaseConnection,
        params: TimelineQueryParams,
    ) -> Result<Vec<segments::Model>> {
        let mut query = Segments::find().filter(segments::Column::Stream.eq(&params.stream));

        if let Some(start_ts) = params.start_ts {
            query = query.filter(segments::Column::EndTs.gte(start_ts));
        }

        if let Some(end_ts) = params.end_ts {
            query = query.filter(segments::Column::StartTs.lte(end_ts));
        }

        query = query.order_by_asc(segments::Column::StartTs);

        if let Some(limit) = params.limit {
            query = query.limit(limit);
        }

        if let Some(offset) = params.offset {
            query = query.offset(offset);
        }

        let segments = query.all(db).await?;
        Ok(segments)
    }

    pub async fn get_streams(db: &DatabaseConnection) -> Result<Vec<String>> {
        let streams = Segments::find()
            .select_only()
            .column(segments::Column::Stream)
            .distinct()
            .all(db)
            .await?
            .into_iter()
            .map(|s| s.stream)
            .collect();

        Ok(streams)
    }

    pub async fn get_segment_by_path(
        db: &DatabaseConnection,
        path: &str,
    ) -> Result<Option<segments::Model>> {
        let segment = Segments::find()
            .filter(segments::Column::Path.eq(path))
            .one(db)
            .await?;

        Ok(segment)
    }

    pub async fn delete_old_segments(
        db: &DatabaseConnection,
        older_than: DateTime<Utc>,
    ) -> Result<u64> {
        let result = Segments::delete_many()
            .filter(segments::Column::CreatedAt.lt(older_than))
            .exec(db)
            .await?;

        Ok(result.rows_affected)
    }

    pub async fn get_segment_count_by_stream(
        db: &DatabaseConnection,
        stream: &str,
    ) -> Result<u64> {
        let count = Segments::find()
            .filter(segments::Column::Stream.eq(stream))
            .count(db)
            .await?;

        Ok(count)
    }
}