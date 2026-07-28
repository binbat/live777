//! Minimal HS256 (HMAC-SHA256) JWT encode/decode.
//!
//! Replaces the `jsonwebtoken` crate: Live777 only ever issues and verifies
//! HMAC-signed tokens, but `jsonwebtoken` links the RSA/ECDSA/JWKS machinery
//! for every algorithm (~300 KiB in the release binary) because the algorithm
//! is dispatched at runtime. `ring` is already in the dependency graph
//! (WebRTC DTLS), so this module adds no new native code.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Error, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::hmac;

use crate::claims::Claims;

/// Serialized JWT header, byte-identical to `jsonwebtoken::Header::default()`.
const HEADER_JSON: &str = r#"{"typ":"JWT","alg":"HS256"}"#;

/// Matches `jsonwebtoken::Validation::default()` leeway.
const EXPIRY_LEEWAY_SECS: u64 = 60;

pub fn encode(claims: &Claims, secret: &[u8]) -> Result<String, Error> {
    let mut token = URL_SAFE_NO_PAD.encode(HEADER_JSON);
    token.push('.');
    token.push_str(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims)?));
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let signature = hmac::sign(&key, token.as_bytes());
    token.push('.');
    token.push_str(&URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(token)
}

pub fn decode(token: &str, secret: &[u8]) -> Result<Claims, Error> {
    let (message, signature_b64) = token
        .rsplit_once('.')
        .ok_or_else(|| anyhow!("invalid token: missing signature"))?;
    let (header_b64, payload_b64) = message
        .split_once('.')
        .ok_or_else(|| anyhow!("invalid token: missing payload"))?;

    let header: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64)?)?;
    if header.get("alg").and_then(|alg| alg.as_str()) != Some("HS256") {
        return Err(anyhow!("invalid token: unsupported algorithm"));
    }

    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let signature = URL_SAFE_NO_PAD.decode(signature_b64)?;
    hmac::verify(&key, message.as_bytes(), &signature)
        .map_err(|_| anyhow!("invalid token: signature mismatch"))?;

    let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_b64)?)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if claims.exp < now.saturating_sub(EXPIRY_LEEWAY_SECS) {
        return Err(anyhow!("invalid token: expired"));
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"secret";
    /// Precomputed HS256 token for `{"id":"test","exp":4102444800,"mode":7}`
    /// with secret `secret` (HMAC-SHA256, verified against an independent
    /// implementation).
    const KNOWN_TOKEN: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJpZCI6InRlc3QiLCJleHAiOjQxMDI0NDQ4MDAsIm1vZGUiOjd9.PH1OihSKeduwvmBYxnz_mOhvk53aYD1J-31besrOjuM";

    fn claims() -> Claims {
        Claims {
            id: "test".to_string(),
            exp: 4102444800, // 2100-01-01
            mode: 7,
        }
    }

    #[test]
    fn encode_matches_known_vector() {
        assert_eq!(encode(&claims(), SECRET).unwrap(), KNOWN_TOKEN);
    }

    #[test]
    fn roundtrip() {
        let token = encode(&claims(), SECRET).unwrap();
        let decoded = decode(&token, SECRET).unwrap();
        assert_eq!(decoded.id, "test");
        assert_eq!(decoded.exp, 4102444800);
        assert_eq!(decoded.mode, 7);
    }

    #[test]
    fn decode_known_vector() {
        let decoded = decode(KNOWN_TOKEN, SECRET).unwrap();
        assert_eq!(decoded.id, "test");
    }

    #[test]
    fn rejects_wrong_secret() {
        assert!(decode(KNOWN_TOKEN, b"wrong").is_err());
    }

    #[test]
    fn rejects_tampered_payload() {
        let token = KNOWN_TOKEN.replace("InRlc3Qi", "InRlc3Qy");
        assert!(decode(&token, SECRET).is_err());
    }

    #[test]
    fn rejects_non_hs256_alg() {
        // header {"alg":"none"}
        let token = "eyJhbGciOiJub25lIn0.eyJpZCI6InRlc3QiLCJleHAiOjQxMDI0NDQ4MDAsIm1vZGUiOjd9.";
        assert!(decode(token, SECRET).is_err());
    }

    #[test]
    fn rejects_expired() {
        let mut expired = claims();
        expired.exp = 1_000_000; // 1970
        let token = encode(&expired, SECRET).unwrap();
        assert!(decode(&token, SECRET).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("not-a-token", SECRET).is_err());
        assert!(decode("a.b.c", SECRET).is_err());
    }
}
