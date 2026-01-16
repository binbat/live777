use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};

pub fn maybe_filter_codecs(sdp: &str, disabled_codecs: &[String]) -> Result<String> {
    let disabled: HashSet<String> = disabled_codecs
        .iter()
        .map(|c| c.trim().to_ascii_uppercase())
        .filter(|c| !c.is_empty())
        .collect();
    if disabled.is_empty() {
        return Ok(sdp.to_string());
    }

    let mut pt_to_codec: HashMap<String, String> = HashMap::new();
    for line in sdp.lines() {
        if let Some(rest) = line.strip_prefix("a=rtpmap:")
            && let Some((pt, codec)) = rest.split_once(' ')
        {
            let codec_name = codec.split('/').next().unwrap_or(codec).trim();
            pt_to_codec.insert(pt.to_string(), codec_name.to_ascii_uppercase());
        }
    }

    let mut disabled_pts = HashSet::new();
    for (pt, codec) in pt_to_codec.iter() {
        if disabled.contains(codec) {
            disabled_pts.insert(pt.clone());
        }
    }

    if disabled_pts.is_empty() {
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
                .filter(|pt| !disabled_pts.contains(*pt))
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
            && disabled_pts.contains(pt)
        {
            continue;
        }

        output.push(line.to_string());
    }

    if !removed_media.is_empty() {
        let mut list = removed_media.into_iter().collect::<Vec<_>>();
        list.sort();
        return Err(anyhow!(
            "Disabled codecs removed all payloads for media: {}",
            list.join(", ")
        ));
    }

    Ok(output.join("\n"))
}
