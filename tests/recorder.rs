use liveion::config::RecorderConfig;
use storage::StorageConfig;

#[test]
fn test_recorder_config_serialization() {
    let config = RecorderConfig {
        auto_streams: vec!["stream1".to_string(), "stream2".to_string()],
        storage: StorageConfig::S3 {
            bucket: "my-bucket".to_string(),
            root: "/recordings".to_string(),
            region: Some("us-west-2".to_string()),
            endpoint: None,
            access_key_id: Some("AKIA...".to_string()),
            secret_access_key: Some("secret...".to_string()),
            session_token: None,
            disable_config_load: false,
            enable_virtual_host_style: true,
        },
        liveman: None,
    };

    let serialized = toml::to_string(&config).expect("Failed to serialize config");
    let deserialized: RecorderConfig =
        toml::from_str(&serialized).expect("Failed to deserialize config");

    match (&config.storage, &deserialized.storage) {
        (StorageConfig::S3 { bucket: b1, .. }, StorageConfig::S3 { bucket: b2, .. }) => {
            assert_eq!(b1, b2, "Bucket names should match");
        }
        _ => panic!("Storage config type mismatch"),
    }
}

#[test]
fn test_default_recorder_config() {
    let config = RecorderConfig::default();
    assert!(config.auto_streams.is_empty());
    assert!(config.liveman.is_none());

    match config.storage {
        StorageConfig::Fs { root } => {
            assert_eq!(root, "./storage");
        }
        _ => panic!("Default storage should be FS"),
    }
}

#[test]
fn test_recorder_toml_config_parsing() {
    let toml_str = r#"
auto_streams = ["*"]

[storage]
type = "s3"
bucket = "test-bucket"
root = "/recordings"
region = "us-east-1"
access_key_id = "test-key"
secret_access_key = "test-secret"
enable_virtual_host_style = true
"#;

    let config: RecorderConfig = toml::from_str(toml_str).expect("Failed to parse TOML config");
    assert_eq!(config.auto_streams, vec!["*"]);

    match config.storage {
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
fn test_recorder_oss_config_parsing() {
    let toml_str = r#"
auto_streams = []

[storage]
type = "oss"
bucket = "my-oss-bucket"
root = "/data"
region = "oss-cn-beijing"
endpoint = "https://oss-cn-beijing.aliyuncs.com"
access_key_id = "LTAI..."
access_key_secret = "secret..."
"#;

    let config: RecorderConfig = toml::from_str(toml_str).expect("Failed to parse OSS TOML config");

    match config.storage {
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
