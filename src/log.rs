const WEBRTC_ICE_NOISE_FILTER: &str = "webrtc::peer_connection::driver=off";

pub fn set(env_filter: String) {
    use tracing_subscriber::EnvFilter;
    let env_filter = default_filter(&env_filter);
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or(EnvFilter::new(env_filter)))
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .init();
}

fn default_filter(env_filter: &str) -> String {
    if env_filter.contains(WEBRTC_ICE_NOISE_FILTER) {
        env_filter.to_owned()
    } else {
        format!("{env_filter},{WEBRTC_ICE_NOISE_FILTER}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_filter_suppresses_webrtc_ice_driver_noise() {
        let filter = super::default_filter("live777=debug,webrtc=error");

        assert!(filter.contains("live777=debug"));
        assert!(filter.contains("webrtc=error"));
        assert!(filter.contains("webrtc::peer_connection::driver=off"));
    }
}
