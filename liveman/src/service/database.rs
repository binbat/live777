use anyhow::{Context, Result};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use std::{path::PathBuf, time::Duration};
use tracing::info;

use crate::migration::Migrator;
use sea_orm_migration::MigratorTrait;

use crate::config::Database as DatabaseConfig;

#[derive(Clone)]
pub struct DatabaseService {
    pub connection: DatabaseConnection,
}

impl DatabaseService {
    pub async fn new(config: &DatabaseConfig) -> Result<Self> {
        let connection_url = normalize_sqlite_url(&config.url)?;

        let mut opt = ConnectOptions::new(&connection_url);
        opt.max_connections(config.max_connections)
            .connect_timeout(Duration::from_secs(config.connect_timeout))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(3600))
            .sqlx_logging(true);

        info!("Connecting to database: {}", connection_url);
        let connection = Database::connect(opt).await?;

        // Run migrations to ensure tables exist
        Migrator::up(&connection, None).await?;

        info!("Database connection established and migrations completed");

        Ok(Self { connection })
    }

    pub fn get_connection(&self) -> &DatabaseConnection {
        &self.connection
    }
}

fn normalize_sqlite_url(database_url: &str) -> Result<String> {
    if !database_url.starts_with("sqlite:") {
        return Ok(database_url.to_string());
    }

    let url_body = database_url.trim_start_matches("sqlite:");
    if url_body.starts_with(":memory:") {
        return Ok(database_url.to_string());
    }

    let mut parts = url_body.splitn(2, '?');
    let path_section = parts.next().unwrap_or("");
    let query_section = parts.next();

    if path_section.is_empty() {
        return Ok(database_url.to_string());
    }

    let path_without_prefix = path_section.trim_start_matches("//");
    if path_without_prefix.is_empty() {
        return Ok(database_url.to_string());
    }

    let mut db_path = PathBuf::from(path_without_prefix);
    if !db_path.is_absolute() {
        db_path = std::env::current_dir()?.join(db_path);
    }

    if let Some(parent) = db_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create database directory: {:?}", parent))?;
        }
    }

    if !db_path.exists() {
        std::fs::File::create(&db_path)
            .with_context(|| format!("failed to create sqlite database file at {:?}", db_path))?;
    }

    let mut normalized_path = db_path.to_string_lossy().to_string();
    if cfg!(windows) {
        normalized_path = normalized_path.replace('\\', "/");
    }

    let mut normalized_url = if db_path.is_absolute() {
        format!("sqlite:///{}", normalized_path)
    } else {
        format!("sqlite://{}", normalized_path)
    };
    if let Some(query) = query_section {
        normalized_url.push('?');
        normalized_url.push_str(query);
    }

    Ok(normalized_url)
}
