pub const METRICS: &str = "/metrics";
pub const METRICS_JSON: &str = "/metrics/json";

pub fn whip(stream: &str) -> String {
    format!("/whip/{}", stream)
}
pub fn whep(stream: &str) -> String {
    format!("/whep/{}", stream)
}

pub fn session(stream: &str, session: &str) -> String {
    format!("/session/{}/{}", stream, session)
}
pub fn session_layer(stream: &str, session: &str) -> String {
    format!("/session/{}/{}/layer", stream, session)
}

pub fn streams(stream: &str) -> String {
    format!("/api/streams/{}", stream)
}

pub fn cascade(stream: &str) -> String {
    format!("/api/cascade/{}", stream)
}
