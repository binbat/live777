use chrono::Datelike;
use std::path::Path;

/// Generate hierarchical storage path based on stream name and timestamp
/// Format: {stream}/{year}/{month}/{day}/{filename}
pub fn generate_path(stream: &str, timestamp: i64, filename: &str) -> String {
    let dt =
        chrono::DateTime::from_timestamp(timestamp / 1_000_000, 0).unwrap_or_else(chrono::Utc::now);

    format!(
        "{}/{:04}/{:02}/{:02}/{}",
        stream,
        dt.year_ce().1,
        dt.month(),
        dt.day(),
        filename
    )
}

/// Extract directory path from full storage path
pub fn get_directory(path: &str) -> Option<&str> {
    Path::new(path).parent()?.to_str()
}

/// Validate storage path format
pub fn validate_path(path: &str) -> bool {
    !path.is_empty() && !path.contains("..") && !path.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_path() {
        // 2024-01-15 12:00:00 UTC
        let timestamp = 1_705_320_000_000_000_i64;
        let path = generate_path("camera01", timestamp, "segment_001.m4s");
        assert_eq!(path, "camera01/2024/01/15/segment_001.m4s");
    }

    #[test]
    fn test_get_directory() {
        let path = "camera01/2024/01/15/segment_001.m4s";
        assert_eq!(get_directory(path), Some("camera01/2024/01/15"));
    }

    #[test]
    fn test_validate_path() {
        assert!(validate_path("camera01/2024/01/15/segment.m4s"));
        assert!(!validate_path("../camera01/segment.m4s"));
        assert!(!validate_path("/absolute/path"));
        assert!(!validate_path(""));
    }
}
