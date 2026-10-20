use anyhow::{Context, Result, anyhow};
use cli::codec_from_str;
use sdp::description::common::{Address, ConnectionInformation};
use sdp::{SessionDescription, description::media::RangedPort};
use std::io::Cursor;
use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{debug, info};

use crate::utils;
use rtsp::constants::media_type;

const SDP_FILE_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const SDP_FILE_WAIT_INTERVAL: Duration = Duration::from_millis(100);

pub async fn setup_rtp_input(target_url: &str) -> Result<(rtsp::MediaInfo, String)> {
    info!("Processing RTP input mode");

    let path = Path::new(target_url);

    // FFmpeg writes the SDP file asynchronously. Wait for it to contain a
    // valid-looking session description instead of parsing an empty or partial
    // file immediately.
    let sdp_bytes = tokio::time::timeout(SDP_FILE_WAIT_TIMEOUT, async {
        loop {
            match tokio::fs::read_to_string(path).await {
                Ok(contents) if contents.contains("v=") && contents.contains("m=") => {
                    return contents.into_bytes();
                }
                _ => {
                    tokio::time::sleep(SDP_FILE_WAIT_INTERVAL).await;
                }
            }
        }
    })
    .await
    .map_err(|_| {
        anyhow!(
            "SDP file was not populated within {:?}",
            SDP_FILE_WAIT_TIMEOUT
        )
    })?;

    let sdp =
        sdp_types::Session::parse(&sdp_bytes).map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;

    let mut host = String::new();
    if let Some(connection_info) = &sdp.connection {
        let addr: IpAddr = connection_info
            .connection_address
            .parse()
            .map_err(|e| anyhow!("Invalid IP address in SDP: {}", e))?;
        host = addr.to_string();
    }

    // Shared SDP parsing (libs/rtsp): keeps codec parameters such as H264
    // sprop-parameter-sets and H265 sprop-vps/sps/pps so the WHIP track setup
    // can inject them instead of silently dropping parameter sets.
    let (video_codec, audio_codec) = rtsp::parse_codecs_from_sdp(&sdp)?;
    let (video_transport, audio_transport) = rtsp::parse_transports_from_sdp(&sdp);

    let media_info = rtsp::MediaInfo {
        video_transport,
        audio_transport,
        video_codec,
        audio_codec,
    };

    info!("SDP parsed: host={}, media_info={:?}", host, media_info);

    Ok((media_info, host))
}

/// Options of an `rtp://` output, parsed from the URL: multicast tuning for
/// group destinations.
#[derive(Debug, Clone, Default)]
pub struct RtpOutputOptions {
    /// IPv4 multicast TTL / IPv6 multicast hops (kernel default 1).
    pub ttl: Option<u32>,
    /// Outbound multicast interface: an IPv4 address for IPv4 groups, an
    /// interface index or name for IPv6 groups.
    pub multicast_interface: Option<String>,
}

