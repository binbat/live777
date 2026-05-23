use rtc::rtcp::packet::Packet;
use rtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtcp::payload_feedbacks::slice_loss_indication::SliceLossIndication;

#[derive(Debug, Clone, Copy)]
pub enum RtcpMessage {
    _FullIntraRequest,
    PictureLossIndication,
    _SliceLossIndication,
}

impl RtcpMessage {
    pub(crate) fn _from_rtcp_packet(packet: Box<dyn Packet + Send + Sync>) -> Option<Self> {
        let any = packet.as_any();
        if any.downcast_ref::<FullIntraRequest>().is_some() {
            return Some(RtcpMessage::_FullIntraRequest);
        } else if any.downcast_ref::<PictureLossIndication>().is_some() {
            return Some(RtcpMessage::PictureLossIndication);
        } else if any.downcast_ref::<SliceLossIndication>().is_some() {
            return Some(RtcpMessage::_SliceLossIndication);
        }
        None
    }

    pub(crate) fn to_rtcp_packet(self, ssrc: u32) -> Box<dyn Packet + Send + Sync> {
        match self {
            RtcpMessage::_FullIntraRequest => Box::new(FullIntraRequest {
                sender_ssrc: 0,
                media_ssrc: ssrc,
                fir: vec![],
            }),
            RtcpMessage::PictureLossIndication => Box::new(PictureLossIndication {
                sender_ssrc: 0,
                media_ssrc: ssrc,
            }),
            RtcpMessage::_SliceLossIndication => Box::new(SliceLossIndication {
                sender_ssrc: 0,
                media_ssrc: ssrc,
                sli_entries: vec![],
            }),
        }
    }
}
