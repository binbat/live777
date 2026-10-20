use shadow_rs::shadow;

shadow!(build);

const GIT_VERSION: &str = {
    if build::COMMITS_SINCE_TAG == 0 {
        if build::TAG.is_empty() {
            if build::SHORT_COMMIT.is_empty() {
                // No .git at build time (Docker build context, source
                // tarball): fall back to the Cargo.toml version instead of
                // printing an empty version string (live777#499).
                build::PKG_VERSION
            } else {
                build::SHORT_COMMIT
            }
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

pub const VERSION: &str = {
    // An explicit compile-time override (e.g. a Docker ARG carrying `git
    // describe` output from a runner that has .git) wins over what the
    // build environment can discover on its own.
    match option_env!("LIVE777_VERSION") {
        Some(version) if !version.is_empty() => version,
        _ => GIT_VERSION,
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

#[cfg(test)]
mod tests {
    use super::*;

    // Regression for live777#499: --version printed an empty version string
    // when the build environment had no .git (Docker build context).
    #[test]
    fn version_is_never_empty() {
        assert!(!VERSION.is_empty());
        println!("VERSION: {VERSION}");
        assert!(full_version(&["webui"]).starts_with(VERSION));
        assert!(full_version(&[]).contains("build_time: "));
    }
}