pub async fn setup_rtp_output(
    input: &url::Url,
    filtered_sdp: String,
    sdp_filename: Option<String>,
    notify: Arc<Notify>,
    authority_port: bool,
) -> Result<(rtsp::MediaInfo, RtpOutputOptions)> {
    info!("Processing RTP output mode");

    let mut reader = Cursor::new(filtered_sdp.as_bytes());
    let session = SessionDescription::unmarshal(&mut reader)
        .map_err(|e| anyhow!("Failed to parse SDP: {:?}", e))?;

    let (target_host, _listen_host) = utils::host::parse_host(input);

    let mut video_port: Option<u16> = None;
    let mut audio_port: Option<u16> = None;
    let mut options = RtpOutputOptions::default();

    // rtp://host:port addresses the video track with the URL's own port and
    // the audio track with port + 2 (the RTP/AVP convention, matching
    // live777's rtp:// target); ?video=/?audio= override per track. The
    // sdp:// scheme keeps its historical behavior: the authority port is
    // ignored and only the query ports count.
    if authority_port && let Some(port) = input.port() {
        video_port = Some(port);
        audio_port = Some(port.checked_add(2).ok_or_else(|| {
            anyhow!("rtp:// output port {port} leaves no room for the audio track (+2)")
        })?);
    }

    for (key, value) in input.query_pairs() {
        match key.as_ref() {
            media_type::VIDEO => video_port = value.parse::<u16>().ok(),
            media_type::AUDIO => audio_port = value.parse::<u16>().ok(),
            "ttl" => {
                let ttl = value
                    .parse::<u32>()
                    .map_err(|_| anyhow!("invalid rtp:// output ttl '{value}'"))?;
                if ttl > 255 {
                    return Err(anyhow!("rtp:// output ttl must be <= 255, got {ttl}"));
                }
                options.ttl = Some(ttl);
            }
            "interface" => options.multicast_interface = Some(value.into_owned()),
            _ => {}
        }
    }

    let is_multicast = target_host
        .parse::<IpAddr>()
        .map(|ip| ip.is_multicast())
        .unwrap_or(false);
    if is_multicast {
        #[cfg(not(feature = "multicast"))]
        return Err(anyhow!(
            "multicast rtp:// output ({target_host}) requires livetwo's 'multicast' feature"
        ));
    } else if options.ttl.is_some() || options.multicast_interface.is_some() {
        return Err(anyhow!(
            "ttl and interface are only valid with a multicast rtp:// output ({target_host} is not a multicast group)"
        ));
    }

    let mut video_codec = None;
    let mut audio_codec = None;

    for media in &session.media_descriptions {
        if media.media_name.media == "video" {
            video_codec = extract_codec_from_media(media);
        } else if media.media_name.media == "audio" {
            audio_codec = extract_codec_from_media(media);
        }
    }

    let video_port = video_port.filter(|_| video_codec.is_some());

    let audio_port = audio_port.filter(|_| audio_codec.is_some());

    let media_info = rtsp::MediaInfo {
        video_transport: video_port.map(|port| rtsp::TransportInfo::Udp {
            rtp_send_port: Some(port),
            rtp_recv_port: None,
            rtcp_send_port: Some(port + 1),
            rtcp_recv_port: None,
            server_addr: None,
        }),
        audio_transport: audio_port.map(|port| rtsp::TransportInfo::Udp {
            rtp_send_port: Some(port),
            rtp_recv_port: None,
            rtcp_send_port: Some(port + 1),
            rtcp_recv_port: None,
            server_addr: None,
        }),
        video_codec: video_codec.map(|c| c.into()),
        audio_codec: audio_codec.map(|c| c.into()),
    };

    // RFC 4566 asks for a /ttl suffix on the c= address of an IPv4 group;
    // receivers join the group either way, so the suffix is omitted rather
    // than risk tripping stricter SDP parsers on the receive side
    // (livetwo's own SDP file input included).
    let connection_info = ConnectionInformation {
        network_type: "IN".to_string(),
        address_type: if target_host.parse::<Ipv6Addr>().is_ok() {
            "IP6"
        } else {
            "IP4"
        }
        .to_string(),
        address: Some(Address {
            address: target_host.to_string(),
            ttl: None,
            range: None,
        }),
    };

    let mut session = session;
    session.connection_information = Some(connection_info.clone());

    for media in &mut session.media_descriptions {
        media.connection_information = Some(connection_info.clone());

        if media.media_name.media == media_type::VIDEO
            && let Some(rtsp::TransportInfo::Udp {
                rtp_send_port: Some(port),
                ..
            }) = &media_info.video_transport
        {
            media.media_name.port = RangedPort {
                value: *port as isize,
                range: None,
            };
        } else if media.media_name.media == media_type::AUDIO
            && let Some(rtsp::TransportInfo::Udp {
                rtp_send_port: Some(port),
                ..
            }) = &media_info.audio_transport
        {
            media.media_name.port = RangedPort {
                value: *port as isize,
                range: None,
            };
        }
    }

    // Remove media sections that have no corresponding transport mapping.
    // Prevents WebRTC virtual ports (< 1024) from leaking into the output SDP.
    session.media_descriptions.retain(|media| {
        if media.media_name.media == media_type::VIDEO {
            media_info.video_transport.is_some()
        } else if media.media_name.media == media_type::AUDIO {
            media_info.audio_transport.is_some()
        } else {
            false
        }
    });

    let sdp = session.marshal();
    let file_path = sdp_filename.unwrap_or_else(|| "output.sdp".to_string());
    debug!("SDP written to {:?}", file_path);

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&file_path)
        .await
        .with_context(|| format!("Failed to open SDP file {file_path}"))?;
    tokio::io::AsyncWriteExt::write_all(&mut file, sdp.as_bytes())
        .await
        .with_context(|| format!("Failed to write SDP file {file_path}"))?;

    notify.notify_one();
    debug!("Sent signal to start child process");

    Ok((media_info, options))
}

