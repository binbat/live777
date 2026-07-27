use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};

/// Metadata of one RTP payload type collected from the offer.
#[derive(Default)]
struct Payload {
    /// Media type of the enclosing m= section, e.g. "video" / "audio".
    media: String,
    /// Codec name from a=rtpmap, uppercase; empty for static payload types.
    codec: String,
    /// Base payload referenced via a=fmtp apt= (e.g. RTX retransmission).
    apt: Option<String>,
}

/// Filter an SDP offer down to the configured codec whitelists.
///
/// `video_codecs` / `audio_codecs` list the only codecs allowed for that media
/// type; an empty list means no restriction. Codec-associated payloads such as
/// RTX retransmission follow the codec they reference via `fmtp apt=` instead
/// of being judged by their own name. Payloads without an `a=rtpmap` entry
/// (static payload types) are kept as-is.
pub fn maybe_filter_codecs(
    sdp: &str,
    video_codecs: &[String],
    audio_codecs: &[String],
) -> Result<String> {
    let video_whitelist = normalize(video_codecs);
    let audio_whitelist = normalize(audio_codecs);
    if video_whitelist.is_empty() && audio_whitelist.is_empty() {
        return Ok(sdp.to_string());
    }

    // First pass: collect payload metadata per media section.
    let mut payloads: HashMap<String, Payload> = HashMap::new();
    let mut current_media = String::new();

    for line in sdp.lines() {
        if let Some(rest) = line.strip_prefix("m=") {
            current_media = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix("a=rtpmap:")
            && let Some((pt, codec)) = rest.split_once(' ')
        {
            let payload = payloads.entry(pt.to_string()).or_default();
            payload.media = current_media.clone();
            payload.codec = codec
                .split('/')
                .next()
                .unwrap_or(codec)
                .trim()
                .to_ascii_uppercase();
            continue;
        }
        if let Some(rest) = line.strip_prefix("a=fmtp:")
            && let Some((pt, params)) = rest.split_once(' ')
            && let Some(apt) = params
                .split(';')
                .find_map(|p| p.trim().strip_prefix("apt="))
        {
            payloads.entry(pt.to_string()).or_default().apt = Some(apt.trim().to_string());
        }
    }

    // Base codecs are judged by name against their media type's whitelist;
    // associated payloads (e.g. RTX) are removed along with their apt base.
    let mut removed_pts: HashSet<String> = HashSet::new();
    for (pt, payload) in &payloads {
        if payload.apt.is_some() {
            continue;
        }
        let allowed = match payload.media.as_str() {
            "video" => video_whitelist.is_empty() || video_whitelist.contains(&payload.codec),
            "audio" => audio_whitelist.is_empty() || audio_whitelist.contains(&payload.codec),
            _ => true,
        };
        if !allowed {
            removed_pts.insert(pt.clone());
        }
    }
    loop {
        let mut changed = false;
        for (pt, payload) in &payloads {
            if let Some(base) = &payload.apt
                && removed_pts.contains(base)
                && removed_pts.insert(pt.clone())
            {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    if removed_pts.is_empty() {
        return Ok(sdp.to_string());
    }

    let mut output = Vec::new();
    let mut removed_media: HashSet<String> = HashSet::new();

    for line in sdp.lines() {
        if line.starts_with("m=") {
            let mut parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 4 {
                output.push(line.to_string());
                continue;
            }
            let media = parts[0].trim_start_matches("m=").to_string();
            let head = parts.drain(0..3).collect::<Vec<_>>().join(" ");
            let payloads: Vec<&str> = parts
                .into_iter()
                .filter(|pt| !removed_pts.contains(*pt))
                .collect();
            if payloads.is_empty() {
                removed_media.insert(media);
                continue;
            }
            output.push(format!("{} {}", head, payloads.join(" ")));
            continue;
        }

        if let Some(rest) = line
            .strip_prefix("a=rtpmap:")
            .or_else(|| line.strip_prefix("a=fmtp:"))
            .or_else(|| line.strip_prefix("a=rtcp-fb:"))
            && let Some((pt, _)) = rest.split_once(' ')
            && removed_pts.contains(pt)
        {
            continue;
        }

        output.push(line.to_string());
    }

    if !removed_media.is_empty() {
        let mut list = removed_media.into_iter().collect::<Vec<_>>();
        list.sort();
        return Err(anyhow!(
            "Codec whitelist removed all payloads for media: {}",
            list.join(", ")
        ));
    }

    Ok(output.join("\n"))
}

fn normalize(codecs: &[String]) -> HashSet<String> {
    codecs
        .iter()
        .map(|c| c.trim().to_ascii_uppercase())
        .filter(|c| !c.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFER: &str = "v=0\r
o=- 0 0 IN IP4 127.0.0.1\r
s=-\r
t=0 0\r
m=video 9 UDP/TLS/RTP/SAVPF 96 97 102 103\r
a=rtpmap:96 VP8/90000\r
a=rtcp-fb:96 nack\r
a=rtpmap:97 rtx/90000\r
a=fmtp:97 apt=96\r
a=rtpmap:102 H264/90000\r
a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f\r
a=rtcp-fb:102 nack\r
a=rtpmap:103 rtx/90000\r
a=fmtp:103 apt=102\r
m=audio 9 UDP/TLS/RTP/SAVPF 111 0\r
a=rtpmap:111 opus/48000/2\r
a=fmtp:111 minptime=10;useinbandfec=1\r
a=rtpmap:0 PCMU/8000";

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_whitelists_keep_sdp() {
        assert_eq!(maybe_filter_codecs(OFFER, &[], &[]).unwrap(), OFFER);
    }

    #[test]
    fn video_whitelist_keeps_rtx_of_allowed_codec() {
        let out = maybe_filter_codecs(OFFER, &strs(&["H264"]), &[]).unwrap();
        assert!(out.contains("m=video 9 UDP/TLS/RTP/SAVPF 102 103"));
        // VP8 and its RTX are gone, including fmtp/rtcp-fb lines
        assert!(!out.contains("VP8"));
        assert!(!out.contains("apt=96"));
        assert!(!out.contains("a=rtcp-fb:96"));
        // H264 RTX follows its base codec even though "RTX" is not whitelisted
        assert!(out.contains("a=rtpmap:103 rtx/90000"));
        // audio is unrestricted
        assert!(out.contains("m=audio 9 UDP/TLS/RTP/SAVPF 111 0"));
    }

    #[test]
    fn audio_whitelist_filters_audio_only() {
        let out = maybe_filter_codecs(OFFER, &[], &strs(&["OPUS"])).unwrap();
        assert!(out.contains("m=audio 9 UDP/TLS/RTP/SAVPF 111"));
        assert!(!out.contains("PCMU"));
        // video is unrestricted
        assert!(out.contains("m=video 9 UDP/TLS/RTP/SAVPF 96 97 102 103"));
    }

    #[test]
    fn whitelist_is_case_insensitive() {
        let out = maybe_filter_codecs(OFFER, &strs(&["h264"]), &[]).unwrap();
        assert!(out.contains("m=video 9 UDP/TLS/RTP/SAVPF 102 103"));
    }

    #[test]
    fn whitelist_emptying_media_section_errors() {
        let err = maybe_filter_codecs(OFFER, &strs(&["VP9"]), &[]).unwrap_err();
        assert!(err.to_string().contains("video"), "unexpected: {err}");
    }
}
