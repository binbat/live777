use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters};

pub fn rtp_codecs_match(left: &RTCRtpCodec, right: &RTCRtpCodec) -> bool {
    left.mime_type.eq_ignore_ascii_case(&right.mime_type)
        && left.clock_rate == right.clock_rate
        && left.channels == right.channels
        && left.sdp_fmtp_line == right.sdp_fmtp_line
}

pub fn fmtp_param(fmtp: &str, key: &str) -> Option<String> {
    fmtp.split(';').find_map(|part| {
        let (param_key, value) = part.trim().split_once('=')?;
        param_key
            .trim()
            .eq_ignore_ascii_case(key)
            .then(|| value.trim().to_ascii_lowercase())
    })
}

/// Extract a fmtp parameter value without lowercasing. Use this for
/// binary/base64-encoded values (sprop-parameter-sets, sprop-vps, etc.).
pub fn fmtp_param_case_preserving<'a>(fmtp: &'a str, key: &str) -> Option<&'a str> {
    fmtp.split(';').find_map(|part| {
        let (param_key, value) = part.trim().split_once('=')?;
        param_key
            .trim()
            .eq_ignore_ascii_case(key)
            .then_some(value.trim())
    })
}

pub fn is_h265_codec(codec: &RTCRtpCodec) -> bool {
    codec.mime_type.eq_ignore_ascii_case("video/H265")
}

pub fn is_av1_codec(codec: &RTCRtpCodec) -> bool {
    codec.mime_type.eq_ignore_ascii_case("video/AV1")
}

pub fn h265_codecs_are_compatible(existing_codec: &RTCRtpCodec, new_codec: &RTCRtpCodec) -> bool {
    if !is_h265_codec(existing_codec) || !is_h265_codec(new_codec) {
        return false;
    }

    if existing_codec.clock_rate != new_codec.clock_rate
        || existing_codec.channels != new_codec.channels
    {
        return false;
    }

    // Parameters with RFC 7798 defaults: compare the resolved values so
    // that an explicit default on one side does not conflict with an
    // omitted default on the other.
    for key in ["profile-space", "profile-id", "tier-flag"] {
        let existing_value = h265_fmtp_param_or_default(&existing_codec.sdp_fmtp_line, key);
        let new_value = h265_fmtp_param_or_default(&new_codec.sdp_fmtp_line, key);
        if existing_value != new_value {
            return false;
        }
    }

    // tx-mode has no defined default.  Only reject when both sides
    // declare tx-mode and the values differ.  RTSP publishers typically
    // omit tx-mode entirely; treating an absent value as "compatible
    // with any mode" avoids unnecessary replace_track timeouts.
    match (
        fmtp_param(&existing_codec.sdp_fmtp_line, "tx-mode"),
        fmtp_param(&new_codec.sdp_fmtp_line, "tx-mode"),
    ) {
        (Some(existing_value), Some(new_value)) if existing_value == new_value => {}
        (Some(_), Some(_)) => return false,
        _ => {}
    }

    true
}

pub fn h265_fmtp_param_or_default(fmtp: &str, key: &str) -> String {
    fmtp_param(fmtp, key).unwrap_or_else(|| match key {
        "profile-space" => "0".to_string(),
        "profile-id" => "1".to_string(),
        "tier-flag" => "0".to_string(),
        _ => "".to_string(),
    })
}

pub fn h265_candidate_level_sufficient(candidate: &RTCRtpCodec, publisher: &RTCRtpCodec) -> bool {
    // level-id in the offer indicates the highest level the receiver can
    // support, so the publisher's level must be <= the candidate's level.
    // RFC 7798 defines level-id as a *decimal* integer equal to
    // general_level_idc (e.g. 93 = Level 3.1, 180 = Level 6.0).
    //
    // Only enforce the comparison when both sides explicitly declare level-id.
    // Browsers such as Safari/WebKit commonly omit level-id from their offer;
    // treating an omitted value as "no level limit declared" avoids rejecting
    // subscribers that previously matched before this gate existed.
    let candidate_level = fmtp_param(&candidate.sdp_fmtp_line, "level-id");
    let publisher_level = fmtp_param(&publisher.sdp_fmtp_line, "level-id");
    let (Some(candidate_level), Some(publisher_level)) = (candidate_level, publisher_level) else {
        return true;
    };
    match (
        candidate_level.parse::<u32>(),
        publisher_level.parse::<u32>(),
    ) {
        (Ok(candidate_level), Ok(publisher_level)) => publisher_level <= candidate_level,
        // Don't reject on an unparseable fmtp value.
        _ => true,
    }
}

