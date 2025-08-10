use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Recordings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Recordings::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Recordings::Stream).string().not_null())
                    .col(ColumnDef::new(Recordings::Year).integer().not_null())
                    .col(ColumnDef::new(Recordings::Month).integer().not_null())
                    .col(ColumnDef::new(Recordings::Day).integer().not_null())
                    .col(ColumnDef::new(Recordings::MpdPath).string().not_null())
                    .col(
                        ColumnDef::new(Recordings::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Recordings::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .index(
                        Index::create()
                            .name("idx_recordings_stream_date")
                            .table(Recordings::Table)
                            .col(Recordings::Stream)
                            .col(Recordings::Year)
                            .col(Recordings::Month)
                            .col(Recordings::Day)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Recordings::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Recordings {
    Table,
    Id,
    Stream,
    Year,
    Month,
    Day,
    MpdPath,
    CreatedAt,
    UpdatedAt,
}
