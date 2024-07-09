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
pub struct Cascade {
    // server auth
    pub token: Option<String>,
    // pull mode ,value : whep_url
    pub source_url: Option<String>,
    // push mode ,value : whip_url
    pub target_url: Option<String>,
}
