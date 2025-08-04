pub use sea_orm_migration::prelude::*;

mod m20240801_000001_create_segments_table;
mod m20250804_000001_create_recording_sessions_table;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20240801_000001_create_segments_table::Migration),
            Box::new(m20250804_000001_create_recording_sessions_table::Migration),
        ]
    }
}
