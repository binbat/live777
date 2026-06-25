use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strategy {
    #[serde(default)]
    pub each_stream_max_sub: EachStreamMaxSub,
    #[serde(default)]
    pub cascade_push_close_sub: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            cascade_push_close_sub: false,
            auto_create_whip: true,
            auto_create_whep: true,
            auto_delete_whip: Default::default(),
            auto_delete_whep: Default::default(),
        }
    }
}

impl Strategy {
    /// Merge another (per-stream) strategy on top of this one.
    /// Any explicitly set field in `other` overrides the base value.
    /// For booleans and simple values this is a straight replacement;
    /// `EachStreamMaxSub` and `AutoDestrayTime` are also replaced when
    /// `other` uses a non-default value.
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            each_stream_max_sub: if other.each_stream_max_sub != EachStreamMaxSub::default() {
                other.each_stream_max_sub.clone()
            } else {
                self.each_stream_max_sub.clone()
            },
            cascade_push_close_sub: other.cascade_push_close_sub,
            auto_create_whip: other.auto_create_whip,
            auto_create_whep: other.auto_create_whep,
            auto_delete_whip: if other.auto_delete_whip != AutoDestrayTime::default() {
                other.auto_delete_whip.clone()
            } else {
                self.auto_delete_whip.clone()
            },
            auto_delete_whep: if other.auto_delete_whep != AutoDestrayTime::default() {
                other.auto_delete_whep.clone()
            } else {
                self.auto_delete_whep.clone()
            },
        }
    }

    /// Build an effective strategy from a base (global) strategy and an optional
    /// per-stream override.
    pub fn effective(base: &Self, override_strategy: Option<&Self>) -> Self {
        match override_strategy {
            Some(over) => base.merge(over),
            None => base.clone(),
        }
    }
}

/// -1: disable
/// 0: immediately destroy
/// >= 1: delay millisecond
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoDestrayTime(pub i64);

impl Default for AutoDestrayTime {
    fn default() -> Self {
        AutoDestrayTime(-1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_strategy() -> Strategy {
        Strategy {
            each_stream_max_sub: EachStreamMaxSub(10),
            cascade_push_close_sub: false,
            auto_create_whip: true,
            auto_create_whep: true,
            auto_delete_whip: AutoDestrayTime(-1),
            auto_delete_whep: AutoDestrayTime(-1),
        }
    }

    #[test]
    fn test_strategy_default() {
        let default = Strategy::default();
        assert_eq!(default.each_stream_max_sub, EachStreamMaxSub(u16::MAX));
        assert!(!default.cascade_push_close_sub);
        assert!(default.auto_create_whip);
        assert!(default.auto_create_whep);
        assert_eq!(default.auto_delete_whip, AutoDestrayTime(-1));
        assert_eq!(default.auto_delete_whep, AutoDestrayTime(-1));
    }

    #[test]
    fn test_strategy_merge_with_default_keeps_base() {
        let base = base_strategy();
        let over = Strategy::default();
        let merged = base.merge(&over);
        assert_eq!(merged.each_stream_max_sub, EachStreamMaxSub(10));
        assert!(!merged.cascade_push_close_sub);
        assert!(merged.auto_create_whip);
        assert!(merged.auto_create_whep);
        assert_eq!(merged.auto_delete_whip, AutoDestrayTime(-1));
        assert_eq!(merged.auto_delete_whep, AutoDestrayTime(-1));
    }

    #[test]
    fn test_strategy_merge_overrides_fields() {
        let base = base_strategy();
        let over = Strategy {
            each_stream_max_sub: EachStreamMaxSub(5),
            cascade_push_close_sub: true,
            auto_create_whip: false,
            auto_create_whep: false,
            auto_delete_whip: AutoDestrayTime(0),
            auto_delete_whep: AutoDestrayTime(1000),
        };
        let merged = base.merge(&over);
        assert_eq!(merged.each_stream_max_sub, EachStreamMaxSub(5));
        assert!(merged.cascade_push_close_sub);
        assert!(!merged.auto_create_whip);
        assert!(!merged.auto_create_whep);
        assert_eq!(merged.auto_delete_whip, AutoDestrayTime(0));
        assert_eq!(merged.auto_delete_whep, AutoDestrayTime(1000));
    }

    #[test]
    fn test_strategy_merge_default_time_keeps_base() {
        let base = base_strategy();
        let mut over = base.clone();
        over.auto_delete_whip = AutoDestrayTime::default();
        over.auto_delete_whep = AutoDestrayTime::default();
        let merged = base.merge(&over);
        assert_eq!(merged.auto_delete_whip, AutoDestrayTime(-1));
        assert_eq!(merged.auto_delete_whep, AutoDestrayTime(-1));
    }

    #[test]
    fn test_strategy_effective_no_override() {
        let base = base_strategy();
        let effective = Strategy::effective(&base, None);
        assert_eq!(effective, base);
    }

    #[test]
    fn test_strategy_effective_with_override() {
        let base = base_strategy();
        let over = Strategy {
            each_stream_max_sub: EachStreamMaxSub(2),
            cascade_push_close_sub: true,
            ..base.clone()
        };
        let effective = Strategy::effective(&base, Some(&over));
        assert_eq!(effective.each_stream_max_sub, EachStreamMaxSub(2));
        assert!(effective.cascade_push_close_sub);
        assert!(effective.auto_create_whip);
        assert_eq!(effective.auto_delete_whip, AutoDestrayTime(-1));
    }
}
