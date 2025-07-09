use super::{CodecAdapter, TrackKind};
use bytes::Bytes;

pub struct Vp8Adapter {
    timescale: u32,
}

impl Vp8Adapter {
    pub fn new() -> Self {
        Self { timescale: 90_000 }
    }
}

impl CodecAdapter for Vp8Adapter {
    fn kind(&self) -> TrackKind {
        TrackKind::Video
    }

    fn timescale(&self) -> u32 {
        self.timescale
    }

    fn ready(&self) -> bool {
        false
    }

    fn convert_frame(&mut self, _frame: &Bytes) -> (Vec<u8>, bool, bool) {
        // TODO: Implement VP8 Annex-B → IVF → length prefix conversion
        (Vec::new(), false, false)
    }

    fn codec_config(&self) -> Option<Vec<Vec<u8>>> {
        None
    }

    fn codec_string(&self) -> Option<String> {
        Some("vp8".to_string())
    }
}
