use serde::Deserialize;

#[derive(Deserialize)]
pub struct SelectLayer {
    #[serde(rename = "encodingId")]
    pub encoding_id: Option<String>,
}