/// AV1 fmtp may differ by `profile`/`profile-id`, `level-idx` and `tier`
/// between the existing bound sender track and the newly selected codec.
/// The bound track can be reused only if it is at least as capable as the
/// new stream in every dimension (profile, level-idx, tier). Reusing the
/// bound track avoids a `replace_track` that `webrtc-rs` will not re-bind,
/// which manifests as `track is not binding yet` and a 3 s subscribe loop
/// timeout.
pub fn av1_codecs_are_compatible(existing_codec: &RTCRtpCodec, new_codec: &RTCRtpCodec) -> bool {
    if !is_av1_codec(existing_codec) || !is_av1_codec(new_codec) {
        return false;
    }

    if existing_codec.clock_rate != new_codec.clock_rate
        || existing_codec.channels != new_codec.channels
    {
        return false;
    }

    // AV1 RTP spec uses `profile`, while older rtc-rs/webrtc-rs code uses
    // `profile-id`. Chrome answers with `profile`, so accept both names.
    let existing_profile = av1_profile_id(&existing_codec.sdp_fmtp_line);
    let new_profile = av1_profile_id(&new_codec.sdp_fmtp_line);
    if new_profile > existing_profile {
        return false;
    }

    let existing_level = av1_level_idx(&existing_codec.sdp_fmtp_line);
    let new_level = av1_level_idx(&new_codec.sdp_fmtp_line);
    if new_level > existing_level {
        return false;
    }

    let existing_tier = av1_tier(&existing_codec.sdp_fmtp_line);
    let new_tier = av1_tier(&new_codec.sdp_fmtp_line);
    if new_tier > existing_tier {
        return false;
    }

    true
}

/// Parse AV1 profile from an fmtp line, accepting both `profile` and
/// `profile-id`. Defaults to 0 (Main profile) when absent.
pub fn av1_profile_id(fmtp: &str) -> u32 {
    fmtp_param(fmtp, "profile")
        .or_else(|| fmtp_param(fmtp, "profile-id"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Parse AV1 level-idx from an fmtp line. Defaults to 0 when absent.
pub fn av1_level_idx(fmtp: &str) -> u32 {
    fmtp_param(fmtp, "level-idx")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Parse AV1 tier from an fmtp line. Defaults to 0 when absent.
pub fn av1_tier(fmtp: &str) -> u32 {
    fmtp_param(fmtp, "tier")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn sender_track_codec_compatible(
    sender_track_codec: &RTCRtpCodec,
    selected_codec: &RTCRtpCodec,
) -> bool {
    rtp_codecs_match(sender_track_codec, selected_codec)
        || (is_h264_codec(sender_track_codec) && is_h264_codec(selected_codec))
        || (h265_codecs_are_compatible(sender_track_codec, selected_codec)
            && h265_candidate_level_sufficient(sender_track_codec, selected_codec))
        || av1_codecs_are_compatible(sender_track_codec, selected_codec)
}

pub fn is_h264_codec(codec: &RTCRtpCodec) -> bool {
    codec.mime_type.eq_ignore_ascii_case("video/H264")
}

pub fn select_compatible_codec(
    publisher_codec: &RTCRtpCodec,
    codecs: &[RTCRtpCodecParameters],
) -> Option<RTCRtpCodecParameters> {
    let exact_match = codecs
        .iter()
        .find(|candidate| rtp_codecs_match(&candidate.rtp_codec, publisher_codec))
        .cloned();

    if exact_match.is_some() {
        return exact_match;
    }

    if is_h265_codec(publisher_codec) {
        return codecs
            .iter()
            .find(|candidate| {
                h265_codecs_are_compatible(&candidate.rtp_codec, publisher_codec)
                    && h265_candidate_level_sufficient(&candidate.rtp_codec, publisher_codec)
            })
            .cloned();
    }

    if is_av1_codec(publisher_codec) {
        return codecs
            .iter()
            .find(|candidate| av1_codecs_are_compatible(&candidate.rtp_codec, publisher_codec))
            .cloned();
    }

    if is_h264_codec(publisher_codec) {
        // Chrome iterates the *answer's* m= line and picks the first H264
        // codec whose fmtp matches its offer.  The answer lists codecs in
        // MediaEngine registration order (PT 119 High Profile first).  We
        // must use the same first-matched PT so Chrome's decoder PT matches
        // the PT we write RTP with.  Compatibility of profile-level-id etc.
        // is handled by inject_publisher_sprop in the SDP answer.
        return codecs
            .iter()
            .find(|candidate| {
                candidate
                    .rtp_codec
                    .mime_type
                    .eq_ignore_ascii_case(&publisher_codec.mime_type)
                    && candidate.rtp_codec.clock_rate == publisher_codec.clock_rate
            })
            .cloned();
    }

    codecs
        .iter()
        .find(|candidate| {
            candidate
                .rtp_codec
                .mime_type
                .eq_ignore_ascii_case(&publisher_codec.mime_type)
                && candidate.rtp_codec.clock_rate == publisher_codec.clock_rate
        })
        .cloned()
}

/// Remove a semicolon-separated key from a fmtp string.
pub(crate) fn remove_fmtp_key(fmtp: &str, key: &str) -> String {
    let parts: Vec<&str> = fmtp.split(';').collect();
    let result: Vec<String> = parts
        .iter()
        .filter(|p| {
            let trimmed = p.trim();
            if let Some((k, _)) = trimmed.split_once('=') {
                !k.trim().eq_ignore_ascii_case(key)
            } else {
                !trimmed.is_empty()
            }
        })
        .map(|p| p.trim().to_owned())
        .collect();
    result.join(";")
}
