pub const METRICS: &str = "/metrics";
pub const METRICS_JSON: &str = "/metrics/json";

pub fn whip(stream: &str) -> String {
    format!("/whip/{stream}")
}
pub fn whep(stream: &str) -> String {
    format!("/whep/{stream}")
}

pub fn whip_with_node(stream: &str, alias: &str) -> String {
    format!("/api/whip/{alias}/{stream}")
}
pub fn whep_with_node(stream: &str, alias: &str) -> String {
    format!("/api/whep/{alias}/{stream}")
}

pub fn session(stream: &str, session: &str) -> String {
    format!("/session/{stream}/{session}")
}
pub fn session_layer(stream: &str, session: &str) -> String {
    format!("/session/{stream}/{session}/layer")
}

pub fn streams(stream: &str) -> String {
    format!("/api/streams/{stream}")
}

pub fn cascade(stream: &str) -> String {
    format!("/api/cascade/{stream}")
}

pub fn streams_sse() -> &'static str {
    "/api/sse/streams"
}

pub fn strategy() -> &'static str {
    "/api/strategy/"
}

pub fn record(stream: &str) -> String {
    format!("/api/record/{stream}")
}
