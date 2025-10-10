#[cfg(all(test, feature = "recorder"))]
mod tests {
    use super::super::*;
    use crate::recorder::segmenter::Segmenter;
    use bytes::Bytes;
    use opendal::Operator;
    use opendal::services::Fs;
    use tempfile::TempDir;
    use tokio::time::{Duration, sleep};

    // Helper to build a minimal H264 frame that includes SPS, PPS and an IDR slice
    fn make_h264_idr_frame() -> Bytes {
        // Annex-B start codes followed by nal type bytes
        let mut buf = Vec::new();
        // SPS (nal type 7)
        buf.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0xE0, 0x1E, 0x8D, 0x68, 0x50]);
        // PPS (nal type 8)
        buf.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xCE, 0x06, 0xE2]);
        // IDR slice (nal type 5) – payload bytes arbitrary
        buf.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00]);
        Bytes::from(buf)
    }

    #[tokio::test]
    async fn test_segmenter_writes_init_and_manifest() {
        // Prepare temporary filesystem backend
        let tmp = TempDir::new().expect("Failed to create temp dir");
        let tmp_path = tmp.path().to_str().unwrap().to_string();

        let mut builder = Fs::default();
        builder.root(&tmp_path);
        let op: Operator = Operator::new(builder).unwrap().finish();

        // Instantiate a Segmenter
        let stream_name = "test_stream".to_string();
        let prefix = "dash".to_string();
        let mut seg = Segmenter::new(op.clone(), stream_name.clone(), prefix.clone())
            .await
            .expect("Failed to create segmenter");

        // Feed one IDR frame which should trigger writer init
        let frame = make_h264_idr_frame();
        seg.push_h264(frame, 3000).await.expect("push failed");

        // Allow async background write task to finish
        sleep(Duration::from_millis(200)).await;

        // The init segment and manifest should exist
        let init_path = format!("{}/init.m4s", prefix);
        let manifest_path = format!("{}/manifest.mpd", prefix);
        assert!(
            op.is_exist(&init_path).await.unwrap(),
            "init.m4s not written"
        );
        assert!(
            op.is_exist(&manifest_path).await.unwrap(),
            "manifest.mpd not written"
        );
    }

    #[test]
    fn test_should_record_glob() {
        let patterns = vec!["live/*".to_string(), "demo".to_string()];
        assert!(super::should_record(&patterns, "live/abc"));
        assert!(!super::should_record(&patterns, "other/stream"));
        assert!(super::should_record(&patterns, "demo"));
    }
}
