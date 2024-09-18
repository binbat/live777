use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use clap::{ArgAction, Parser, Subcommand};
use tracing::{debug, info, trace, Level};
use url::Url;

mod proxy;
mod socks;
mod topic;

#[cfg(test)]
mod broker;

#[cfg(test)]
mod tests;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,

    /// Mqtt Broker Address
    #[arg(short, long, default_value_t = format!("mqtt://localhost:1883"))]
    broker: String,

    /// Mqtt Topic Prefix
    #[arg(short, long, default_value_t = format!("net4mqtt"))]
    prefix: String,

    /// Mqtt Client Id
    #[arg(short, long, default_value_t = format!("-"))]
    client_id: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// use local proxy
    Local {
        /// socks5 proxy
        #[arg(short, long, default_value_t = false)]
        socks: bool,
        /// lists test values
        #[arg(short, long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 6666))]
        target: SocketAddr,
        /// Optional name to operate on
        #[arg(short, long, default_value_t = format!("-"))]
        agent_id: String,
        /// Optional name to operate on
        #[arg(short, long, default_value_t = format!("-"))]
        local_id: String,
        /// lists test values
        #[arg(short, long, default_value_t = false)]
        no_kcp: bool,
    },

    /// use agent proxy
    Agent {
        /// lists test values
        #[arg(short, long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7777))]
        target: SocketAddr,
        /// Optional name to operate on
        #[arg(short, long, default_value_t = format!("-"))]
        agent_id: String,
    },
}

#[tokio::main]
async fn main() {
    let args = Cli::parse();

    utils::set_log(format!(
        "net4mqtt={}",
        match args.verbose {
            0 => Level::WARN,
            1 => Level::INFO,
            2 => Level::DEBUG,
            _ => Level::TRACE,
        }
    ));

    trace!("{:?}", args);

    let mqtt_broker_url = args.broker.parse::<Url>().unwrap();
    let mqtt_broker_host = mqtt_broker_url.host().unwrap();
    let mqtt_broker_port = mqtt_broker_url.port().unwrap_or(1883);
    let mqtt_client_id = args.client_id;
    let mqtt_topic_prefix = args.prefix;
    debug!("mqtt_broker_url {:?}", mqtt_broker_url);

    match args.command {
        Commands::Local {
            socks,
            target,
            agent_id,
            local_id,
            no_kcp,
        } => {
            info!("Running as local, {:?}", target);

            if socks {
                proxy::local_socks(
                    &proxy::MqttConfig {
                        id: mqtt_client_id,
                        host: mqtt_broker_host.to_string(),
                        port: mqtt_broker_port,
                    },
                    target,
                    &mqtt_topic_prefix,
                    &agent_id,
                    &local_id,
                    !no_kcp,
                )
                .await;
            } else {
                proxy::local(
                    &proxy::MqttConfig {
                        id: mqtt_client_id,
                        host: mqtt_broker_host.to_string(),
                        port: mqtt_broker_port,
                    },
                    target,
                    &mqtt_topic_prefix,
                    &agent_id,
                    &local_id,
                    !no_kcp,
                )
                .await;
            }
        }
        Commands::Agent { target, agent_id } => {
            info!("Running as agent, {:?}", target);

            proxy::agent(
                &proxy::MqttConfig {
                    id: mqtt_client_id,
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                target,
                &mqtt_topic_prefix,
                &agent_id,
            )
            .await;
        }
    }
}
