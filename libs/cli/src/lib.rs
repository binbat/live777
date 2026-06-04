use std::{
    process::{Child, Command, Stdio},
    sync::Mutex,
};

use anyhow::{Result, anyhow};
use clap::ValueEnum;
use rtc::{
    peer_connection::configuration::media_engine::*,
    rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RtpCodecKind},
};

#[derive(Copy, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Codec {
    Vp8,
    Vp9,
    H264,
    H265,
    AV1,
    Opus,
    G722,
    PCMU,
    PCMA,
}

impl From<Codec> for RTCRtpCodec {
    fn from(val: Codec) -> Self {
        let video_rtcp_feedback = vec![
            RTCPFeedback {
                typ: "goog-remb".to_owned(),
                parameter: "".to_owned(),
            },
            RTCPFeedback {
                typ: "transport-cc".to_owned(),
                parameter: "".to_owned(),
            },
            RTCPFeedback {
                typ: "ccm".to_owned(),
                parameter: "fir".to_owned(),
            },
            RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: "".to_owned(),
            },
            RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: "pli".to_owned(),
            },
        ];
        match val {
            Codec::Vp8 => RTCRtpCodec {
                mime_type: MIME_TYPE_VP8.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: video_rtcp_feedback,
            },
            Codec::Vp9 => RTCRtpCodec {
                mime_type: MIME_TYPE_VP9.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_owned(),
                rtcp_feedback: video_rtcp_feedback,
            },
            Codec::H264 => RTCRtpCodec {
                mime_type: MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                        .to_owned(),
                rtcp_feedback: video_rtcp_feedback,
            },
            Codec::H265 => RTCRtpCodec {
                mime_type: MIME_TYPE_HEVC.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: video_rtcp_feedback,
            },
            Codec::AV1 => RTCRtpCodec {
                mime_type: MIME_TYPE_AV1.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_owned(),
                rtcp_feedback: video_rtcp_feedback,
            },
            Codec::Opus => RTCRtpCodec {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            Codec::G722 => RTCRtpCodec {
                mime_type: MIME_TYPE_G722.to_owned(),
                clock_rate: 8000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            Codec::PCMU => RTCRtpCodec {
                mime_type: MIME_TYPE_PCMU.to_owned(),
                clock_rate: 8000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            Codec::PCMA => RTCRtpCodec {
                mime_type: MIME_TYPE_PCMA.to_owned(),
                clock_rate: 8000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
        }
    }
}

pub fn codec_from_str(s: &str) -> Result<Codec> {
    match s.to_uppercase().as_str() {
        "VP8" => Ok(Codec::Vp8),
        "VP9" => Ok(Codec::Vp9),
        "H264" => Ok(Codec::H264),
        "H265" | "HEVC" => Ok(Codec::H265),
        "AV1" => Ok(Codec::AV1),
        "OPUS" => Ok(Codec::Opus),
        "G722" => Ok(Codec::G722),
        _ => Err(anyhow!("Unknown codec: {}", s)),
    }
}

pub fn get_codec_type(codec: &RTCRtpCodec) -> RtpCodecKind {
    let mime_type = &codec.mime_type;
    if mime_type.starts_with("video") {
        RtpCodecKind::Video
    } else if mime_type.starts_with("audio") {
        RtpCodecKind::Audio
    } else {
        RtpCodecKind::Unspecified
    }
}

pub struct ChildGuard(Mutex<Child>);

impl ChildGuard {
    pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, Child>> {
        self.0.lock()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(mut child) = self.0.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn create_child(command: Option<String>) -> Result<Option<ChildGuard>> {
    Ok(match command {
        Some(command) => {
            #[cfg(windows)]
            let command = command.replace('\\', "/");
            let mut args = shellwords::split(&command)?;
            Some(ChildGuard(Mutex::new(
                Command::new(args.remove(0))
                    .args(args)
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()?,
            )))
        }
        None => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_codecs_include_transport_cc_feedback() {
        for codec in [Codec::Vp8, Codec::Vp9, Codec::H264, Codec::H265, Codec::AV1] {
            let rtp_codec = RTCRtpCodec::from(codec);

            assert!(
                rtp_codec.rtcp_feedback.iter().any(|feedback| {
                    feedback.typ == "transport-cc" && feedback.parameter.is_empty()
                }),
                "{codec:?} missing transport-cc feedback: {:?}",
                rtp_codec.rtcp_feedback
            );
        }
    }
}
