use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Layer {
    #[serde(rename = "encodingId")]
    pub encoding_id: String,
}
