use anyhow::Result;
use sea_orm::{Database, DatabaseConnection, ConnectOptions};
use std::time::Duration;
use tracing::info;

use crate::config::Database as DatabaseConfig;
use crate::migration::{Migrator, MigratorTrait};

#[derive(Clone)]
pub struct DatabaseService {
    pub connection: DatabaseConnection,
}

impl DatabaseService {
    pub async fn new(config: &DatabaseConfig) -> Result<Self> {
        let mut opt = ConnectOptions::new(&config.url);
        opt.max_connections(config.max_connections)
            .connect_timeout(Duration::from_secs(config.connect_timeout))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(3600))
            .sqlx_logging(true);

        info!("Connecting to database: {}", config.url);
        let connection = Database::connect(opt).await?;

        info!("Running database migrations...");
        Migrator::up(&connection, None).await?;
        
        info!("Database connection established and migrations completed");
        
        Ok(Self { connection })
    }

    pub fn get_connection(&self) -> &DatabaseConnection {
        &self.connection
    }
}