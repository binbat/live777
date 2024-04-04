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
    pub paths: Option<String>,
}

#[derive(Deserialize)]
pub struct ReforwardReq {
    #[serde(rename = "whipUrl")]
    pub whip_url: String,
    pub basic: Option<String>,
    pub token: Option<String>,
}
