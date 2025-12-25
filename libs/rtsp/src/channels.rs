use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::types::SessionMode;

pub type InterleavedData = (u8, Vec<u8>);
pub type InterleavedChannel = (
    UnboundedSender<InterleavedData>,
    UnboundedReceiver<InterleavedData>,
);

pub struct RtspChannels {
    recv_tx: UnboundedSender<InterleavedData>,
    recv_rx: Option<UnboundedReceiver<InterleavedData>>,

    send_tx: UnboundedSender<InterleavedData>,
    send_rx: Option<UnboundedReceiver<InterleavedData>>,
}

impl RtspChannels {
    pub fn new() -> Self {
        let (recv_tx, recv_rx) = unbounded_channel::<InterleavedData>();
        let (send_tx, send_rx) = unbounded_channel::<InterleavedData>();

        Self {
            recv_tx,
            recv_rx: Some(recv_rx),
            send_tx,
            send_rx: Some(send_rx),
        }
    }

    pub fn get_channels(&mut self, mode: SessionMode) -> InterleavedChannel {
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

    pub fn get_internal_rx(&mut self, mode: SessionMode) -> UnboundedReceiver<InterleavedData> {
        match mode {
            SessionMode::Pull => self.recv_rx.take().expect("recv_rx already taken"),
            SessionMode::Push => self.send_rx.take().expect("send_rx already taken"),
        }
    }

    pub fn get_sender(&self, mode: SessionMode) -> UnboundedSender<InterleavedData> {
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

    #[test]
    fn test_get_sender() {
        let channels = RtspChannels::new();

        let pull_sender = channels.get_sender(SessionMode::Pull);
        assert!(pull_sender.send((0, vec![1])).is_ok());

        let push_sender = channels.get_sender(SessionMode::Push);
        assert!(push_sender.send((1, vec![2])).is_ok());
    }
}
