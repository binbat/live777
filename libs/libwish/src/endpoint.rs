//! WHIP/WHEP endpoint URL handling: mapping the `whip(s)://` / `whep(s)://`
//! config schemes onto the `http(s)://` URLs a client POSTs to, with an
//! optional Bearer token carried as userinfo. Shared by every client of a
//! WHIP/WHEP endpoint — the liveion WHEP pull source, the liveion WHIP push
//! target, and the whepfrom/whipinto tools.

use anyhow::Result;

/// Map a `whip://` / `whips://` endpoint URL to the `http(s)://` URL a WHIP
/// client POSTs to. A Bearer token can be carried as userinfo:
/// `whip://token@host:port/whip/stream`.
pub fn parse_whip_url(raw: &str) -> Result<(String, Option<String>)> {
    parse_endpoint_url(raw, "whip")
}

/// Map a `whep://` / `wheps://` endpoint URL to the `http(s)://` URL a WHEP
/// client POSTs to. A Bearer token can be carried as userinfo:
/// `whep://token@host:port/whep/stream`.
pub fn parse_whep_url(raw: &str) -> Result<(String, Option<String>)> {
    parse_endpoint_url(raw, "whep")
}

fn parse_endpoint_url(raw: &str, scheme: &str) -> Result<(String, Option<String>)> {
    let upper = scheme.to_ascii_uppercase();
    // Scheme matching is case-insensitive (RFC 3986). The replacement itself
    // is done textually: `whip`/`whep` are not WHATWG "special" schemes, so
    // `Url::set_scheme` refuses the conversion to `http(s)`.
    let http_url = match raw.split_once("://") {
        Some((s, rest)) if s.eq_ignore_ascii_case(scheme) => format!("http://{rest}"),
        Some((s, rest)) if s.eq_ignore_ascii_case(format!("{scheme}s").as_str()) => {
            format!("https://{rest}")
        }
        _ => anyhow::bail!("Unsupported {upper} endpoint URL: {}", redact_url(raw)),
    };

    let mut url = url::Url::parse(&http_url)?;
    if url.host_str().is_none() {
        anyhow::bail!(
            "Invalid {upper} endpoint URL (no host): {}",
            redact_url(raw)
        );
    }

    // Only token-in-username is supported. A password means the user:pass
    // form, which has no mapping onto Bearer auth — fail fast instead of
    // silently dropping it (the error must not echo the URL: it contains
    // the credential).
    if url.password().is_some() {
        anyhow::bail!(
            "{upper} endpoint URL must not carry a password; use {scheme}://token@host… for Bearer auth"
        );
    }

    // `Url::username` is still percent-encoded; decode so tokens containing
    // reserved characters reach the Bearer header in their original form.
    let token = (!url.username().is_empty()).then(|| {
        percent_encoding::percent_decode_str(url.username())
            .decode_utf8_lossy()
            .into_owned()
    });

    // Strip userinfo unconditionally: the URL is used for requests and log
    // lines, neither of which may see the credential.
    url.set_username("")
        .map_err(|_| anyhow::anyhow!("Invalid {upper} endpoint URL"))?;
    url.set_password(None)
        .map_err(|_| anyhow::anyhow!("Invalid {upper} endpoint URL"))?;

    Ok((url.to_string(), token))
}

/// `url` with any userinfo credentials stripped, safe for log lines.
/// Falls back to a scheme-only placeholder when the URL cannot be parsed
/// (an unparseable URL may still embed credentials).
pub fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => match raw.split_once("://") {
            Some((scheme, _)) => format!("{scheme}://<redacted>"),
            None => "<redacted>".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maps_scheme() {
        let (url, token) = parse_whip_url("whip://example.com:7777/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whip/cam1");
        assert_eq!(token, None);

        let (url, token) = parse_whep_url("whep://example.com:7777/whep/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whep/cam1");
        assert_eq!(token, None);
    }

    #[test]
    fn parse_scheme_is_case_insensitive() {
        let (url, _) = parse_whip_url("WHIP://example.com:7777/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whip/cam1");
        let (url, _) = parse_whip_url("Whips://example.com/whip/cam1").unwrap();
        assert_eq!(url, "https://example.com/whip/cam1");

        let (url, _) = parse_whep_url("WHEP://example.com:7777/whep/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whep/cam1");
        let (url, _) = parse_whep_url("Wheps://example.com/whep/cam1").unwrap();
        assert_eq!(url, "https://example.com/whep/cam1");
    }

    #[test]
    fn parse_tls_scheme_maps_to_https() {
        let (url, token) = parse_whip_url("whips://example.com/whip/cam1").unwrap();
        assert_eq!(url, "https://example.com/whip/cam1");
        assert_eq!(token, None);

        let (url, token) = parse_whep_url("wheps://example.com/whep/cam1").unwrap();
        assert_eq!(url, "https://example.com/whep/cam1");
        assert_eq!(token, None);
    }

    #[test]
    fn parse_extracts_userinfo_token() {
        let (url, token) = parse_whip_url("whip://secret@example.com/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com/whip/cam1");
        assert_eq!(token, Some("secret".to_string()));

        let (url, token) = parse_whep_url("whep://secret@example.com/whep/cam1").unwrap();
        assert_eq!(url, "http://example.com/whep/cam1");
        assert_eq!(token, Some("secret".to_string()));
    }

    #[test]
    fn parse_decodes_percent_encoded_token() {
        let (url, token) = parse_whip_url("whip://tok%2Fen%3D@example.com/whip/cam1").unwrap();
        assert_eq!(url, "http://example.com/whip/cam1");
        assert_eq!(token, Some("tok/en=".to_string()));
    }

    #[test]
    fn parse_rejects_other_schemes() {
        assert!(parse_whip_url("rtsp://example.com/stream").is_err());
        assert!(parse_whip_url("whep://example.com/whep/cam1").is_err());
        assert!(parse_whep_url("rtsp://example.com/stream").is_err());
        assert!(parse_whep_url("whip://example.com/whip/cam1").is_err());
    }

    #[test]
    fn parse_rejects_password_without_leaking_it() {
        for raw in [
            "whip://user:s3cret@example.com/whip/cam1",
            "whip://:s3cret@example.com/whip/cam1",
            "whep://user:s3cret@example.com/whep/cam1",
            "whep://:s3cret@example.com/whep/cam1",
        ] {
            for err in [
                parse_whip_url(raw).unwrap_err(),
                parse_whep_url(raw).unwrap_err(),
            ] {
                assert!(
                    !err.to_string().contains("s3cret"),
                    "error leaks the credential: {err}"
                );
            }
        }
    }

    #[test]
    fn parse_error_redacts_credentials() {
        let err = parse_whip_url("whop://secret@example.com/whip/cam1").unwrap_err();
        assert!(!err.to_string().contains("secret"));
        let err = parse_whep_url("whelp://secret@example.com/whep/cam1").unwrap_err();
        assert!(!err.to_string().contains("secret"));
    }

    #[test]
    fn redact_url_strips_userinfo() {
        assert_eq!(
            redact_url("whip://token@edge-0:7777/whip/cam1"),
            "whip://edge-0:7777/whip/cam1"
        );
        assert_eq!(redact_url("whip://tok en@not a host"), "whip://<redacted>");
        assert_eq!(redact_url("not-a-url"), "<redacted>");
    }
}
