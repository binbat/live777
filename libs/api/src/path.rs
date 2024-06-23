use crate::request::QueryInfo;

pub const METRICS: &str = "/metrics";
pub const METRICS_JSON: &str = "/metrics/json";
pub const ADMIN_INFOS: &str = "/admin/infos";

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

pub fn infos(qry: QueryInfo) -> String {
    let query = serde_html_form::to_string(qry).unwrap();
    format!("/admin/infos?{}", query)
}

pub fn cascade(stream: &str) -> String {
    format!("/admin/cascade/{}", stream)
}
