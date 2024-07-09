#[cfg(test)]
mod tests {

    use webrtc::{
        api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
        ice_transport::ice_server::RTCIceServer,
        interceptor::registry::Registry,
        peer_connection::configuration::RTCConfiguration,
    };

    use std::sync::Arc;
    use webrtc::ice_transport::ice_credential_type::RTCIceCredentialType;
    use webrtc::peer_connection::policy::bundle_policy::RTCBundlePolicy;
    use webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy;
    use webrtc::peer_connection::policy::rtcp_mux_policy::RTCRtcpMuxPolicy;

    #[tokio::test]
    async fn test_set_get_configuration() {
        let mut media_engine = MediaEngine::default();
        let registry = Registry::new();

        register_default_interceptors(registry, &mut media_engine)
            .expect("Failed to register default interceptors");

        let registry = Registry::default();

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        // constrct config
        let initial_config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                username: "".to_string(),
                credential: "".to_string(),
                credential_type: RTCIceCredentialType::Unspecified,
            }],
            ..Default::default()
        };

        // construct peer
        let peer = Arc::new(
            api.new_peer_connection(initial_config.clone())
                .await
                .expect("Failed to create RTCPeerConnection"),
        );

        let new_config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec![
                    "turn:turn.22333.fun".to_string(),
                    "turn:cn.22333.fun".to_string(),
                ],
                username: "live777".to_string(),
                credential: "live777".to_string(),
                credential_type: RTCIceCredentialType::Password,
            }],
            ..Default::default()
        };

        // set new config
        peer.set_configuration(new_config.clone())
            .await
            .expect("Failed to set configuration");

        //  validate
        let updated_config = peer.get_configuration().await;
        assert_eq!(updated_config.ice_servers.len(), 1);
        assert_eq!(
            updated_config.ice_servers[0].urls,
            vec![
                "turn:turn.22333.fun".to_string(),
                "turn:cn.22333.fun".to_string()
            ]
        );
        assert_eq!(updated_config.ice_servers[0].username, "live777");
        assert_eq!(updated_config.ice_servers[0].credential, "live777");
        assert_eq!(
            updated_config.ice_servers[0].credential_type,
            RTCIceCredentialType::Password
        );
        assert_eq!(
            updated_config.ice_transport_policy,
            RTCIceTransportPolicy::Unspecified
        );
        assert_eq!(updated_config.bundle_policy, RTCBundlePolicy::Unspecified);
        assert_eq!(
            updated_config.rtcp_mux_policy,
            RTCRtcpMuxPolicy::Unspecified
        );
        assert!(updated_config.peer_identity.is_empty());
        assert!(updated_config.certificates.is_empty());
        assert_eq!(updated_config.ice_candidate_pool_size, 0);
    }
}
