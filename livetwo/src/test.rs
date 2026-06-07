#[cfg(test)]
mod tests {
    use rtc::peer_connection::{
        RTCPeerConnectionBuilder,
        configuration::{
            RTCBundlePolicy, RTCConfigurationBuilder, RTCIceServer, RTCIceTransportPolicy,
            RTCRtcpMuxPolicy,
        },
    };

    #[test]
    fn test_set_get_configuration() {
        let initial_config = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                username: "".to_string(),
                credential: "".to_string(),
            }])
            .build();

        let mut peer = RTCPeerConnectionBuilder::new()
            .with_configuration(initial_config)
            .build()
            .expect("Failed to create RTCPeerConnection");

        let new_config = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![RTCIceServer {
                urls: vec![
                    "turn:turn.22333.fun".to_string(),
                    "turn:cn.22333.fun".to_string(),
                ],
                username: "live777_username".to_string(),
                credential: "live777_password".to_string(),
            }])
            .build();

        peer.set_configuration(new_config.clone())
            .expect("Failed to set configuration");

        let updated_config = peer.get_configuration();
        assert_eq!(updated_config.ice_servers().len(), 1);
        assert_eq!(
            updated_config.ice_servers()[0].urls,
            vec![
                "turn:turn.22333.fun".to_string(),
                "turn:cn.22333.fun".to_string()
            ]
        );
        assert_eq!(updated_config.ice_servers()[0].username, "live777_username");
        assert_eq!(
            updated_config.ice_servers()[0].credential,
            "live777_password"
        );
        assert_eq!(
            updated_config.ice_transport_policy(),
            RTCIceTransportPolicy::Unspecified
        );
        assert_eq!(updated_config.bundle_policy(), RTCBundlePolicy::Unspecified);
        assert_eq!(
            updated_config.rtcp_mux_policy(),
            RTCRtcpMuxPolicy::Unspecified
        );
        assert!(updated_config.peer_identity().is_empty());
        assert!(updated_config.certificates().is_empty());
        assert_eq!(updated_config.ice_candidate_pool_size(), 0);
    }
}
