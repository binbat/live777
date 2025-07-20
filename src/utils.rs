use tracing::{debug, error};

pub async fn shutdown_signal() {
    let str = signal::wait_for_stop_signal().await;
    debug!("Received signal: {}", str);
}

pub fn load<T>(name: String, path: Option<String>) -> T
where
    T: serde::de::DeserializeOwned + std::default::Default,
{
    use std::fs::read_to_string;
    let result = read_to_string(path.unwrap_or(format!("{name}.toml")))
        .or(read_to_string(format!("/etc/live777/{name}.toml")))
        .unwrap_or("".to_string());
    match toml::from_str(result.as_str()) {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("config load error: {}", err);
            Default::default()
        }
    }
}
