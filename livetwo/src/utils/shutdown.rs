use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tracing::{info, warn};

#[derive(Clone)]
pub struct ShutdownSignal {
    notify: Arc<broadcast::Sender<()>>,
    is_shutdown: Arc<Mutex<bool>>,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        let (notify, _) = broadcast::channel(1);
        Self {
            notify: Arc::new(notify),
            is_shutdown: Arc::new(Mutex::new(false)),
        }
    }

    /// Trigger shutdown signal
    pub async fn shutdown(&self) {
        let mut is_shutdown = self.is_shutdown.lock().await;
        if !*is_shutdown {
            *is_shutdown = true;
            info!("Shutdown signal triggered");
            let _ = self.notify.send(());
        }
    }

    /// Subscribe to shutdown signal
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.notify.subscribe()
    }

    /// Check if already shutdown
    pub async fn is_shutdown(&self) -> bool {
        *self.is_shutdown.lock().await
    }

    /// Wait for shutdown signal
    pub async fn wait(&self) {
        let mut rx = self.subscribe();
        let _ = rx.recv().await;
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Wait for system signal or manual shutdown signal
pub async fn wait_for_shutdown(
    shutdown: ShutdownSignal,
    mut complete_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) -> String {
    tokio::select! {
        _ = complete_rx.recv() => {
            info!("Received completion signal");
            shutdown.shutdown().await;
            "completion".to_string()
        }
        msg = signal::wait_for_stop_signal() => {
            warn!("Received system signal: {}", msg);
            shutdown.shutdown().await;
            msg.to_string()
        }
        _ = shutdown.wait() => {
            info!("Received shutdown signal");
            "shutdown".to_string()
        }
    }
}
