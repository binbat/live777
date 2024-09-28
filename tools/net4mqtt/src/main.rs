use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use clap::{ArgAction, Parser, Subcommand};
use tracing::{debug, info, trace, Level};

use netmqtt::proxy;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// use local socks5 mode as client
    Socks {
        /// Mqtt Broker Address
        #[arg(short, long, default_value_t = format!("mqtt://localhost:1883/net4mqtt"))]
        broker: String,
        /// Listen socks5 server address
        #[arg(short, long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 6666))]
        listen: SocketAddr,
        /// Built-in DNS: <agent_id>.mqtt.local
        #[arg(short, long, default_value_t = format!("mqtt.local"))]
        domain: String,
        /// If DNS cannot get agent id use a default agent_id
        #[arg(short, long, default_value_t = format!("-"))]
        agent_id: String,
        /// Set Current local id
        #[arg(short, long, default_value_t = format!("-"))]
        id: String,
        /// enable kcp in mqtt
        #[arg(short, long, default_value_t = false)]
        no_kcp: bool,
    },

    /// use local proxy mode as client
    Local {
        /// Mqtt Broker Address
        #[arg(short, long, default_value_t = format!("mqtt://localhost:1883/net4mqtt"))]
        broker: String,
        /// Listen local port mapping as agent's target address
        #[arg(short, long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 6666))]
        listen: SocketAddr,
        /// agent id
        #[arg(short, long, default_value_t = format!("-"))]
        agent_id: String,
        /// Set Current local id
        #[arg(short, long, default_value_t = format!("-"))]
        id: String,
        /// enable kcp in mqtt
        #[arg(short, long, default_value_t = false)]
        no_kcp: bool,
    },

    /// use agent mode as server
    Agent {
        /// Mqtt Broker Address
        #[arg(short, long, default_value_t = format!("mqtt://localhost:1883/net4mqtt"))]
        broker: String,
        /// Agent's target address
        #[arg(short, long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7777))]
        target: SocketAddr,
        /// Set Current agent id
        #[arg(short, long, default_value_t = format!("-"))]
        id: String,
    },
}

#[tokio::main]
async fn main() {
    let args = Cli::parse();

    utils::set_log(format!(
        "net4mqtt={},netmqtt=trace",
        match args.verbose {
            0 => Level::WARN,
            1 => Level::INFO,
            2 => Level::DEBUG,
            _ => Level::TRACE,
        }
    ));

    trace!("{:?}", args);
    match args.command {
        Commands::Socks {
            broker,
            listen,
            domain,
            agent_id,
            id,
            no_kcp,
        } => {
            info!("Running as socks, {:?}", listen);
            debug!("use domain: {:?}", domain);

            proxy::local_socks(&broker, listen, &agent_id, &id, None, None, !no_kcp)
                .await
                .unwrap();
        }
        Commands::Local {
            broker,
            listen,
            agent_id,
            id,
            no_kcp,
        } => {
            info!("Running as local, {:?}", listen);

            proxy::local(&broker, listen, &agent_id, &id, None, None, !no_kcp)
                .await
                .unwrap();
        }
        Commands::Agent { broker, target, id } => {
            info!("Running as agent, {:?}", target);

            proxy::agent(&broker, target, &id, None, None)
                .await
                .unwrap();
        }
    }
}
