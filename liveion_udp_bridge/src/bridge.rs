use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::datachannel_client::DataChannelClient;
use crate::udp_server::{UdpMessage, UdpServer};

pub struct UdpDataChannelBridge {
    config: Config,
    udp_server: UdpServer,
    datachannel_client: DataChannelClient,
    // Track UDP clients for bidirectional communication
    udp_clients: HashMap<String, SocketAddr>,
}

impl UdpDataChannelBridge {
    pub async fn new(config: Config) -> Result<Self> {
        let udp_server = UdpServer::new(
            &config.udp.listen,
            config.udp.port,
            config.bridge.max_message_size,
            config.bridge.enable_logging,
        ).await?;
        
        let datachannel_client = DataChannelClient::new(config.liveion.clone());
        
        Ok(Self {
            config,
            udp_server,
            datachannel_client,
            udp_clients: HashMap::new(),
        })
    }
    
    pub async fn run(self) -> Result<()> {
        // Create channels for communication between UDP and DataChannel
        let (udp_inbound_tx, mut udp_inbound_rx) = mpsc::channel::<UdpMessage>(100);
        let (udp_outbound_tx, udp_outbound_rx) = mpsc::channel::<UdpMessage>(100);
        
        let (dc_inbound_tx, mut dc_inbound_rx) = mpsc::channel::<Vec<u8>>(100);
        let (dc_outbound_tx, dc_outbound_rx) = mpsc::channel::<Vec<u8>>(100);
        
        info!("Starting UDP-DataChannel bridge");
        println!("🔗 Starting UDP-DataChannel bridge components");
        
        // Start UDP server
        let udp_task = {
            let udp_server = self.udp_server;
            tokio::spawn(async move {
                if let Err(e) = udp_server.run(udp_outbound_rx, udp_inbound_tx).await {
                    error!("UDP server error: {}", e);
                }
            })
        };
        
        // Start DataChannel client
        let dc_task = {
            let mut datachannel_client = self.datachannel_client;
            tokio::spawn(async move {
                if let Err(e) = datachannel_client.connect(dc_inbound_tx, dc_outbound_rx).await {
                    error!("DataChannel client error: {}", e);
                }
            })
        };
        
        // Bridge messages between UDP and DataChannel
        let mut bridge_state = BridgeState {
            udp_clients: self.udp_clients,
            config: self.config,
        };
        
        let bridge_task = tokio::spawn(async move {
            println!("Bridge task started, waiting for messages...");
            loop {
                tokio::select! {
                    // UDP -> DataChannel
                    Some(udp_msg) = udp_inbound_rx.recv() => {
                        println!("[Bridge] Received UDP message: {} bytes from {}", udp_msg.data.len(), udp_msg.addr);
                        bridge_state.handle_udp_to_datachannel(udp_msg, &dc_outbound_tx).await;
                    }
                    
                    // DataChannel -> UDP
                    Some(dc_data) = dc_inbound_rx.recv() => {
                        println!("[Bridge] Received DataChannel message: {} bytes", dc_data.len());
                        println!("   Content: {:?}", String::from_utf8_lossy(&dc_data));
                        bridge_state.handle_datachannel_to_udp(dc_data, &udp_outbound_tx).await;
                    }
                }
            }
        });
        
        // Wait for any task to complete (or fail)
        tokio::select! {
            result = udp_task => {
                error!("UDP task ended: {:?}", result);
            }
            result = dc_task => {
                error!("DataChannel task ended: {:?}", result);
            }
            result = bridge_task => {
                error!("Bridge task ended: {:?}", result);
            }
        }
        
        Ok(())
    }
}

struct BridgeState {
    udp_clients: HashMap<String, SocketAddr>,
    config: Config,
}

impl BridgeState {
    
    async fn handle_udp_to_datachannel(
        &mut self,
        udp_msg: UdpMessage,
        dc_outbound_tx: &mpsc::Sender<Vec<u8>>,
    ) {
        // Store the UDP client address for potential responses
        let client_id = format!("{}:{}", udp_msg.addr.ip(), udp_msg.addr.port());
        self.udp_clients.insert(client_id.clone(), udp_msg.addr);
        
        // Create a message that includes the UDP client info
        let bridge_message = serde_json::json!({
            "type": "udp_to_datachannel",
            "client_id": client_id,
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "data": String::from_utf8_lossy(&udp_msg.data)
        });
        
        let message_bytes = bridge_message.to_string().into_bytes();
        
        if let Err(e) = dc_outbound_tx.send(message_bytes).await {
            error!("Failed to forward UDP message to DataChannel: {}", e);
        } else {
            if self.config.bridge.enable_logging {
                debug!("Forwarded UDP message from {} to DataChannel", udp_msg.addr);
            }
        }
    }
    
    async fn handle_datachannel_to_udp(
        &mut self,
        dc_data: Vec<u8>,
        udp_outbound_tx: &mpsc::Sender<UdpMessage>,
    ) {
        // Try to parse the DataChannel message
        let message_str = match String::from_utf8(dc_data.clone()) {
            Ok(s) => s,
            Err(_) => {
                // If it's not UTF-8, treat as raw binary data
                warn!("Received non-UTF-8 data from DataChannel, treating as raw binary");
                self.broadcast_to_all_udp_clients(dc_data, udp_outbound_tx).await;
                return;
            }
        };
        
        if self.config.bridge.enable_logging {
            debug!("DataChannel received: {}", message_str);
        }
        
        // Try to parse as JSON
        match serde_json::from_str::<serde_json::Value>(&message_str) {
            Ok(json_msg) => {
                if self.config.bridge.enable_logging {
                    debug!("Parsed DataChannel JSON: {}", json_msg);
                }
                self.handle_structured_datachannel_message(json_msg, udp_outbound_tx).await;
            }
            Err(_) => {
                // Not JSON, treat as plain text command
                if self.config.bridge.enable_logging {
                    debug!("Received plain text from DataChannel: {}", message_str);
                }
                self.broadcast_to_all_udp_clients(message_str.into_bytes(), udp_outbound_tx).await;
            }
        }
    }
    
