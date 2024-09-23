use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use clap::{ArgAction, Parser, Subcommand};
use tracing::{debug, info, trace, Level};
use url::Url;

use netmqtt::proxy;

/// Reference: https://docs.oasis-open.org/mqtt/mqtt/v5.0/mqtt-v5.0.html
/// The Server MUST allow ClientID’s which are between 1 and 23 UTF-8 encoded bytes in length, and that contain only the characters
/// "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
const MQTT_CLIENT_ID_RANDOM_LENGTH: usize = 7;

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
        /// Mqtt Client Id
        #[arg(short, long, default_value_t = generate_random_string(MQTT_CLIENT_ID_RANDOM_LENGTH))]
        client_id: String,
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
        /// Mqtt Client Id
        #[arg(short, long, default_value_t = generate_random_string(MQTT_CLIENT_ID_RANDOM_LENGTH))]
        client_id: String,
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
        /// Mqtt Client Id
        #[arg(short, long, default_value_t = generate_random_string(MQTT_CLIENT_ID_RANDOM_LENGTH))]
        client_id: String,
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
        "net4mqtt={}",
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
            client_id,
            listen,
            domain,
            agent_id,
            id,
            no_kcp,
        } => {
            info!("Running as socks, {:?}", listen);
            debug!("use domain: {:?}", domain);

            let mqtt_broker_url = broker.parse::<Url>().unwrap();
            let mqtt_broker_host = mqtt_broker_url.host().unwrap();
            let mqtt_broker_port = mqtt_broker_url.port().unwrap_or(1883);
            let mqtt_topic_prefix = strip_slashes(mqtt_broker_url.path());

            proxy::local_socks(
                &proxy::MqttConfig {
                    id: client_id,
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                listen,
                mqtt_topic_prefix,
                &agent_id,
                &id,
                !no_kcp,
            )
            .await;
        }
        Commands::Local {
            broker,
            client_id,
            listen,
            agent_id,
            id,
            no_kcp,
        } => {
            info!("Running as local, {:?}", listen);

            let mqtt_broker_url = broker.parse::<Url>().unwrap();
            let mqtt_broker_host = mqtt_broker_url.host().unwrap();
            let mqtt_broker_port = mqtt_broker_url.port().unwrap_or(1883);
            let mqtt_topic_prefix = strip_slashes(mqtt_broker_url.path());

            proxy::local(
                &proxy::MqttConfig {
                    id: client_id,
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                listen,
                mqtt_topic_prefix,
                &agent_id,
                &id,
                !no_kcp,
            )
            .await;
        }
        Commands::Agent {
            broker,
            client_id,
            target,
            id,
        } => {
            info!("Running as agent, {:?}", target);

            let mqtt_broker_url = broker.parse::<Url>().unwrap();
            let mqtt_broker_host = mqtt_broker_url.host().unwrap();
            let mqtt_broker_port = mqtt_broker_url.port().unwrap_or(1883);
            let mqtt_topic_prefix = strip_slashes(mqtt_broker_url.path());

            proxy::agent(
                &proxy::MqttConfig {
                    id: client_id,
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                target,
                mqtt_topic_prefix,
                &id,
            )
            .await;
        }
    }
}

use rand::Rng;

fn generate_random_string(length: usize) -> String {
    let charset: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();

    let random_string: String = (0..length)
        .map(|_| {
            let idx = rng.gen_range(0..charset.len());
            charset[idx] as char
        })
        .collect();

    random_string
}

fn strip_slashes(path: &str) -> &str {
    let mut start = 0;
    let mut end = path.len();

    if path.starts_with("/") {
        start = 1;
    }

    if path.ends_with("/") {
        end -= 1;
    }

    &path[start..end]
}
