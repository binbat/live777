use std::collections::HashSet;

use anyhow::{Result, anyhow};

pub fn maybe_filter_vp8(sdp: &str, disable_vp8: bool) -> Result<String> {
    if !disable_vp8 {
        return Ok(sdp.to_string());
    }

    let mut vp8_pts = HashSet::new();
    for line in sdp.lines() {
        if let Some(rest) = line.strip_prefix("a=rtpmap:")
            && let Some((pt, codec)) = rest.split_once(' ')
            && codec.to_ascii_uppercase().starts_with("VP8/")
        {
            vp8_pts.insert(pt.to_string());
        }
    }

    if vp8_pts.is_empty() {
        return Ok(sdp.to_string());
    }

    let mut output = Vec::new();
    let mut removed_all_video = false;

    for line in sdp.lines() {
        if line.starts_with("m=video ") {
            let mut parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 4 {
                output.push(line.to_string());
                continue;
            }
            let head = parts.drain(0..3).collect::<Vec<_>>().join(" ");
            let payloads: Vec<&str> = parts
                .into_iter()
                .filter(|pt| !vp8_pts.contains(*pt))
                .collect();
            if payloads.is_empty() {
                removed_all_video = true;
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
            && vp8_pts.contains(pt)
        {
            continue;
        }

        output.push(line.to_string());
    }

    if removed_all_video {
        return Err(anyhow!(
            "VP8 is disabled and no alternative video codec is available"
        ));
    }

    Ok(output.join("\n"))
}
