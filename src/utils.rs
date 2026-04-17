use tracing::debug;

pub async fn shutdown_signal() {
    let str = signal::wait_for_stop_signal().await;
    debug!("Received signal: {}", str);
}

#[allow(dead_code)]
pub fn load<T>(name: String, path_opt: Option<String>) -> T
where
    T: serde::de::DeserializeOwned + std::default::Default,
{
    use std::fs::read_to_string;
    use std::process::exit;

    let content = if let Some(p) = path_opt {
        // User explicitly specified a path, it MUST exist
        match read_to_string(&p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("CRITICAL: Failed to read config file at '{}': {}", p, e);
                exit(1);
            }
        }
    } else {
        // Default lookup logic
        read_to_string(format!("{name}.toml"))
            .or_else(|_| read_to_string(format!("/etc/live777/{name}.toml")))
            .unwrap_or_else(|_| "".to_string())
    };

    match toml::from_str(&content) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("CRITICAL: Config parsing error: {}", err);
            exit(1);
        }
    }
}
