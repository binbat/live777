use clap::{Parser, Subcommand};
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

mod log;
mod utils;

use livecam::config::Config;

#[derive(Parser)]
#[command(name = "livecam", version)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    GenHash {
        #[arg(short, long)]
        password: Option<String>,
    },
    Serve,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    match args.command {
        Some(Commands::GenHash { password }) => {
            gen_hash(password).await;
            return;
        }
        Some(Commands::Serve) | None => {}
    }

    let mut cfg: Config = livecam::utils::load("livecam", args.config);
    cfg.validate().unwrap();

    log::set(format!(
        "livecam={},tower_http=info,webrtc=error",
        cfg.log.level
    ));

    warn!("set log level: {}", cfg.log.level);
    debug!("load config: {:?}", cfg);

    let listener = match tokio::net::TcpListener::bind(&cfg.http.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("bind to {} failed: {}", &cfg.http.listen, e);
            return;
        }
    };
    info!("server listening on : {}", &cfg.http.listen);

    let config_arc = Arc::new(RwLock::new(cfg));

    if let Err(e) = livecam::serve(config_arc, listener, utils::shutdown_signal()).await {
        tracing::error!("server error: {}", e);
    }

    info!("Server shutdown");
}

async fn gen_hash(password: Option<String>) {
    let password = if let Some(pwd) = password {
        pwd
    } else {
        print!("Enter password: ");
        std::io::Write::flush(&mut std::io::stdout()).unwrap();

        let mut input = String::new();
        match std::io::stdin().read_line(&mut input) {
            Ok(_) => input.trim().to_string(),
            Err(e) => {
                eprintln!("Failed to read password: {}", e);
                std::process::exit(1);
            }
        }
    };

    if password.is_empty() {
        eprintln!("Password cannot be empty");
        std::process::exit(1);
    }

    match livecam::utils::generate_password_hash(&password) {
        Ok(hash) => {
            println!("Generated password hash:");
            println!("{}", hash);
        }
        Err(e) => {
            eprintln!("Failed to generate password hash: {}", e);
            std::process::exit(1);
        }
    }
}
