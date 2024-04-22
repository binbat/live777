pub const METRICS: &str = "/metrics";
pub const METRICS_JSON: &str = "/metrics/json";
pub const ADMIN_INFOS: &str = "/admin/infos";

pub fn whip(stream: &str) -> String {
    format!("/whip/{}", stream)
}
pub fn whep(stream: &str) -> String {
    format!("/whep/{}", stream)
}

pub fn reforward(stream: &str) -> String {
    format!("/admin/reforward/{}", stream)
}

pub fn resource(stream: &str, session: &str) -> String {
    format!("/resource/{}/{}", stream, session)
}
pub fn resource_layer(stream: &str, session: &str) -> String {
    format!("/resource/{}/{}/layer", stream, session)
}

pub fn infos(streams: Vec<String>) -> String {
    format!("/admin/infos?streams={}", streams.join(","))
}
