use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Segments::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Segments::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Segments::NodeAlias).string().not_null())
                    .col(ColumnDef::new(Segments::Stream).string().not_null())
                    .col(ColumnDef::new(Segments::StartTs).big_integer().not_null())
                    .col(ColumnDef::new(Segments::EndTs).big_integer().not_null())
                    .col(ColumnDef::new(Segments::DurationMs).integer().not_null())
                    .col(ColumnDef::new(Segments::Path).string().not_null())
                    .col(ColumnDef::new(Segments::IsKeyframe).boolean().not_null())
                    .col(
                        ColumnDef::new(Segments::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .index(
                        Index::create()
                            .name("idx_segments_stream_time")
                            .col(Segments::Stream)
                            .col(Segments::StartTs)
                            .col(Segments::EndTs),
                    )
                    .index(
                        Index::create()
                            .name("idx_segments_node_alias")
                            .col(Segments::NodeAlias),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Segments::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Segments {
    Table,
    Id,
    NodeAlias,
    Stream,
    StartTs,
    EndTs,
    DurationMs,
    Path,
    IsKeyframe,
    CreatedAt,
}