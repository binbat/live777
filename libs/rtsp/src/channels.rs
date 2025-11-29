use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::types::SessionMode;

pub struct RtspChannels {
    recv_tx: UnboundedSender<(u8, Vec<u8>)>,
    recv_rx: Option<UnboundedReceiver<(u8, Vec<u8>)>>,

    send_tx: UnboundedSender<(u8, Vec<u8>)>,
    send_rx: Option<UnboundedReceiver<(u8, Vec<u8>)>>,
}

impl RtspChannels {
    pub fn new() -> Self {
        let (recv_tx, recv_rx) = unbounded_channel::<(u8, Vec<u8>)>();
        let (send_tx, send_rx) = unbounded_channel::<(u8, Vec<u8>)>();

        Self {
            recv_tx,
            recv_rx: Some(recv_rx),
            send_tx,
            send_rx: Some(send_rx),
        }
    }

    pub fn get_channels(
        &mut self,
        mode: SessionMode,
    ) -> (
        UnboundedSender<(u8, Vec<u8>)>,
        UnboundedReceiver<(u8, Vec<u8>)>,
    ) {
        match mode {
            SessionMode::Pull => {
                let send_rx = self.send_rx.take().expect("send_rx already taken");
                (self.recv_tx.clone(), send_rx)
            }
            SessionMode::Push => {
                let recv_rx = self.recv_rx.take().expect("recv_rx already taken");
                (self.send_tx.clone(), recv_rx)
            }
        }
    }

    pub fn get_internal_rx(&mut self, mode: SessionMode) -> UnboundedReceiver<(u8, Vec<u8>)> {
        match mode {
            SessionMode::Pull => self.recv_rx.take().expect("recv_rx already taken"),
            SessionMode::Push => self.send_rx.take().expect("send_rx already taken"),
        }
    }

    pub fn get_sender(&self, mode: SessionMode) -> UnboundedSender<(u8, Vec<u8>)> {
        match mode {
            SessionMode::Pull => self.recv_tx.clone(),
            SessionMode::Push => self.send_tx.clone(),
        }
    }
}

impl Default for RtspChannels {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channels_creation() {
        let channels = RtspChannels::new();
        assert!(channels.recv_rx.is_some());
        assert!(channels.send_rx.is_some());
    }

    #[tokio::test]
    async fn test_pull_mode_channels() {
        let mut channels = RtspChannels::new();
        let (tx, mut rx) = channels.get_channels(SessionMode::Pull);

        tx.send((0, vec![1, 2, 3])).unwrap();
        let (channel, data) = rx.recv().await.unwrap();
        assert_eq!(channel, 0);
        assert_eq!(data, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_push_mode_channels() {
        let mut channels = RtspChannels::new();
        let (tx, mut rx) = channels.get_channels(SessionMode::Push);

        tx.send((1, vec![4, 5, 6])).unwrap();
        let (channel, data) = rx.recv().await.unwrap();
        assert_eq!(channel, 1);
        assert_eq!(data, vec![4, 5, 6]);
    }
}
