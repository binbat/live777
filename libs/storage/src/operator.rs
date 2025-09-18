use crate::config::StorageConfig;
use anyhow::Result;
use opendal::Operator;
use opendal::services;

/// Create storage operator based on storage configuration
pub fn create_operator(config: &StorageConfig) -> Result<Operator> {
    tracing::debug!("Creating storage operator for config: {:?}", config);

    match config {
        StorageConfig::Fs { root } => {
            tracing::info!("Configuring filesystem storage with root: {}", root);
            let builder = services::Fs::default().root(root);
            let op = Operator::new(builder)?.finish();
            tracing::debug!("Filesystem storage operator created successfully");
            Ok(op)
        }
        StorageConfig::S3 {
            bucket,
            root,
            region,
            endpoint,
            access_key_id,
            secret_access_key,
            session_token,
            disable_config_load,
            enable_virtual_host_style,
        } => {
            tracing::info!(
                "Configuring S3 storage with bucket: {}, region: {:?}",
                bucket,
                region
            );

            let mut builder = services::S3::default()
                .bucket(bucket)
                .root(root.trim_start_matches('/'));

            if let Some(region) = region {
                builder = builder.region(region);
                tracing::debug!("S3 region set to: {}", region);
            }

            if let Some(endpoint) = endpoint {
                builder = builder.endpoint(endpoint);
                tracing::debug!("S3 endpoint set to: {}", endpoint);
            }

            if let Some(access_key_id) = access_key_id {
                builder = builder.access_key_id(access_key_id);
                tracing::debug!("S3 access key configured");
            }

            if let Some(secret_access_key) = secret_access_key {
                builder = builder.secret_access_key(secret_access_key);
                tracing::debug!("S3 secret key configured");
            }

            if let Some(session_token) = session_token {
                builder = builder.session_token(session_token);
                tracing::debug!("S3 session token configured");
            }

            if *disable_config_load {
                builder = builder.disable_config_load();
                tracing::debug!("S3 config load disabled");
            }

            if *enable_virtual_host_style {
                builder = builder.enable_virtual_host_style();
                tracing::debug!("S3 virtual host style enabled");
            }

            let op = Operator::new(builder)?.finish();
            tracing::debug!("S3 storage operator created successfully");
            Ok(op)
        }
        StorageConfig::Oss {
            bucket,
            root,
            region,
            endpoint,
            access_key_id,
            access_key_secret,
            security_token,
        } => {
            tracing::info!(
                "Configuring OSS storage with bucket: {}, region: {}",
                bucket,
                region
            );

            // Use S3 service for OSS compatibility
            let mut builder = services::S3::default()
                .bucket(bucket)
                .root(root.trim_start_matches('/'))
                .region(region)
                .endpoint(endpoint)
                .enable_virtual_host_style();

            if let Some(access_key_id) = access_key_id {
                builder = builder.access_key_id(access_key_id);
                tracing::debug!("OSS access key configured");
            }

            if let Some(access_key_secret) = access_key_secret {
                builder = builder.secret_access_key(access_key_secret);
                tracing::debug!("OSS secret key configured");
            }

            if let Some(security_token) = security_token {
                builder = builder.session_token(security_token);
                tracing::debug!("OSS security token configured");
            }

            let op = Operator::new(builder)?.finish();
            tracing::debug!("OSS storage operator created successfully");
            Ok(op)
        }
    }
}

/// Test storage connection
pub async fn test_connection(operator: &Operator) -> Result<()> {
    operator.check().await?;
    tracing::info!("Storage connection test successful");
    Ok(())
}

/// Initialize storage operator with connection test
pub async fn init_operator(config: &StorageConfig) -> Result<Operator> {
    let operator = create_operator(config)?;

    // Test the storage connection
    match test_connection(&operator).await {
        Ok(_) => {
            tracing::info!("Storage backend initialized and verified: {:?}", config);
        }
        Err(e) => {
            tracing::warn!(
                "Storage backend initialized but connection test failed: {}, continuing anyway",
                e
            );
        }
    }

    Ok(operator)
}
