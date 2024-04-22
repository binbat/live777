use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct SelectLayer {
    #[serde(rename = "encodingId")]
    pub encoding_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ChangeResource {
    pub kind: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct QueryInfo {
    #[serde(default)]
    pub streams: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Reforward {
    #[serde(rename = "targetUrl")]
    pub target_url: String,
    #[serde(rename = "adminAuthorization")]
    pub admin_authorization: Option<String>,
}
