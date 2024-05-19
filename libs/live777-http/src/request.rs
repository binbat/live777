use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SelectLayer {
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
    pub streams: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Reforward {
    pub target_url: String,
    pub admin_authorization: Option<String>,
}
