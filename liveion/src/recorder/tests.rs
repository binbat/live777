#[cfg(all(test, feature = "recorder"))]
mod tests {
    use crate::recorder::segmenter::{RecordingMediaOutcome, Segmenter};
    use crate::recorder::should_record;
    use bytes::Bytes;
    use opendal::Operator;
    use opendal::services::Fs;
    use tempfile::TempDir;

    async fn test_segmenter(prefix: &str) -> (TempDir, Operator, Segmenter) {
        let tmp = TempDir::new().expect("create temp dir");
        let builder = Fs::default().root(tmp.path().to_str().unwrap());
        let op = Operator::new(builder).unwrap().finish();
        let seg = Segmenter::new(
            op.clone(),
            "test_stream".to_string(),
            prefix.to_string(),
            None,
            None,
        )
        .await
        .expect("create segmenter");
        (tmp, op, seg)
    }

    fn h264_idr_with_config() -> Bytes {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0xE0, 0x1E, 0x8D, 0x68, 0x50]);
        buf.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xCE, 0x06, 0xE2]);
        buf.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00]);
        Bytes::from(buf)
    }

    #[tokio::test]
    async fn h264_flush_waits_for_init_segment_and_manifest() {
        let (_tmp, op, mut seg) = test_segmenter("h264").await;

        seg.expect_video_track("video/h264", Some(96), Some(1234), None);
        seg.push_h264(h264_idr_with_config(), 3000).await.unwrap();
        seg.flush().await.unwrap();

        assert!(op.exists("h264/v_init.m4s").await.unwrap());
        assert!(op.exists("h264/v_seg_0001.m4s").await.unwrap());
        assert!(op.exists("h264/manifest.mpd").await.unwrap());
        assert_eq!(seg.media_outcome(), RecordingMediaOutcome::Complete);
    }

    #[tokio::test]
    async fn audio_only_recording_is_complete_without_video() {
        let (_tmp, op, mut seg) = test_segmenter("audio").await;

        seg.push_opus(Bytes::from_static(&[0x11, 0x22, 0x33]), 960)
            .await
            .unwrap();
        seg.flush().await.unwrap();

        assert!(op.exists("audio/a_init.m4s").await.unwrap());
        assert!(op.exists("audio/a_seg_0001.m4s").await.unwrap());
        assert!(op.exists("audio/manifest.mpd").await.unwrap());
        assert_eq!(seg.media_outcome(), RecordingMediaOutcome::Complete);
    }

    #[tokio::test]
    async fn expected_video_without_video_segments_is_degraded() {
        let (_tmp, _op, mut seg) = test_segmenter("video-missing").await;

        seg.expect_video_track("video/h264", Some(96), Some(1234), None);
        seg.push_opus(Bytes::from_static(&[0x11, 0x22, 0x33]), 960)
            .await
            .unwrap();
        seg.flush().await.unwrap();

        assert_eq!(seg.media_outcome(), RecordingMediaOutcome::Degraded);
    }

    #[tokio::test]
    async fn vp9_keyframe_initializes_video_without_codec_config_blob() {
        let (_tmp, op, mut seg) = test_segmenter("vp9").await;

        seg.expect_video_track("video/vp9", Some(98), Some(1234), None);
        seg.configure_video_from_track_metadata("video/vp9", None, Some((640, 360)));
        seg.push_vp9(Bytes::from_static(&[0x82, 0x49, 0x83, 0x42]), 3000)
            .await
            .unwrap();
        seg.flush().await.unwrap();

        assert!(op.exists("vp9/v_init.m4s").await.unwrap());
        assert!(op.exists("vp9/v_seg_0001.m4s").await.unwrap());
        assert_eq!(seg.media_outcome(), RecordingMediaOutcome::Complete);
    }

    #[tokio::test]
    async fn av1_can_initialize_from_track_metadata_before_late_keyframe() {
        let (_tmp, op, mut seg) = test_segmenter("av1").await;

        seg.expect_video_track("video/av1", Some(99), Some(1234), None);
        seg.configure_video_from_track_metadata(
            "video/av1",
            Some(vec![vec![0x81, 0x08, 0, 0]]),
            Some((640, 360)),
        );
        seg.push_av1(Bytes::from_static(&[0x12, 0x00]), 3000)
            .await
            .unwrap();
        seg.flush().await.unwrap();

        assert!(op.exists("av1/v_init.m4s").await.unwrap());
        assert!(op.exists("av1/v_seg_0001.m4s").await.unwrap());
        assert_eq!(seg.media_outcome(), RecordingMediaOutcome::Complete);
    }

    #[tokio::test]
    async fn h265_can_initialize_from_complete_track_metadata() {
        let (_tmp, op, mut seg) = test_segmenter("h265-complete").await;

        seg.expect_video_track("video/h265", Some(100), Some(1234), None);
        seg.configure_video_from_track_metadata(
            "video/h265",
            Some(vec![vec![0x01, 0x01, 0x60, 0x00]]),
            Some((640, 360)),
        );
        seg.push_h265(Bytes::from_static(&[0, 0, 0, 1, 0x02, 0x01, 0x55]), 3000)
            .await
            .unwrap();
        seg.flush().await.unwrap();

        assert!(op.exists("h265-complete/v_init.m4s").await.unwrap());
        assert!(op.exists("h265-complete/v_seg_0001.m4s").await.unwrap());
        assert_eq!(seg.media_outcome(), RecordingMediaOutcome::Complete);
    }

    #[tokio::test]
    async fn h265_requires_vps_sps_and_pps_before_video_is_complete() {
        let (_tmp, _op, mut seg) = test_segmenter("h265").await;

        seg.expect_video_track("video/h265", Some(100), Some(1234), None);
        seg.push_opus(Bytes::from_static(&[0x11, 0x22]), 960)
            .await
            .unwrap();
        seg.flush().await.unwrap();

        assert_eq!(seg.media_outcome(), RecordingMediaOutcome::Degraded);
    }

    #[test]
    fn test_should_record_glob() {
        let patterns = vec!["live/*".to_string(), "demo".to_string()];
        assert!(should_record(&patterns, "live/abc"));
        assert!(!should_record(&patterns, "other/stream"));
        assert!(should_record(&patterns, "demo"));
    }
}
