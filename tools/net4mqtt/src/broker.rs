use std::net::SocketAddr;

use rumqttd::{Broker, Config, ConnectionSettings, RouterConfig, ServerSettings};

fn get_mqtt_broker_config(listen: SocketAddr) -> Config {
    let name = "test-broker";
    let mut map = std::collections::HashMap::new();
    // Reference: https://github.com/bytebeamio/rumqtt/blob/2377e4e2c57bbcbdb1b2d5372556f8b3977b03b5/rumqttd/rumqttd.toml#L1-L47
    map.insert(
        name.into(),
        ServerSettings {
            name: name.into(),
            listen,
            tls: None,
            next_connection_delay_ms: 1,
            connections: ConnectionSettings {
                connection_timeout_ms: 60000,
                max_payload_size: 20480,
                max_inflight_count: 500,
                auth: None,
                external_auth: None,
                dynamic_filters: true,
            },
        },
    );
    Config {
        id: 0,
        router: RouterConfig {
            max_connections: 10010,
            max_outgoing_packet_count: 200,
            max_segment_size: 104857600,
            max_segment_count: 10,
            ..Default::default()
        },
        v4: Some(map),
        ..Default::default()
    }
}

pub fn up_mqtt_broker(listen: SocketAddr) {
    Broker::new(get_mqtt_broker_config(listen)).start().unwrap();
}
