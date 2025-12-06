use anyhow::{Result, anyhow};
use cli::codec_from_str;
use sdp::description::common::{Address, ConnectionInformation};
use sdp::{SessionDescription, description::media::RangedPort};
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{debug, info};

use crate::utils;

pub async fn setup_rtp_input(target_url: &str) -> Result<(rtsp::MediaInfo, String)> {
    info!("Processing RTP input mode");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let path = Path::new(target_url);
    let sdp_bytes = fs::read(path).map_err(|e| anyhow!("Failed to read SDP file: {}", e))?;
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
    info!("SDP file parsed successfully");

    let video_track = sdp.medias.iter().find(|md| md.media == "video");
    let audio_track = sdp.medias.iter().find(|md| md.media == "audio");
    let (codec_vid, codec_aud) = parse_codecs(&video_track, &audio_track);

    let media_info = rtsp::MediaInfo {
        video_transport: video_track.map(|track| {
            let port = track.port;
            rtsp::TransportInfo::Udp {
                rtp_send_port: None,
                rtp_recv_port: Some(port),
                rtcp_send_port: None,
                rtcp_recv_port: Some(port + 1),
                server_addr: None,
            }
        }),
        audio_transport: audio_track.map(|track| {
            let port = track.port;
            rtsp::TransportInfo::Udp {
                rtp_send_port: None,
                rtp_recv_port: Some(port),
                rtcp_send_port: None,
                rtcp_recv_port: Some(port + 1),
                server_addr: None,
            }
        }),
        video_codec: if !codec_vid.is_empty() && codec_vid != "unknown" {
            Some(codec_from_str(&codec_vid)?.into())
        } else {
            None
        },
        audio_codec: if !codec_aud.is_empty() && codec_aud != "unknown" {
            Some(codec_from_str(&codec_aud)?.into())
        } else {
            None
        },
    };

    Ok((media_info, host))
}

pub async fn setup_rtp_output(
    input: &url::Url,
    filtered_sdp: String,
    sdp_filename: Option<String>,
    notify: Arc<Notify>,
) -> Result<rtsp::MediaInfo> {
    info!("Processing RTP output mode");

    let mut reader = Cursor::new(filtered_sdp.as_bytes());
    let session = SessionDescription::unmarshal(&mut reader)
        .map_err(|e| anyhow!("Failed to parse SDP: {:?}", e))?;

    let (target_host, _listen_host) = utils::host::parse_host(input);

    let mut video_port: Option<u16> = None;
    let mut audio_port: Option<u16> = None;

    for (key, value) in input.query_pairs() {
        match key.as_ref() {
            "video" => video_port = value.parse::<u16>().ok(),
            "audio" => audio_port = value.parse::<u16>().ok(),
            _ => {}
        }
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

    let video_port = if video_codec.is_some() {
        video_port.or(Some(5004))
    } else {
        None
    };

    let audio_port = if audio_codec.is_some() {
        audio_port.or(Some(5006))
    } else {
        None
    };

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

        if media.media_name.media == "video"
            && let Some(rtsp::TransportInfo::Udp {
                rtp_send_port: Some(port),
                ..
            }) = &media_info.video_transport
        {
            media.media_name.port = RangedPort {
                value: *port as isize,
                range: None,
            };
        } else if media.media_name.media == "audio"
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

    let sdp = session.marshal();
    let file_path = sdp_filename.unwrap_or_else(|| "output.sdp".to_string());
    debug!("SDP written to {:?}", file_path);

    let mut file = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(file_path)?;
    file.write_all(sdp.as_bytes())?;

    notify.notify_one();
    debug!("Sent signal to start child process");

    Ok(media_info)
}

fn parse_codecs(
    video_track: &Option<&sdp_types::Media>,
    audio_track: &Option<&sdp_types::Media>,
) -> (String, String) {
    let codec_vid = video_track
        .and_then(extract_codec_name)
        .unwrap_or_else(|| "unknown".to_string());

    let codec_aud = audio_track
        .and_then(extract_codec_name)
        .unwrap_or_else(|| "unknown".to_string());

    (codec_vid, codec_aud)
}

fn extract_codec_name(media: &sdp_types::Media) -> Option<String> {
    media
        .attributes
        .iter()
        .find(|attr| attr.attribute == "rtpmap")
        .and_then(|attr| attr.value.as_ref())
        .and_then(|value| {
            value
                .split_whitespace()
                .nth(1)?
                .split('/')
                .next()
                .map(|s| s.to_string())
        })
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
                .unwrap_or("")
                .split('/')
                .next()
                .and_then(|codec_str| codec_from_str(codec_str).ok())
        })
}
