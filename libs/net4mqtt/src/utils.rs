use rand::Rng;
use url::Url;

#[inline]
fn generate_random_string(length: usize) -> String {
    let charset: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();

    let random_string: String = (0..length)
        .map(|_| {
            let idx = rng.gen_range(0..charset.len());
            charset[idx] as char
        })
        .collect();

    random_string
}

#[inline]
fn strip_slashes(path: &str) -> &str {
    let mut start = 0;
    let mut end = path.len();

    if path.starts_with('/') {
        start = 1;
    }

    if path.ends_with('/') {
        end -= 1;
    }

    &path[start..end]
}

/// Reference: https://docs.oasis-open.org/mqtt/mqtt/v5.0/mqtt-v5.0.html
/// The Server MUST allow ClientID’s which are between 1 and 23 UTF-8 encoded bytes in length, and that contain only the characters
/// "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
const MQTT_CLIENT_ID_RANDOM_LENGTH: usize = 7;

/// https://docs.rs/rumqttc/0.24.0/src/rumqttc/lib.rs.html#799
const CLIENT_ID: &str = "client_id";

const DEFAULT_PREFIX: &str = "net4mqtt";

pub fn pre_url(mut u: Url) -> (Url, String) {
    if match u.query_pairs().find(|(k, _v)| k == CLIENT_ID) {
        Some((_k, v)) => v.is_empty(),
        None => true,
    } {
        u.query_pairs_mut()
            .clear()
            .append_pair(
                CLIENT_ID,
                &generate_random_string(MQTT_CLIENT_ID_RANDOM_LENGTH),
            )
            .finish();
    }

    let mut prefix = strip_slashes(u.path()).to_string();
    if prefix.is_empty() {
        prefix = DEFAULT_PREFIX.into()
    }

    (u, prefix)
}

#[test]
fn test_pre_url() {
    let raw = "mqtt://example.com:1883?client_id=777";
    let u = raw.parse::<Url>().unwrap();
    let (u2, p) = pre_url(u);
    assert_eq!(u2.as_str(), raw);
    assert_eq!(&p, DEFAULT_PREFIX);

    let raw = "mqtt://example.com:1883/net4mqtt?client_id=777";
    let u = raw.parse::<Url>().unwrap();
    let (u2, p) = pre_url(u);
    assert_eq!(u2.as_str(), raw);
    assert_eq!(&p, DEFAULT_PREFIX);

    let raw = "mqtt://example.com:1883/net4mqtt/?client_id=777";
    let u = raw.parse::<Url>().unwrap();
    let (u2, p) = pre_url(u);
    assert_eq!(u2.as_str(), raw);
    assert_eq!(&p, DEFAULT_PREFIX);

    let raw = "mqtt://example.com:1883/233/?client_id=777";
    let u = raw.parse::<Url>().unwrap();
    let (u2, p) = pre_url(u);
    assert_eq!(u2.as_str(), raw);
    assert_eq!(&p, "233");

    let raw = "mqtt://example.com:1883/net4mqtt/?client_id=";
    let u = raw.parse::<Url>().unwrap();
    let (u2, p) = pre_url(u);
    assert_eq!(&p, DEFAULT_PREFIX);
    assert_eq!(u2.as_str().len(), raw.len() + MQTT_CLIENT_ID_RANDOM_LENGTH);

    let raw = "mqtt://example.com:1883";
    let u = raw.parse::<Url>().unwrap();
    let (u2, p) = pre_url(u);
    assert_eq!(&p, DEFAULT_PREFIX);
    assert_eq!(
        u2.as_str().len(),
        raw.len() + MQTT_CLIENT_ID_RANDOM_LENGTH + CLIENT_ID.len() + 2
    );
}
