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

#[cfg(test)]
mod tests {
    use super::*;
    use rtc::rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate;
    use rtc::rtcp::receiver_report::ReceiverReport;
    use rtc::rtcp::transport_feedbacks::transport_layer_cc::TransportLayerCc;

    #[test]
    fn bridge_accepts_keyframe_feedback() {
        assert!(matches!(
            RtcpMessage::_from_rtcp_packet(Box::new(PictureLossIndication {
                sender_ssrc: 1,
                media_ssrc: 2,
            })),
            Some(RtcpMessage::PictureLossIndication)
        ));
        assert!(matches!(
            RtcpMessage::_from_rtcp_packet(Box::new(FullIntraRequest {
                sender_ssrc: 1,
                media_ssrc: 2,
                fir: vec![],
            })),
            Some(RtcpMessage::_FullIntraRequest)
        ));
        assert!(matches!(
            RtcpMessage::_from_rtcp_packet(Box::new(SliceLossIndication {
                sender_ssrc: 1,
                media_ssrc: 2,
                sli_entries: vec![],
            })),
            Some(RtcpMessage::_SliceLossIndication)
        ));
    }

    #[test]
    fn bridge_rejects_path_specific_bandwidth_feedback() {
        assert!(
            RtcpMessage::_from_rtcp_packet(Box::new(TransportLayerCc::default())).is_none(),
            "downstream TWCC is path-specific and must not be bridged to the publisher"
        );
        assert!(
            RtcpMessage::_from_rtcp_packet(Box::new(ReceiverEstimatedMaximumBitrate::default()))
                .is_none(),
            "downstream REMB is path-specific and must not be bridged to the publisher"
        );
        assert!(
            RtcpMessage::_from_rtcp_packet(Box::new(ReceiverReport::default())).is_none(),
            "downstream RR is path-specific and must not be bridged to the publisher"
        );
    }
}
