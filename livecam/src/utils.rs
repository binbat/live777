use anyhow::Context;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use config::{Config as ConfigRs, File, FileFormat};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::Path;
use tracing::{info, warn};

const ETC_CONFIG_PATH: &str = "/etc/live777/";
const VAR_CONFIG_PATH: &str = "/var/lib/live777/";

pub fn load<T: DeserializeOwned>(name: &str, config_path_override: Option<String>) -> T {
    let current_dir_path = std::env::current_dir()
        .context("Failed to get current working directory")
        .unwrap()
        .join(format!("{}.toml", name));

    let etc_path = Path::new(ETC_CONFIG_PATH).join(format!("{}.toml", name));
    let var_path = Path::new(VAR_CONFIG_PATH).join(format!("{}.toml", name));

    let mut builder = ConfigRs::builder()
        .set_default("http.listen", "0.0.0.0:9999")
        .unwrap()
        .set_default("log.level", "info")
        .unwrap()
        .set_default("auth.username", "admin")
        .unwrap()
        .set_default(
            "auth.password_hash",
            "$argon2id$v=19$m=19456,t=2,p=1$bmljZXRyeQ$PqTT/n9ToBNVsdsoquTz1A/P5s9O4yvA9fym5Vd5s9s",
        )
        .unwrap()
        .set_default("cameras", Vec::<String>::new())
        .unwrap();

    if current_dir_path.exists() {
        builder = builder.add_source(File::from(current_dir_path.clone()).required(false));
        info!(
            "Loaded config from current directory: {}",
            current_dir_path.display()
        );
    } else {
        warn!(
            "Config not found in current directory at {}, checking other sources.",
            current_dir_path.display()
        );
    }

    if etc_path.exists() {
        builder = builder.add_source(File::from(etc_path.clone()).required(false));
        info!("Loaded base config from {}", etc_path.display());
    } else {
        warn!(
            "Base config not found at {}, using internal defaults.",
            etc_path.display()
        );
    }

    if var_path.exists() {
        builder = builder.add_source(File::from(var_path.clone()).required(false));
        info!("Loaded user override config from {}", var_path.display());
    }

    if let Some(path) = config_path_override {
        builder = builder.add_source(File::new(&path, FileFormat::Toml).required(true));
        info!("Loaded override config from command line: {}", path);
    }

    builder
        .build()
        .context("Failed to build configuration")
        .unwrap()
        .try_deserialize()
        .context("Failed to deserialize configuration")
        .unwrap()
}

pub fn save_config<T: Serialize>(name: &str, config: &T) -> anyhow::Result<()> {
    let dir_path = Path::new(VAR_CONFIG_PATH);
    std::fs::create_dir_all(dir_path)?;

    let path = dir_path.join(format!("{}.toml", name));
    let temp_path = dir_path.join(format!("{}.toml.tmp", name));

    let toml_string = toml::to_string_pretty(config)?;

    std::fs::write(&temp_path, toml_string)?;
    std::fs::rename(&temp_path, &path)?;

    info!("Configuration saved to {}", path.display());
    Ok(())
}

pub fn reset_config(name: &str) -> anyhow::Result<()> {
    let path = Path::new(VAR_CONFIG_PATH).join(format!("{}.toml", name));
    if path.exists() {
        std::fs::remove_file(&path)?;
        info!("Configuration reset by removing {}", path.display());
    } else {
        info!(
            "No user configuration found at {}, nothing to reset.",
            path.display()
        );
    }
    Ok(())
}

pub fn generate_password_hash(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();

    match argon2.hash_password(password.as_bytes(), &salt) {
        Ok(hash) => Ok(hash.to_string()),
        Err(e) => Err(anyhow::anyhow!("Failed to generate password hash: {}", e)),
    }
}

#[cfg(test)]
mod password_tests {
    use super::*;
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    #[test]
    fn test_generate_and_verify_password() {
        let password = "test123";
        let hash = generate_password_hash(password).unwrap();

        let parsed_hash = PasswordHash::new(&hash).unwrap();
        let result = Argon2::default().verify_password(password.as_bytes(), &parsed_hash);
        assert!(result.is_ok());
    }
}
