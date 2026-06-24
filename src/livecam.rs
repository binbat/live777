use std::path::Path;
use std::sync::{Arc, RwLock};

use clap::{Parser, Subcommand};
use tracing::{debug, info, warn};

mod log;
mod utils;

#[derive(Parser)]
#[command(name = "livecam", version = version::version_with_features!(
    "webui",
    "net4mqtt",
))]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Set config file path
    #[arg(short, long, default_value_t = format!("{}.toml", "livecam"))]
    config: String,
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
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Some(Commands::GenHash { password }) => {
            gen_hash(password).await;
            return Ok(());
        }
        Some(Commands::Serve) | None => {}
    }

    let path = Path::new(&args.config);
    let mut cfg: livecam::config::Config = if path.try_exists()? {
        toml::from_str(std::fs::read_to_string(path)?.as_str())?
    } else {
        eprintln!("=== No any config file, use default config ===");
        Default::default()
    };

    cfg.validate()?;

    log::set(format!(
        "livecam={},tower_http=info,webrtc=error",
        cfg.log.level
    ));

    warn!("set log level: {}", cfg.log.level);
    debug!("load config: {:?}", cfg);

    let listener = tokio::net::TcpListener::bind(&cfg.http.listen).await?;
    info!("server listening on : {}", &cfg.http.listen);

    let config_arc = Arc::new(RwLock::new(cfg));

    livecam::serve(config_arc, listener, utils::shutdown_signal()).await?;
    info!("Server shutdown");

    Ok(())
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
