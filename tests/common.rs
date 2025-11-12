use std::collections::BTreeSet;

pub async fn shutdown_signal() {
    let _str = signal::wait_for_stop_signal().await;
}

pub fn pick_port() -> u16 {
    portpicker::pick_unused_port().expect("failed to pick unused port")
}

pub fn pick_ports(count: usize) -> Vec<u16> {
    let mut ports = BTreeSet::new();
    while ports.len() < count {
        ports.insert(pick_port());
    }
    ports.into_iter().collect()
}
