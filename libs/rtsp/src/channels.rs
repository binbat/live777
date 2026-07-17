use tokio::sync::mpsc::{Receiver, Sender, channel};

use crate::types::SessionMode;

pub type InterleavedData = (u8, Vec<u8>);
pub type InterleavedChannel = (Sender<InterleavedData>, Receiver<InterleavedData>);

pub const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

pub struct RtspChannels {
    recv_tx: Sender<InterleavedData>,
    recv_rx: Option<Receiver<InterleavedData>>,

    send_tx: Sender<InterleavedData>,
    send_rx: Option<Receiver<InterleavedData>>,
}

impl RtspChannels {
    pub fn new() -> Self {
        let (recv_tx, recv_rx) = channel::<InterleavedData>(DEFAULT_CHANNEL_CAPACITY);
        let (send_tx, send_rx) = channel::<InterleavedData>(DEFAULT_CHANNEL_CAPACITY);

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
            SessionMode::Mixed => panic!("Mixed mode must be resolved before allocating channels"),
        }
    }

    pub fn get_internal_rx(&mut self, mode: SessionMode) -> Receiver<InterleavedData> {
        match mode {
            SessionMode::Pull => self.recv_rx.take().expect("recv_rx already taken"),
            SessionMode::Push => self.send_rx.take().expect("send_rx already taken"),
            SessionMode::Mixed => panic!("Mixed mode must be resolved before allocating channels"),
        }
    }

    pub fn get_sender(&self, mode: SessionMode) -> Sender<InterleavedData> {
        match mode {
            SessionMode::Pull => self.recv_tx.clone(),
            SessionMode::Push => self.send_tx.clone(),
            SessionMode::Mixed => panic!("Mixed mode must be resolved before allocating channels"),
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
        assert!(pull_sender.try_send((0, vec![1])).is_ok());

        let push_sender = channels.get_sender(SessionMode::Push);
        assert!(push_sender.try_send((1, vec![2])).is_ok());
    }
}
