use anyhow::{anyhow, Result};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::ice::mdns::MulticastDnsMode;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;

use crate::config::LiveionConfig;

pub struct DataChannelClient {
    config: LiveionConfig,
    http_client: Client,
    auth_token: Option<String>,
}

impl DataChannelClient {
    pub fn new(config: LiveionConfig) -> Self {
        Self {
            config,
            http_client: Client::new(),
            auth_token: None,
        }
    }
    
    pub async fn connect(
        &mut self,
        inbound_tx: mpsc::Sender<Vec<u8>>,
        mut outbound_rx: mpsc::Receiver<Vec<u8>>,
    ) -> Result<()> {
        loop {
            match self.try_connect(inbound_tx.clone(), &mut outbound_rx).await {
                Ok(_) => {
                    info!("DataChannel connection ended normally");
                    break;
                }
                Err(e) => {
                    error!("DataChannel connection failed: {}", e);
                    info!("Retrying connection in 5 seconds...");
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
        Ok(())
    }
    
    async fn try_connect(
        &mut self,
        inbound_tx: mpsc::Sender<Vec<u8>>,
        outbound_rx: &mut mpsc::Receiver<Vec<u8>>,
    ) -> Result<()> {
        // Authenticate if needed
        if self.config.auth.is_some() {
            self.authenticate().await?;
        }
        
        // Create WebRTC peer connection
        let pc = self.create_peer_connection().await?;
        
        // Create data channel
        println!("Creating DataChannel with label 'control'");
        let dc = pc.create_data_channel("control", None).await?;
        println!("DataChannel created successfully, ID: {:?}, Label: {}", dc.id(), dc.label());
        let dc_clone = dc.clone();
        
        // Set up data channel handlers
        let dc_id = dc.id();
        dc.on_open(Box::new(move || {
            info!("DataChannel opened");
            println!("DataChannel opened [ID: {:?}]", dc_id);
            println!("   - Ready to receive messages");
            Box::pin(async {})
        }));
        
        dc.on_close(Box::new(move || {
            warn!("DataChannel closed");
            println!("DataChannel closed");
            Box::pin(async {})
        }));
        
        dc.on_error(Box::new(move |err| {
            error!("DataChannel error: {}", err);
            println!("DataChannel error: {}", err);
            Box::pin(async {})
        }));
        
        // Also listen for server-created DataChannels
        let inbound_tx_server = inbound_tx.clone();
        pc.on_data_channel(Box::new(move |d| {
            let tx = inbound_tx_server.clone();
            println!("Received server DataChannel:");
            println!("   - Label: {}", d.label());
            println!("   - ID: {:?}", d.id());
            println!("   - ReadyState: {:?}", d.ready_state());
            
            let d_clone = d.clone();
            let d_id = d.id();
            d.on_open(Box::new(move || {
                println!("[Server DC ID:{:?}] opened: {}", d_id, d_clone.label());
                println!("   - Ready to receive messages");
                Box::pin(async {})
            }));
            
            let d_msg_id = d.id();
            d.on_message(Box::new(move |msg| {
                let tx_msg = tx.clone();
                let data = msg.data.to_vec();
                println!("[Server DC ID:{:?}] received message {} bytes", d_msg_id, data.len());
                println!("   Content: {:?}", String::from_utf8_lossy(&data));
                tokio::spawn(async move {
                    if let Err(e) = tx_msg.send(data).await {
                        error!("Failed to forward server DataChannel message: {}", e);
                        println!("[Server DC] Forward failed: {}", e);
                    } else {
                        println!("[Server DC] Message forwarded to bridge handler");
                    }
                });
                Box::pin(async {})
            }));
            
            let d_err_id = d.id();
            d.on_error(Box::new(move |err| {
                println!("[Server DC ID:{:?}] error: {}", d_err_id, err);
                Box::pin(async {})
            }));
            
            let d_close_id = d.id();
            d.on_close(Box::new(move || {
                println!("[Server DC ID:{:?}] closed", d_close_id);
                Box::pin(async {})
            }));
            
            Box::pin(async {})
        }));
        
        let inbound_tx_msg = inbound_tx.clone();
        let inbound_tx_msg = inbound_tx.clone();
        let dc_msg_id = dc.id();
        dc.on_message(Box::new(move |msg| {
            let tx = inbound_tx_msg.clone();
            let data = msg.data.to_vec();
            println!("[Client DC ID:{:?}] received message {} bytes", dc_msg_id, data.len());
            println!("   Content: {:?}", String::from_utf8_lossy(&data));
            tokio::spawn(async move {
                if let Err(e) = tx.send(data).await {
                    error!("Failed to forward DataChannel message: {}", e);
                    println!("Forward failed: {}", e);
                } else {
                    println!("[Client DC] Message forwarded to bridge handler");
                }
            });
            Box::pin(async {})
        }));
        
        // Add transceivers for media (required for WHEP - as subscriber)
        pc.add_transceiver_from_kind(
            webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video,
            Some(webrtc::rtp_transceiver::RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                send_encodings: Vec::new(),
            }),
        ).await?;
        
        pc.add_transceiver_from_kind(
            webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Audio,
            Some(webrtc::rtp_transceiver::RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                send_encodings: Vec::new(),
            }),
        ).await?;
        
        // Create offer and set local description
        let offer = pc.create_offer(None).await?;
        let mut gather_complete = pc.gathering_complete_promise().await;
        pc.set_local_description(offer).await?;
        let _ = gather_complete.recv().await;
        
        let local_desc = pc.local_description().await
            .ok_or_else(|| anyhow!("Failed to get local description"))?;
        
        // Send offer to liveion WHEP endpoint (as subscriber)
        let whep_url = format!("{}/whep/{}", self.config.url, self.config.stream);
        let mut request = self.http_client
            .post(&whep_url)
            .header("Content-Type", "application/sdp")
            .body(local_desc.sdp);
        
        if let Some(token) = &self.auth_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }
        
        let response = request.send().await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("WHEP request failed: {}", response.status()));
        }
        
        let answer_sdp = response.text().await?;
        let answer = RTCSessionDescription::answer(answer_sdp)?;
        pc.set_remote_description(answer).await?;
        
        info!("WebRTC connection established");
        println!("WebRTC connection established");
        
        // Handle outbound messages
        loop {
            tokio::select! {
                Some(data) = outbound_rx.recv() => {
                    if dc_clone.ready_state() == RTCDataChannelState::Open {
                        let data_len = data.len();
                        if let Err(e) = dc_clone.send(&data.into()).await {
                            error!("Failed to send DataChannel message: {}", e);
                            break;
                        } else {
                            debug!("Sent DataChannel message: {} bytes", data_len);
                        }
                    } else {
                        warn!("DataChannel not open, dropping message");
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    // Send keepalive
                    if dc_clone.ready_state() == RTCDataChannelState::Open {
                        let keepalive = json!({
                            "action": "keepalive",
                            "timestamp": chrono::Utc::now().timestamp_millis()
                        }).to_string();
                        
                        if let Err(e) = dc_clone.send(&keepalive.into_bytes().into()).await {
                            error!("Failed to send keepalive: {}", e);
                            break;
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    async fn authenticate(&mut self) -> Result<()> {
        let auth = self.config.auth.as_ref()
            .ok_or_else(|| anyhow!("No auth config provided"))?;
        
        let login_url = format!("{}/api/login", self.config.url);
        let login_data = json!({
            "username": auth.username,
            "password": auth.password
        });
        
        let response = self.http_client
            .post(&login_url)
            .json(&login_data)
            .send()
            .await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Authentication failed: {}", response.status()));
        }
        
        let result: Value = response.json().await?;
        let token = result["token"].as_str()
            .ok_or_else(|| anyhow!("No token in auth response"))?;
        
        self.auth_token = Some(token.to_string());
        info!("Authentication successful");
        
        Ok(())
    }
    
    async fn create_peer_connection(&self) -> Result<Arc<RTCPeerConnection>> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        
        let mut s = SettingEngine::default();
        // Temporarily disable detach_data_channels to test on_message callbacks
        // s.detach_data_channels();
        s.set_ice_multicast_dns_mode(MulticastDnsMode::Disabled);
        
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .with_setting_engine(s)
            .build();
        
        let config = RTCConfiguration {
            ice_servers: vec![
                RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        
        let pc = Arc::new(api.new_peer_connection(config).await?);
        
        Ok(pc)
    }
}