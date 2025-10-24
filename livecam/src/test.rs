#[cfg(test)]
mod tests {
    use crate::{AppState, LiveCamManager, auth::create_auth_router, config::Config};
    use axum::http::StatusCode;
    use axum_test::TestServer;
    use serde_json::json;
    use std::sync::{Arc, RwLock};

    fn create_test_config() -> Config {
        let mut config = Config::default();
        config.auth.password_hash = "$argon2id$v=19$m=19456,t=2,p=1$uv2fQ0ruVrBOhs9j1axP2Q$JzoOes/WQbWW8AKrcZb0BppCaMRsjq3dJ8ndSnKfR4U".to_string();
        config
    }

    fn create_test_webrtc_api() -> webrtc::api::API {
        use webrtc::api::{APIBuilder, media_engine::MediaEngine};
        use webrtc::interceptor::registry::Registry;

        let mut m = MediaEngine::default();
        m.register_default_codecs().unwrap();
        let registry = Registry::new();

        APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build()
    }

    #[test]
    fn test_password_hash_verification() {
        use argon2::{Argon2, PasswordHash, PasswordVerifier};

        let config = create_test_config();
        let stored_hash = &config.auth.password_hash;
        let password = "admin";

        let parsed_hash = PasswordHash::new(stored_hash).unwrap();
        let result = Argon2::default().verify_password(password.as_bytes(), &parsed_hash);

        assert!(result.is_ok(), "Password verification should succeed");
    }

    #[tokio::test]
    async fn test_login_success() {
        let config = Arc::new(RwLock::new(create_test_config()));
        let manager = LiveCamManager::new(config.clone(), Arc::new(create_test_webrtc_api()));

        let app_state = AppState { config, manager };
        let app = create_auth_router().with_state(app_state);
        let server = TestServer::new(app).unwrap();

        let response = server
            .post("/api/login")
            .json(&json!({
                "username": "admin",
                "password": "admin"
            }))
            .await;

        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert!(body.get("token").is_some());
    }

    #[tokio::test]
    async fn test_login_invalid_credentials() {
        let config = Arc::new(RwLock::new(create_test_config()));
        let manager = LiveCamManager::new(config.clone(), Arc::new(create_test_webrtc_api()));

        let app_state = AppState { config, manager };
        let app = create_auth_router().with_state(app_state);
        let server = TestServer::new(app).unwrap();

        let response = server
            .post("/api/login")
            .json(&json!({
                "username": "admin",
                "password": "wrongpassword"
            }))
            .await;

        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_change_password() {
        let config = Arc::new(RwLock::new(create_test_config()));
        let manager = LiveCamManager::new(config.clone(), Arc::new(create_test_webrtc_api()));

        let app_state = AppState {
            config: config.clone(),
            manager,
        };
        let app = create_auth_router().with_state(app_state);
        let server = TestServer::new(app).unwrap();

        let login_response = server
            .post("/api/login")
            .json(&json!({
                "username": "admin",
                "password": "admin"
            }))
            .await;

        assert_eq!(login_response.status_code(), StatusCode::OK);
        let login_body: serde_json::Value = login_response.json();
        let token = login_body["token"].as_str().unwrap();

        let response = server
            .post("/api/user/password")
            .add_header("Authorization", format!("Bearer {}", token))
            .json(&json!({
                "new_password": "newpassword123"
            }))
            .await;

        let updated_hash = {
            let config_read = config.read().unwrap();
            config_read.auth.password_hash.clone()
        };

        use argon2::{Argon2, PasswordHash, PasswordVerifier};
        let parsed_hash = PasswordHash::new(&updated_hash).unwrap();
        let result = Argon2::default().verify_password("newpassword123".as_bytes(), &parsed_hash);
        assert!(result.is_ok(), "New password should be valid");

        assert!(
            response.status_code() == StatusCode::OK
                || response.status_code() == StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use crate::{AppState, LiveCamManager, config::Config};
    use axum::http::StatusCode;
    use axum_test::TestServer;
    use std::sync::{Arc, RwLock};
    use webrtc::api::{APIBuilder, media_engine::MediaEngine};
    use webrtc::interceptor::registry::Registry;

    fn create_test_config() -> Config {
        Config::default()
    }

    fn create_test_webrtc_api() -> webrtc::api::API {
        let mut m = MediaEngine::default();
        m.register_default_codecs().unwrap();
        let registry = Registry::new();

        APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build()
    }

    #[tokio::test]
    async fn test_health_check() {
        let config = Arc::new(RwLock::new(create_test_config()));
        let manager = LiveCamManager::new(config.clone(), Arc::new(create_test_webrtc_api()));

        let app_state = AppState { config, manager };
        let app = axum::Router::new()
            .route("/api/health", axum::routing::get(crate::health_check))
            .with_state(app_state);

        let server = TestServer::new(app).unwrap();

        let response = server.get("/api/health").await;

        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["service"], "livecam");
        assert!(body["timestamp"].is_string());
        assert!(body["version"].is_string());
    }
}

#[cfg(test)]
mod rtp_receiver_tests {
    use crate::rtp_receiver;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

    #[tokio::test]
    async fn test_rtp_receiver_shutdown() {
        let track = Arc::new(TrackLocalStaticRTP::new(
            webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: "video/H264".to_string(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: vec![],
            },
            "test".to_string(),
            "test".to_string(),
        ));

        let (tx, rx) = mpsc::channel(1);

        let handle = tokio::spawn(async move { rtp_receiver::start(0, track, rx).await });

        tx.send(()).await.unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
