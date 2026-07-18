use std::net::SocketAddr;

use anyhow::{Result, bail};

use super::{Source, SourceHandle};
use crate::profile::{MediaProfile, VideoCodec};

/// Placeholder for a GStreamer VP8 RTP source.
///
/// To implement this for real, add a dependency on `gstreamer` and build a
/// pipeline such as:
///
/// ```text
/// videotestsrc ! vp8enc ! rtpvp8pay ! udpsink host=127.0.0.1 port=<port>
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct GstreamerVp8Source;

impl Source for GstreamerVp8Source {
    fn name(&self) -> String {
        "gstreamer-vp8".to_string()
    }

    fn profile(&self) -> MediaProfile {
        MediaProfile::video_only(VideoCodec::Vp8)
    }

    fn start(&self, _target_addr: SocketAddr) -> Result<Box<dyn SourceHandle>> {
        bail!("GStreamer VP8 source is not implemented yet")
    }

    fn sdp(&self, listen_addr: SocketAddr) -> String {
        format!(
            "v=0\r\n\
             o=- 0 0 IN IP4 127.0.0.1\r\n\
             s=gstreamer VP8 test stream\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n\
             m=video {} RTP/AVP 96\r\n\
             a=rtpmap:96 VP8/90000\r\n",
            listen_addr.port()
        )
    }
}
