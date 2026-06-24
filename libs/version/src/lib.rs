use shadow_rs::shadow;

shadow!(build);

pub const VERSION: &str = {
    if build::COMMITS_SINCE_TAG == 0 {
        if build::TAG.is_empty() {
            build::SHORT_COMMIT
        } else {
            build::TAG
        }
    } else if build::LAST_TAG.is_empty() {
        shadow_rs::formatcp!("{}-g{}", build::PKG_VERSION, build::SHORT_COMMIT)
    } else {
        shadow_rs::formatcp!(
            "{}-{}-g{}",
            build::LAST_TAG,
            build::COMMITS_SINCE_TAG,
            build::SHORT_COMMIT
        )
    }
};

pub use build::{
    BRANCH, BUILD_OS, BUILD_TARGET, BUILD_TIME, BUILD_TIME_3339, CARGO_FEATURES, COMMIT_HASH,
    COMMITS_SINCE_TAG, LAST_TAG, PKG_VERSION, SHORT_COMMIT, TAG,
};

pub fn full_version(features: &[&'static str]) -> &'static str {
    let s = if features.is_empty() {
        format!("{}\nbuild_time: {}", VERSION, BUILD_TIME_3339)
    } else {
        format!(
            "{}\nbuild_time: {}\nfeatures: {}",
            VERSION,
            BUILD_TIME_3339,
            features.join(", ")
        )
    };
    Box::leak(s.into_boxed_str())
}

#[macro_export]
macro_rules! version_with_features {
    ($($feature:literal),* $(,)?) => {
        $crate::full_version(&[
            $(#[cfg(feature = $feature)] $feature,)*
        ])
    };
}
