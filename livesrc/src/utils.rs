use std::fs;
use std::path::PathBuf;
use tracing::info;

pub fn load<T>(name: &str, config_path: Option<String>) -> T
where
    T: serde::de::DeserializeOwned + Default,
{
    let config_file = if let Some(path) = config_path {
        PathBuf::from(path)
    } else {
        let mut path = PathBuf::from("conf");
        path.push(format!("{}.toml", name));
        path
    };

    if !config_file.exists() {
        info!("Config file not found, using default configuration");
        return T::default();
    }

    let content = fs::read_to_string(&config_file)
        .unwrap_or_else(|e| panic!("Failed to read config file {:?}: {}", config_file, e));

    toml::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse config file {:?}: {}", config_file, e))
}
