use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Strategy {
    #[serde(default)]
    pub each_stream_max_sub: EachStreamMaxSub,
    #[serde(default)]
    pub reforward_close_sub: bool,
    #[serde(default = "default_true")]
    pub auto_create_whip: bool,
    #[serde(default = "default_true")]
    pub auto_create_whep: bool,
    #[serde(default)]
    pub auto_delete_whip: AutoDestrayTime,
    #[serde(default)]
    pub auto_delete_whep: AutoDestrayTime,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EachStreamMaxSub(pub u16);

impl Default for EachStreamMaxSub {
    fn default() -> Self {
        EachStreamMaxSub(u16::MAX)
    }
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            each_stream_max_sub: Default::default(),
            reforward_close_sub: false,
            auto_create_whip: true,
            auto_create_whep: true,
            auto_delete_whip: Default::default(),
            auto_delete_whep: Default::default(),
        }
    }
}

/// -1: disable
/// 0: immediately destroy
/// >= 1: delay millisecond
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDestrayTime(pub i64);

impl Default for AutoDestrayTime {
    fn default() -> Self {
        AutoDestrayTime(-1)
    }
}