    async fn handle_structured_datachannel_message(
        &mut self,
        json_msg: serde_json::Value,
        udp_outbound_tx: &mpsc::Sender<UdpMessage>,
    ) {
        let msg_type = json_msg.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
        
        match msg_type {
            "datachannel_to_udp" => {
                // Message specifically intended for UDP
                info!("Processing datachannel_to_udp message: {}", json_msg);
                
                if let Some(target_client) = json_msg.get("target_client").and_then(|v| v.as_str()) {
                    // Send to specific UDP client
                    if let Some(&addr) = self.udp_clients.get(target_client) {
                        let data = json_msg.get("data")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .as_bytes()
                            .to_vec();
                        
                        info!("Sending targeted message to UDP client {}: {} bytes", target_client, data.len());
                        let udp_msg = UdpMessage { data, addr };
                        if let Err(e) = udp_outbound_tx.send(udp_msg).await {
                            error!("Failed to send targeted UDP message: {}", e);
                        } else {
                            info!("�?Successfully sent targeted message to UDP client {}", target_client);
                        }
                    } else {
                        warn!("Target UDP client not found: {}", target_client);
                    }
                } else {
                    // Broadcast to all UDP clients
                    let data_str = json_msg.get("data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let data = data_str.as_bytes().to_vec();
                    
                    info!("Broadcasting datachannel_to_udp message: '{}' ({} bytes)", data_str, data.len());
                    info!("Known UDP clients: {}, Default targets: {}", 
                          self.udp_clients.len(), self.config.udp.target_addresses.len());
                    
                    self.broadcast_to_all_udp_clients(data, udp_outbound_tx).await;
                }
            }
            "keepalive" => {
                // Ignore keepalive messages
                if self.config.bridge.enable_logging {
                    debug!("Received keepalive from DataChannel");
                }
            }
            "bridge_test" => {
                // Respond to bridge test messages
                if self.config.bridge.enable_logging {
                    debug!("Received bridge test from DataChannel");
                }
                
                // Send response back to DataChannel
                let response = serde_json::json!({
                    "type": "bridge_response",
                    "status": "ok",
                    "timestamp": chrono::Utc::now().timestamp_millis(),
                    "message": "UDP bridge is running"
                });
                
                // We need to send this back through the DataChannel, but we don't have access to dc_outbound_tx here
                // For now, we'll just log it. The Web interface will detect the bridge is working by other means.
                info!("Bridge test received - UDP bridge is operational");
            }
            _ => {
                // Unknown structured message, broadcast as JSON string
                if self.config.bridge.enable_logging {
                    debug!("Received structured message from DataChannel: {}", msg_type);
                }
                let data = json_msg.to_string().into_bytes();
                self.broadcast_to_all_udp_clients(data, udp_outbound_tx).await;
            }
        }
    }
    
    async fn broadcast_to_all_udp_clients(
        &self,
        data: Vec<u8>,
        udp_outbound_tx: &mpsc::Sender<UdpMessage>,
    ) {
        let mut sent_count = 0;
        let data_str = String::from_utf8_lossy(&data);
        
        info!("Starting broadcast of message: '{}' ({} bytes)", data_str, data.len());
        
        // First, try to send to known UDP clients
        for (client_id, &addr) in &self.udp_clients {
            let udp_msg = UdpMessage {
                data: data.clone(),
                addr,
            };
            
            info!("Sending to known client {}: {}", client_id, addr);
            if let Err(e) = udp_outbound_tx.send(udp_msg).await {
                error!("Failed to broadcast UDP message to {}: {}", addr, e);
            } else {
                sent_count += 1;
                info!("�?Successfully sent to known client {}: {}", client_id, addr);
            }
        }
        
        // If no known clients, use default target addresses
        if sent_count == 0 && !self.config.udp.target_addresses.is_empty() {
            info!("No known clients, using default targets: {:?}", self.config.udp.target_addresses);
            for target_addr_str in &self.config.udp.target_addresses {
                if let Ok(addr) = target_addr_str.parse::<std::net::SocketAddr>() {
                    let udp_msg = UdpMessage {
                        data: data.clone(),
                        addr,
                    };
                    
                    info!("Sending to default target: {}", addr);
                    if let Err(e) = udp_outbound_tx.send(udp_msg).await {
                        error!("Failed to send UDP message to default target {}: {}", addr, e);
                    } else {
                        sent_count += 1;
                        info!("�?Successfully sent to default target: {}", addr);
                    }
                } else {
                    warn!("Invalid target address format: {}", target_addr_str);
                }
            }
        }
        
        if sent_count > 0 {
            info!("Broadcast complete: sent to {} UDP targets", sent_count);
        } else {
            error!("Broadcast failed: no UDP targets available - known clients: {}, default targets: {}", 
                  self.udp_clients.len(), self.config.udp.target_addresses.len());
        }
    }
}