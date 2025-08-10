use anyhow::Result;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use crate::entity::recording_sessions::{self, Entity as RecordingSessions};

use api::recorder::RecordingStatus;

pub struct RecordingSessionsService;

impl RecordingSessionsService {
    /// Insert a new Active recording row when liveman triggers recording
    pub async fn insert_on_start(
        db: &DatabaseConnection,
        node_alias: String,
        stream: String,
        mpd_path: String,
    ) -> Result<recording_sessions::Model> {
        let now = Utc::now();
        let offset = chrono::FixedOffset::east_opt(0).unwrap();
        let now_fixed = now.with_timezone(&offset);
        let session_model = recording_sessions::ActiveModel {
            id: Set(Uuid::new_v4()),
            stream: Set(stream),
            node_alias: Set(node_alias),
            start_ts: Set(now.timestamp_micros()),
            end_ts: Set(None),
            duration_ms: Set(None),
            mpd_path: Set(mpd_path),
            status: Set(RecordingStatus::Active.to_string()),
            created_at: Set(now_fixed),
            updated_at: Set(now_fixed),
        };

        let inserted = session_model.insert(db).await?;
        Ok(inserted)
    }

    /// Mark last Active recording as Completed when live777 reports end
    pub async fn mark_completed(
        db: &DatabaseConnection,
        stream: &str,
        started_after_ts: i64,
    ) -> Result<Option<recording_sessions::Model>> {
        if let Some(existing) = RecordingSessions::find()
            .filter(recording_sessions::Column::Stream.eq(stream))
            .filter(recording_sessions::Column::StartTs.gte(started_after_ts))
            .order_by_desc(recording_sessions::Column::StartTs)
            .one(db)
            .await?
        {
            let now = Utc::now();
            let offset = chrono::FixedOffset::east_opt(0).unwrap();
            let now_fixed = now.with_timezone(&offset);
            let mut active: recording_sessions::ActiveModel = existing.clone().into();
            active.end_ts = Set(Some(now.timestamp_micros()));
            if let Some(end_ts) = active.end_ts.as_ref().to_owned() {
                let duration_ms = ((end_ts - active.start_ts.as_ref().to_owned()) / 1000) as i32;
                active.duration_ms = Set(Some(duration_ms));
            }
            active.status = Set(RecordingStatus::Completed.to_string());
            active.updated_at = Set(now_fixed);
            let updated = active.update(db).await?;
            Ok(Some(updated))
        } else {
            Ok(None)
        }
    }
}
