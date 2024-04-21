use serde::Deserialize;

#[derive(Deserialize)]
pub struct SelectLayer {
    #[serde(rename = "encodingId")]
    pub encoding_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ChangeResource {
    pub kind: String,
    pub enabled: bool,
}

#[derive(Deserialize)]
pub struct QueryInfo {
    #[serde(default)]
    pub streams: Option<String>,
}

#[derive(Deserialize)]
pub struct Reforward {
    #[serde(rename = "targetUrl")]
    pub target_url: String,
    #[serde(rename = "adminAuthorization")]
    pub admin_authorization: Option<String>,
}
