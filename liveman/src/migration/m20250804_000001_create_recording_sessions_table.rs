use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create the recording_sessions table
        manager
            .create_table(
                Table::create()
                    .table(RecordingSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RecordingSessions::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::Stream)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::NodeAlias)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::StartTs)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::EndTs)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::DurationMs)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::MpdPath)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::Status)
                            .string()
                            .not_null()
                            .default("Active"),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(RecordingSessions::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // Create indices
        manager
            .create_index(
                Index::create()
                    .name("idx_recording_sessions_stream_time")
                    .table(RecordingSessions::Table)
                    .col(RecordingSessions::Stream)
                    .col(RecordingSessions::StartTs)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_recording_sessions_status")
                    .table(RecordingSessions::Table)
                    .col(RecordingSessions::Status)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RecordingSessions::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RecordingSessions {
    Table,
    Id,
    Stream,
    NodeAlias,
    StartTs,
    EndTs,
    DurationMs,
    MpdPath,
    Status,
    CreatedAt,
    UpdatedAt,
}
