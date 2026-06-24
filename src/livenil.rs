use std::path::Path;

use anyhow::Result;
use clap::Parser;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

mod log;
mod utils;

const NAME: &str = "liveman";

#[derive(Parser)]
#[command(name = "livenil", version = version::version_with_features!(
    "webui",
    "cascade",
    "net4mqtt",
    "recorder",
    "source",
    "source-sdp",
    "source-rtsp",
    "source-all",
    "native-source",
    "capture-libcamera",
    "capture-v4l2",
    "encoder-v4l2-m2m",
    "encoder-rdk",
    "native-rpi",
    "native-generic-v4l2",
    "native-rdk",
))]
struct Args {
    /// Set config file path
    #[arg(short, long, default_value_t = format!("livenil"))]
    config: String,
}

pub fn parse(name: &str) -> (&str, &str, &str) {
    let v: Vec<&str> = name.split('.').collect();
    match v.len() {
        1 => (v[0], "", ""),
        2 => (v[0], v[1], v[1]),
        _ => (v[0], v[1], v[v.len() - 1]),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = format!("{}/liveman.toml", args.config);
    let path = Path::new(&config_path);

    let mut cfg: liveman::config::Config = if path.try_exists()? {
        toml::from_str(std::fs::read_to_string(path)?.as_str())?
    } else {
        eprintln!("=== No any config file, use default config ===");
        Default::default()
    };

    cfg.validate()?;

    log::set(format!(
        "livenil={},liveman={},liveion={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level, cfg.log.level
    ));

    debug!("config : {:?}", cfg);

    let mut dir_entries = tokio::fs::read_dir(args.config).await.unwrap();
    let mut results = Vec::new();
    while let Some(entry) = dir_entries.next_entry().await.unwrap() {
        warn!("Entry: {:?}", entry.path());
        let file_name = entry.file_name().to_str().unwrap().to_string();
        if !file_name.ends_with(".toml") {
            continue;
        }

        let (srv, alias, _) = parse(&file_name);
        if srv == NAME {
            continue;
        }
        let alias = alias.to_string();

        let cfg: liveion::config::Config =
            toml::from_str(std::fs::read_to_string(entry.path())?.as_str())?;

        cfg.validate()?;
        debug!("config : {:?}", cfg);
        let listener = TcpListener::bind(&cfg.http.listen).await.unwrap();
        let addr = listener.local_addr().unwrap();
        info!("Server listening on {}", addr);
        results.push((alias, addr));

        tokio::spawn(liveion::serve(cfg, listener, utils::shutdown_signal()));
    }

    cfg.nodes.extend(
        results
            .into_iter()
            .map(|(alias, addr)| liveman::config::Node {
                alias: alias.to_string(),
                url: format!("http://{addr}"),
                ..Default::default()
            }),
    );

    let listener = TcpListener::bind(cfg.http.listen).await.unwrap();

    liveman::serve(cfg, listener, utils::shutdown_signal()).await;
    info!("Server graceful shutdown completed");

    Ok(())
}
