pub async fn shutdown_signal() {
    let _str = signal::wait_for_stop_signal().await;
}
