use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use clap::{ArgAction, Parser, Subcommand};
use tokio::net::{TcpListener, UdpSocket};
use tracing::{debug, info, trace, Level};

use net4mqtt::proxy;

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
        /// Mqtt Broker Address (<scheme>://<host>:<port>/<prefix>?client_id=<client_id>)
        #[arg(short, long, default_value_t = format!("mqtt://localhost:1883/net4mqtt"))]
        mqtt_url: String,
        /// Listen socks5 server address
        #[arg(short, long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 6666))]
        listen: SocketAddr,
        /// Built-in DNS: (<agent_id>.<domain>)
        #[arg(short, long, default_value_t = format!("net4mqtt.local"))]
        domain: String,
        /// If DNS cannot get agent id use a default agent_id
        #[arg(short, long, default_value_t = format!("-"))]
        agent_id: String,
        /// Set Current local id
        #[arg(short, long, default_value_t = format!("-"))]
        id: String,
        /// enable kcp in mqtt
        #[arg(short, long, default_value_t = false)]
        kcp: bool,
    },

    /// use local proxy mode as client
    Local {
        /// Mqtt Broker Address (<scheme>://<host>:<port>/<prefix>?client_id=<client_id>)
        #[arg(short, long, default_value_t = format!("mqtt://localhost:1883/net4mqtt"))]
        mqtt_url: String,
        /// Listen local port mapping as agent's target address
        #[arg(short, long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 6666))]
        listen: SocketAddr,
        /// Agent's target address
        #[arg(short, long)]
        target: Option<String>,
        /// agent id
        #[arg(short, long, default_value_t = format!("-"))]
        agent_id: String,
        /// Set Current local id
        #[arg(short, long, default_value_t = format!("-"))]
        id: String,
        /// use udp port
        #[arg(short, long, default_value_t = false)]
        udp: bool,
        /// enable kcp in mqtt
        #[arg(short, long, default_value_t = false)]
        kcp: bool,
    },

    /// use agent mode as server
    Agent {
        /// Mqtt Broker Address (<scheme>://<host>:<port>/<prefix>?client_id=<client_id>)
        #[arg(short, long, default_value_t = format!("mqtt://localhost:1883/net4mqtt"))]
        mqtt_url: String,
        /// Default Agent's target address
        #[arg(short, long, default_value_t = format!("{}", SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7777)))]
        target: String,
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
            mqtt_url,
            listen,
            domain,
            agent_id,
            id,
            kcp,
        } => {
            info!("Running as socks, {:?}", listen);
            debug!("use domain: {:?}", domain);

            let listener = TcpListener::bind(listen).await.unwrap();
            proxy::local_socks(
                &mqtt_url,
                listener,
                (&agent_id, &id),
                Some(domain),
                None,
                None,
                kcp,
            )
            .await
            .unwrap();
        }
        Commands::Local {
            mqtt_url,
            listen,
            target,
            agent_id,
            id,
            udp,
            kcp,
        } => {
            info!("Running as local, {:?}", listen);

            if udp {
                let sock = UdpSocket::bind(listen).await.unwrap();
                proxy::local_ports_udp(&mqtt_url, sock, target, (&agent_id, &id), None, None)
                    .await
                    .unwrap();
            } else {
                let listener = TcpListener::bind(listen).await.unwrap();
                proxy::local_ports_tcp(
                    &mqtt_url,
                    listener,
                    target,
                    (&agent_id, &id),
                    None,
                    None,
                    kcp,
                )
                .await
                .unwrap();
            }
        }
        Commands::Agent {
            mqtt_url,
            target,
            id,
        } => {
            info!("Running as agent, {:?}", target);

            proxy::agent(&mqtt_url, &target, &id, None, None)
                .await
                .unwrap();
        }
    }
}
