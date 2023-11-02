use webrtc::rtcp::packet::Packet;
use webrtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtcp::payload_feedbacks::slice_loss_indication::SliceLossIndication;

#[derive(Debug, Clone, Copy)]
pub(crate) enum RtcpMessage {
    FullIntraRequest,
    PictureLossIndication,
    SliceLossIndication,
}

impl RtcpMessage {
    pub(crate) fn from_rtcp_packet(packet: Box<dyn Packet + Send + Sync>) -> Option<Self> {
        let any = packet.as_any();
        if any.downcast_ref::<FullIntraRequest>().is_some() {
            return Some(RtcpMessage::FullIntraRequest);
        } else if any.downcast_ref::<PictureLossIndication>().is_some() {
            return Some(RtcpMessage::PictureLossIndication);
        } else if any.downcast_ref::<SliceLossIndication>().is_some() {
            return Some(RtcpMessage::SliceLossIndication);
        }
        None
    }

    pub(crate) fn to_rtcp_packet(self, ssrc: u32) -> Box<dyn Packet + Send + Sync> {
        match self {
            RtcpMessage::FullIntraRequest => Box::new(FullIntraRequest {
                sender_ssrc: 0,
                media_ssrc: ssrc,
                fir: vec![],
            }),
            RtcpMessage::PictureLossIndication => Box::new(PictureLossIndication {
                sender_ssrc: 0,
                media_ssrc: ssrc,
            }),
            RtcpMessage::SliceLossIndication => Box::new(SliceLossIndication {
                sender_ssrc: 0,
                media_ssrc: ssrc,
                sli_entries: vec![],
            }),
        }
    }
}
