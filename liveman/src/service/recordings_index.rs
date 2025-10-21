use anyhow::Result;
use chrono::{FixedOffset, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entity::recordings::{self, Entity as Recordings};

#[derive(Clone)]
pub struct RecordingsIndexService;

impl RecordingsIndexService {
    pub async fn upsert(
        db: &DatabaseConnection,
        stream: &str,
        record: &str,
        mpd_path: &str,
    ) -> Result<recordings::Model> {
        if let Some(existing) = Recordings::find()
            .filter(recordings::Column::Stream.eq(stream))
            .filter(recordings::Column::Record.eq(record))
            .one(db)
            .await?
        {
            let mut am: recordings::ActiveModel = existing.into();
            am.mpd_path = Set(mpd_path.to_string());
            am.updated_at = Set(Utc::now().with_timezone(&FixedOffset::east_opt(0).unwrap()));
            Ok(am.update(db).await?)
        } else {
            let now_fixed = Utc::now().with_timezone(&FixedOffset::east_opt(0).unwrap());
            let am = recordings::ActiveModel {
                id: Set(Uuid::new_v4()),
                stream: Set(stream.to_string()),
                record: Set(record.to_string()),
                mpd_path: Set(mpd_path.to_string()),
                created_at: Set(now_fixed),
                updated_at: Set(now_fixed),
            };
            Ok(am.insert(db).await?)
        }
    }

    pub async fn list_by_stream(
        db: &DatabaseConnection,
        stream: &str,
    ) -> Result<Vec<recordings::Model>> {
        Ok(Recordings::find()
            .filter(recordings::Column::Stream.eq(stream))
            .all(db)
            .await?)
    }
}
