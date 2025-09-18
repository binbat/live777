use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

/// https://github.com/coturn/coturn/blob/24e99eca1cf84ad83f050c957e745ed12edcfeff/README.turnserver#L189
/// the TURN REST API section below.
/// This option uses timestamp as part of combined username:
/// usercombo -> "timestamp:username",
/// turn user -> usercombo,
/// turn password -> base64(hmac(input_buffer = usercombo, key = shared-secret)).
pub fn generate_credentials(
    secret: String,
    expiry_timestamp: u64,
    username: Option<&str>,
) -> (String, String) {
    // usercombo -> "timestamp:username",
    let usercombo = build_username(expiry_timestamp, username);
    let password = generate_password(secret, &usercombo);

    (usercombo, password)
}

pub fn generate_expiry_timestamp(ttl: u64) -> u64 {
    (SystemTime::now() + Duration::from_secs(ttl))
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn build_username(expiry_timestamp: u64, user_id: Option<&str>) -> String {
    match user_id {
        Some(id) => format!("{expiry_timestamp}:{id}"),
        None => format!("{expiry_timestamp}"),
    }
}

fn generate_password(secret: String, username: &str) -> String {
    let mut mac =
        HmacSha1::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");

    mac.update(username.as_bytes());

    let result = mac.finalize();
    let code_bytes = result.into_bytes();

    base64::engine::general_purpose::STANDARD.encode(code_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_credentials() {
        let secret = "live777".to_string();

        // 2^31 - 1
        let time = 2147483647;

        let user = "test_user";

        let (turn_username, turn_password) =
            ("2147483647:test_user", "x7lXoYLSrlcRYpeOHUYpZwdeXBI=");

        let (username, password) = generate_credentials(secret, time, Some(user));

        assert!(username.contains(':'));
        assert!(username.split(':').next().unwrap().parse::<u64>().is_ok());

        assert!(!password.is_empty());

        assert_eq!(username, turn_username);
        assert_eq!(password, turn_password);
    }

    #[test]
    fn test_generate_credentials_no_user() {
        let secret = "live777".to_string();

        // 2^31 - 1
        let time = 2147483647;

        let (turn_username, turn_password) = ("2147483647", "HH+w8NN6qmf+uXPId6yGRy3CLps=");

        let (username, password) = generate_credentials(secret, time, None);

        assert!(!username.contains(':'));
        assert!(username.parse::<u64>().is_ok());

        assert!(!password.is_empty());

        assert_eq!(username, turn_username);
        assert_eq!(password, turn_password);
    }
}
