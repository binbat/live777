use crate::{StorageConfig, create_operator};

#[tokio::test]
async fn test_fs_storage_config() {
    let config = StorageConfig::Fs {
        root: "./test_records".to_string(),
    };

    let result = create_operator(&config);
    assert!(result.is_ok(), "Failed to create FS storage operator");
}

#[tokio::test]
async fn test_s3_storage_config() {
    let config = StorageConfig::S3 {
        bucket: "test-bucket".to_string(),
        root: "/test".to_string(),
        region: Some("us-east-1".to_string()),
        endpoint: Some("http://localhost:9000".to_string()),
        access_key_id: Some("minioadmin".to_string()),
        secret_access_key: Some("minioadmin".to_string()),
        session_token: None,
        disable_config_load: true,
        enable_virtual_host_style: false,
    };

    let result = create_operator(&config);
    assert!(result.is_ok(), "Failed to create S3 storage operator");
}

#[tokio::test]
async fn test_oss_storage_config() {
    let config = StorageConfig::Oss {
        bucket: "test-bucket".to_string(),
        root: "/test".to_string(),
        region: "oss-cn-hangzhou".to_string(),
        endpoint: "https://oss-cn-hangzhou.aliyuncs.com".to_string(),
        access_key_id: Some("test-key".to_string()),
        access_key_secret: Some("test-secret".to_string()),
        security_token: None,
    };

    let result = create_operator(&config);
    assert!(result.is_ok(), "Failed to create OSS storage operator");
}

#[test]
fn test_storage_config_serialization() {
    let config = StorageConfig::S3 {
        bucket: "my-bucket".to_string(),
        root: "/recordings".to_string(),
        region: Some("us-west-2".to_string()),
        endpoint: None,
        access_key_id: Some("AKIA...".to_string()),
        secret_access_key: Some("secret...".to_string()),
        session_token: None,
        disable_config_load: false,
        enable_virtual_host_style: true,
    };

    let serialized = toml::to_string(&config).expect("Failed to serialize config");
    let deserialized: StorageConfig =
        toml::from_str(&serialized).expect("Failed to deserialize config");

    match (&config, &deserialized) {
        (StorageConfig::S3 { bucket: b1, .. }, StorageConfig::S3 { bucket: b2, .. }) => {
            assert_eq!(b1, b2, "Bucket names should match");
        }
        _ => panic!("Storage config type mismatch"),
    }
}

#[test]
fn test_default_storage_config() {
    let config = StorageConfig::default();

    match config {
        StorageConfig::Fs { root } => {
            assert_eq!(root, "./storage");
        }
        _ => panic!("Default storage should be FS"),
    }
}

#[test]
fn test_s3_config_parsing() {
    let toml_str = r#"
type = "s3"
bucket = "test-bucket"
root = "/recordings"
region = "us-east-1"
access_key_id = "test-key"
secret_access_key = "test-secret"
enable_virtual_host_style = true
"#;

    let config: StorageConfig = toml::from_str(toml_str).expect("Failed to parse TOML config");

    match config {
        StorageConfig::S3 {
            bucket,
            root,
            region,
            enable_virtual_host_style,
            ..
        } => {
            assert_eq!(bucket, "test-bucket");
            assert_eq!(root, "/recordings");
            assert_eq!(region, Some("us-east-1".to_string()));
            assert!(enable_virtual_host_style);
        }
        _ => panic!("Expected S3 storage config"),
    }
}

#[test]
fn test_oss_config_parsing() {
    let toml_str = r#"
type = "oss"
bucket = "my-oss-bucket"
root = "/data"
region = "oss-cn-beijing"
endpoint = "https://oss-cn-beijing.aliyuncs.com"
access_key_id = "LTAI..."
access_key_secret = "secret..."
"#;

    let config: StorageConfig = toml::from_str(toml_str).expect("Failed to parse OSS TOML config");

    match config {
        StorageConfig::Oss {
            bucket,
            root,
            region,
            endpoint,
            ..
        } => {
            assert_eq!(bucket, "my-oss-bucket");
            assert_eq!(root, "/data");
            assert_eq!(region, "oss-cn-beijing");
            assert_eq!(endpoint, "https://oss-cn-beijing.aliyuncs.com");
        }
        _ => panic!("Expected OSS storage config"),
    }
}

#[test]
fn test_fs_config_parsing() {
    let toml_str = r#"
type = "fs"
root = "/custom/path"
"#;

    let config: StorageConfig = toml::from_str(toml_str).expect("Failed to parse FS TOML config");

    match config {
        StorageConfig::Fs { root } => {
            assert_eq!(root, "/custom/path");
        }
        _ => panic!("Expected FS storage config"),
    }
}
