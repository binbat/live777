use serde::Deserialize;

#[derive(Deserialize)]
pub struct SelectLayerReq {
    #[serde(rename = "encodingId")]
    pub encoding_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ChangeResourceReq {
    pub kind: String,
    #[serde(rename = "enabled")]
    pub enabled: bool,
}

#[derive(Deserialize)]
pub struct QueryInfoReq {
    #[serde(default)]
    pub rooms: Option<String>,
}

#[derive(Deserialize)]
pub struct ReforwardReq {
    #[serde(rename = "targetUrl")]
    pub target_url: String,
    #[serde(rename = "adminAuthorization")]
    pub admin_authorization: Option<String>,
}
