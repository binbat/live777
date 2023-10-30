use lazy_static::lazy_static;
use prometheus::{Gauge, Registry, TextEncoder};

lazy_static! {
    pub static ref PUBLISH: Gauge = Gauge::new("publish", "publish number").unwrap();
    pub static ref SUBSCRIBE: Gauge = Gauge::new("subscribe", "subscribe number").unwrap();
    pub static ref REGISTRY: Registry =
        Registry::new_custom(Some("live777".to_string()), None).unwrap();
    pub static ref ENCODER: TextEncoder = TextEncoder::new();
}