fn extract_codec_from_media(
    media: &sdp::description::media::MediaDescription,
) -> Option<cli::Codec> {
    media
        .attributes
        .iter()
        .find(|attr| attr.key == "rtpmap")
        .and_then(|attr| attr.value.as_ref())
        .and_then(|value| {
            value
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .split('/')
                .next()
                .and_then(|codec_str| codec_from_str(codec_str).ok())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal AV answer SDP: video H264 (PT 96) and audio Opus (PT 111).
    const AV_SDP: &str = "v=0\r\n\
        o=- 0 0 IN IP4 127.0.0.1\r\n\
        s=test\r\n\
        t=0 0\r\n\
        m=video 9 RTP/AVP 96\r\n\
        a=rtpmap:96 H264/90000\r\n\
        m=audio 9 RTP/AVP 111\r\n\
        a=rtpmap:111 OPUS/48000/2\r\n";

    fn udp_rtp_port(t: Option<rtsp::TransportInfo>) -> Option<u16> {
        match t {
            Some(rtsp::TransportInfo::Udp { rtp_send_port, .. }) => rtp_send_port,
            _ => None,
        }
    }

    async fn run(
        url: &str,
        authority_port: bool,
    ) -> Result<(rtsp::MediaInfo, RtpOutputOptions, String)> {
        let sdp_file = tempfile::NamedTempFile::new().unwrap();
        let path = sdp_file.path().to_string_lossy().into_owned();
        let input = url::Url::parse(url).unwrap();
        let (media_info, options) = setup_rtp_output(
            &input,
            AV_SDP.to_string(),
            Some(path.clone()),
            Arc::new(Notify::new()),
            authority_port,
        )
        .await?;
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        Ok((media_info, options, written))
    }

    /// rtp://host:port: the URL's port addresses video, port + 2 audio; the
    /// written SDP advertises exactly those.
    #[cfg(feature = "multicast")]
    #[tokio::test]
    async fn rtp_authority_port_maps_video_and_audio() {
        let (info, options, written) = run("rtp://230.1.1.1:1720", true).await.unwrap();
        assert_eq!(udp_rtp_port(info.video_transport), Some(1720));
        assert_eq!(udp_rtp_port(info.audio_transport), Some(1722));
        assert!(options.ttl.is_none() && options.multicast_interface.is_none());
        assert!(written.contains("c=IN IP4 230.1.1.1"), "SDP: {written}");
        assert!(
            written.contains("m=video 1720 RTP/AVP 96"),
            "SDP: {written}"
        );
        assert!(
            written.contains("m=audio 1722 RTP/AVP 111"),
            "SDP: {written}"
        );
    }

    /// ?video=/?audio= override the authority port per track.
    #[cfg(feature = "multicast")]
    #[tokio::test]
    async fn rtp_query_ports_override_authority_port() {
        let (info, ..) = run("rtp://230.1.1.1:1720?audio=6000", true).await.unwrap();
        assert_eq!(udp_rtp_port(info.video_transport), Some(1720));
        assert_eq!(udp_rtp_port(info.audio_transport), Some(6000));
    }

    /// The legacy query-only form keeps working unchanged (no authority
    /// port, no +2 default).
    #[tokio::test]
    async fn rtp_query_only_form_unchanged() {
        let (info, options, written) = run("rtp://127.0.0.1?video=5004&audio=5006", true)
            .await
            .unwrap();
        assert_eq!(udp_rtp_port(info.video_transport), Some(5004));
        assert_eq!(udp_rtp_port(info.audio_transport), Some(5006));
        assert!(options.ttl.is_none() && options.multicast_interface.is_none());
        assert!(written.contains("c=IN IP4 127.0.0.1"), "SDP: {written}");
    }

    /// sdp:// ignores the authority port (it never had one).
    #[tokio::test]
    async fn sdp_scheme_ignores_authority_port() {
        let (info, ..) = run("sdp://0.0.0.0:8555", false).await.unwrap();
        assert!(info.video_transport.is_none());
        assert!(info.audio_transport.is_none());
    }

    #[tokio::test]
    async fn rtp_rejects_ttl_on_unicast_and_bad_ttl() {
        assert!(run("rtp://127.0.0.1:5004?ttl=2", true).await.is_err());
        assert!(run("rtp://230.1.1.1:1720?ttl=300", true).await.is_err());
        assert!(run("rtp://230.1.1.1:1720?ttl=abc", true).await.is_err());
    }

    #[cfg(feature = "multicast")]
    #[tokio::test]
    async fn rtp_multicast_accepts_ttl_and_interface() {
        let (_, options, _) = run("rtp://230.1.1.1:1720?ttl=16&interface=192.168.1.10", true)
            .await
            .unwrap();
        assert_eq!(options.ttl, Some(16));
        assert_eq!(options.multicast_interface.as_deref(), Some("192.168.1.10"));
    }

    /// Without the multicast feature a group destination is a clear error.
    #[cfg(not(feature = "multicast"))]
    #[tokio::test]
    async fn rtp_multicast_rejected_without_feature() {
        assert!(run("rtp://230.1.1.1:1720", true).await.is_err());
    }
}
