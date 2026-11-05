//! Shared helpers for the workspace integration tests.

#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Once;

use tokio::net::TcpListener;

pub async fn shutdown_signal() {
    let _str = signal::wait_for_stop_signal().await;
}

/// Locate one of the workspace's binaries built alongside this test run.
///
/// Cargo sets `CARGO_BIN_EXE_<name>` for integration tests of the package
/// that builds the binary; fall back to the conventional target directory
/// layout when the test binary is run from somewhere else.
#[macro_export]
macro_rules! bin_path {
    ($name:literal) => {
        option_env!(concat!("CARGO_BIN_EXE_", $name)).map_or_else(
            || {
                let mut path = std::env::current_exe().unwrap();
                path.pop(); // deps/
                path.pop(); // <profile>/
                path.push($name);
                path
            },
            PathBuf::from,
        )
    };
}

/// Pin ICE to loopback and install the test tracing subscriber once per
/// process. Both WebRTC peers and the spawned tool children inherit the
/// variable, so CI runners cannot pick an unroutable interface.
pub fn init_test_environment() {
    static TRACING_INIT: Once = Once::new();
    TRACING_INIT.call_once(|| {
        unsafe {
            std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
        }

        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "live777=info,liveion=info,livetwo=info,libwish=info".to_string());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

/// Start liveion on an ephemeral loopback port and return its address.
pub async fn start_liveion(token: Option<&str>) -> SocketAddr {
    start_liveion_with(token, |_| {}).await
}

/// Start liveion with per-test configuration on top of the shared test
/// defaults (open CORS, loopback ICE, optional bearer token).
pub async fn start_liveion_with<F>(token: Option<&str>, configure: F) -> SocketAddr
where
    F: FnOnce(&mut liveion::config::Config),
{
    init_test_environment();
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let mut config = liveion::config::Config::default();
    config.http.cors = vec!["*".to_string()].into();
    if let Some(token) = token {
        config.auth.tokens = vec![token.to_string()];
    }
    configure(&mut config);
    tokio::spawn(liveion::serve(config, listener, shutdown_signal()));
    addr
}
