use clap::Parser;
use tracing::{debug, info, warn};

mod log;
mod utils;

use tokio::net::TcpListener;

const NAME: &str = "liveman";

#[derive(Parser)]
#[command(name = "livenil", version)]
struct Args {
    /// Set config file path
    #[arg(short, long)]
    config: Option<String>,
}

pub fn parse(name: &str) -> (&str, &str, &str) {
    let mut v: Vec<&str> = name.split('.').collect();
    if v.len() < 2 {
        v[1] = ""
    };
    (v[0], v[1], v[v.len() - 1])
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mut cfg: liveman::config::Config = utils::load(
        NAME.to_string(),
        args.config.clone().map(|s| format!("{s}/liveman.toml")),
    );
    cfg.validate().unwrap();

    log::set(format!(
        "livenil={},liveman={},liveion={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level, cfg.log.level
    ));

    let mut dir_entries = tokio::fs::read_dir(args.config.unwrap()).await.unwrap();
    let mut results = Vec::new();
    while let Some(entry) = dir_entries.next_entry().await.unwrap() {
        warn!("Entry: {:?}", entry.path());
        let file_name = entry.file_name().to_str().unwrap().to_string();
        let (srv, alias, _) = parse(&file_name);
        if srv == NAME {
            continue;
        }
        let alias = alias.to_string();

        let cfg: liveion::config::Config = utils::load(
            "live777".to_string(),
            entry.path().to_str().map(|s| s.to_string()),
        );
        cfg.validate().unwrap();
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
    info!("Server shutdown");
}
