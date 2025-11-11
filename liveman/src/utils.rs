pub fn timestamp_dir() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

pub fn extract_timestamp_from_record_dir(record_dir: &str) -> Option<String> {
    record_dir
        .rsplit('/')
        .find(|segment| {
            !segment.is_empty()
                && segment.len() >= 10
                && segment.chars().all(|c| c.is_ascii_digit())
        })
        .map(|s| s.to_string())
}
