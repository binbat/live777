#[macro_export]
macro_rules! new_broadcast_channel {
    ($capacity:expr) => {{
        let (sender, mut recv) = tokio::sync::broadcast::channel($capacity);
        tokio::spawn(async move { while recv.recv().await.is_ok() {} });
        sender
    }};
}
