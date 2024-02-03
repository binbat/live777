use serde::Deserialize;

#[derive(Deserialize)]
pub struct SelectLayer {
    #[serde(rename = "encodingId")]
    pub encoding_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ChangeResource {
    pub kind: String,
    #[serde(rename = "enabled")]
    pub enabled: bool,
}
