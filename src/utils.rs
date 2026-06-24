use tracing::debug;

pub async fn shutdown_signal() {
    let str = signal::wait_for_stop_signal().await;
    debug!("Received signal: {}", str);
}
