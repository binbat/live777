use anyhow::Result;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::recording_sessions::{self, Entity as RecordingSessions};

use api::recorder::RecordingSession;

#[derive(Debug, Serialize, Deserialize)]
pub struct RecordingQueryParams {
    pub stream: Option<String>,
    pub status: Option<String>,
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

pub struct RecordingSessionsService;

impl RecordingSessionsService {
    /// Create recording sessions from pulled data (used by puller)
    pub async fn create_sessions_from_pull(
        db: &DatabaseConnection,
        node_alias: String,
        sessions: Vec<RecordingSession>,
    ) -> Result<Vec<recording_sessions::Model>> {
        let mut created_sessions = Vec::new();

        for session in sessions {
            // Check if session already exists (by stream and start_ts)
            let existing = RecordingSessions::find()
                .filter(recording_sessions::Column::Stream.eq(&session.stream))
                .filter(recording_sessions::Column::StartTs.eq(session.start_ts))
                .one(db)
                .await?;

            if let Some(existing_session) = existing {
                // Update existing session
                let mut active_model: recording_sessions::ActiveModel = existing_session.into();
                active_model.node_alias = Set(node_alias.clone());
                active_model.end_ts = Set(session.end_ts);
                active_model.duration_ms = Set(session.duration_ms);
                active_model.status = Set(session.status.to_string());
                active_model.updated_at =
                    Set(chrono::DateTime::<chrono::FixedOffset>::from(Utc::now()));

                let updated = active_model.update(db).await?;
                created_sessions.push(updated);
            } else {
                // Create new session
                let session_model = recording_sessions::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    stream: Set(session.stream),
                    node_alias: Set(node_alias.clone()),
                    start_ts: Set(session.start_ts),
                    end_ts: Set(session.end_ts),
                    duration_ms: Set(session.duration_ms),
                    mpd_path: Set(session.mpd_path),
                    status: Set(session.status.to_string()),
                    created_at: Set(chrono::DateTime::<chrono::FixedOffset>::from(Utc::now())),
                    updated_at: Set(chrono::DateTime::<chrono::FixedOffset>::from(Utc::now())),
                };

                let inserted = session_model.insert(db).await?;
                created_sessions.push(inserted);
            }
        }

        Ok(created_sessions)
    }

    pub async fn get_recordings(
        db: &DatabaseConnection,
        params: RecordingQueryParams,
    ) -> Result<Vec<recording_sessions::Model>> {
        let mut query = RecordingSessions::find();

        if let Some(stream) = params.stream {
            query = query.filter(recording_sessions::Column::Stream.eq(stream));
        }

        if let Some(status) = params.status {
            query = query.filter(recording_sessions::Column::Status.eq(status));
        }

        if let Some(start_ts) = params.start_ts {
            query = query.filter(recording_sessions::Column::StartTs.gte(start_ts));
        }

        if let Some(end_ts) = params.end_ts {
            query = query.filter(recording_sessions::Column::StartTs.lte(end_ts));
        }

        query = query.order_by_desc(recording_sessions::Column::StartTs);

        if let Some(limit) = params.limit {
            query = query.limit(limit);
        }

        if let Some(offset) = params.offset {
            query = query.offset(offset);
        }

        let sessions = query.all(db).await?;
        Ok(sessions)
    }

    pub async fn get_streams(db: &DatabaseConnection) -> Result<Vec<String>> {
        let streams = RecordingSessions::find()
            .select_only()
            .column(recording_sessions::Column::Stream)
            .distinct()
            .all(db)
            .await?
            .into_iter()
            .map(|s| s.stream)
            .collect();

        Ok(streams)
    }

    pub async fn get_recording_by_id(
        db: &DatabaseConnection,
        id: Uuid,
    ) -> Result<Option<recording_sessions::Model>> {
        let session = RecordingSessions::find()
            .filter(recording_sessions::Column::Id.eq(id))
            .one(db)
            .await?;

        Ok(session)
    }

    pub async fn delete_old_recordings(
        db: &DatabaseConnection,
        older_than: DateTime<Utc>,
    ) -> Result<u64> {
        let result = RecordingSessions::delete_many()
            .filter(recording_sessions::Column::CreatedAt.lt(older_than))
            .exec(db)
            .await?;

        Ok(result.rows_affected)
    }

    pub async fn get_recording_count_by_stream(
        db: &DatabaseConnection,
        stream: &str,
    ) -> Result<u64> {
        let count = RecordingSessions::find()
            .filter(recording_sessions::Column::Stream.eq(stream))
            .count(db)
            .await?;

        Ok(count)
    }
}
