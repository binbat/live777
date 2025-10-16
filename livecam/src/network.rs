use super::auth::{AppState, Claims};
use super::config::{CameraSettings, NetworkConfig, NtpConfig, StaticIpConfig};
use super::utils;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkConfigRequest {
    pub protocol: String,
    pub static_ip: StaticIpConfig,
    pub ntp: NtpConfig,
    pub camera: CameraSettings,
}

pub fn create_network_router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/api/network/config", get(get_config))
        .route("/api/network/config", post(set_config))
}

async fn get_config(State(state): State<AppState>, _claims: Claims) -> impl IntoResponse {
    let config = state.config.read().unwrap();
    let network_config = &config.network;

    info!("Retrieved network configuration");
    Json(network_config.clone())
}

async fn set_config(
    State(state): State<AppState>,
    _claims: Claims,
    Json(payload): Json<NetworkConfigRequest>,
) -> impl IntoResponse {
    debug!("Received network config update request");

    if let Err(e) = validate_config(&payload) {
        warn!("Invalid network configuration: {}", e);
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    #[cfg(riscv_mode)]
    {
        if let Err(e) = apply_network_config(&payload).await {
            error!("Failed to apply network configuration: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to apply configuration: {}", e),
            )
                .into_response();
        }
    }

    {
        let mut config_guard = state.config.write().unwrap();
        config_guard.network = NetworkConfig {
            protocol: payload.protocol,
            static_ip: payload.static_ip,
            ntp: payload.ntp,
            camera: payload.camera,
        };

        if let Err(e) = utils::save_config("livecam", &*config_guard) {
            error!("Failed to save updated config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to save configuration",
            )
                .into_response();
        }
    }

    info!("Network configuration updated successfully");
    (StatusCode::OK, "Network configuration updated successfully").into_response()
}

fn validate_config(config: &NetworkConfigRequest) -> Result<(), String> {
    if config.protocol != "rtp" && config.protocol != "rtsp" {
        return Err("Invalid protocol. Must be 'rtp' or 'rtsp'".to_string());
    }

    if config.static_ip.enabled {
        if config.static_ip.ip.is_empty() {
            return Err("IP address is required when static IP is enabled".to_string());
        }

        if !config.static_ip.ip.starts_with("192.168.42.") {
            return Err("IP should be in 192.168.42.x range".to_string());
        }

        let ip_parts: Vec<&str> = config.static_ip.ip.split('.').collect();
        if ip_parts.len() == 4
            && let Ok(last_octet) = ip_parts[3].parse::<u8>()
            && !(2..=242).contains(&last_octet)
        {
            return Err(
                "IP address last octet should be between 2-242 for DHCP compatibility".to_string(),
            );
        }
    }

    if config.ntp.enabled && config.ntp.server.is_empty() {
        return Err("NTP server is required when NTP is enabled".to_string());
    }

    if config.camera.fps == 0 || config.camera.fps > 60 {
        return Err("FPS must be between 1 and 60".to_string());
    }
    if config.camera.bitrate < 100 || config.camera.bitrate > 10000 {
        return Err("Bitrate must be between 100 and 10000 kbps".to_string());
    }

    Ok(())
}

#[cfg(riscv_mode)]
async fn apply_network_config(config: &NetworkConfigRequest) -> Result<(), String> {
    use std::fs;
    use std::process::Command;

    info!("Applying network configuration on Milk-V Duo device");

    if config.static_ip.enabled {
        info!("Configuring USB network IP: {}", config.static_ip.ip);

        let usb_ncm_script = format!(
            r#"#!/bin/sh

/etc/uhubon.sh device >> /tmp/ncm.log 2>&1
/etc/run_usb.sh probe ncm >> /tmp/ncm.log 2>&1
if test -e /usr/bin/burnd; then
  /etc/run_usb.sh probe acm >> /tmp/ncm.log 2>&1
fi
/etc/run_usb.sh start ncm >> /tmp/ncm.log 2>&1

sleep 0.5
ifconfig usb0 {}

count=`ps | grep dnsmasq | grep -v grep | wc -l`
if [ ${{count}} -lt 1 ] ;then
  echo "/etc/init.d/S80dnsmasq start" >> /tmp/ncm.log 2>&1
  /etc/init.d/S80dnsmasq start >> /tmp/ncm.log 2>&1
fi
"#,
            config.static_ip.ip
        );

        fs::write("/mnt/system/usb-ncm.sh", usb_ncm_script)
            .map_err(|e| format!("Failed to write USB NCM script: {}", e))?;

        let output = Command::new("chmod")
            .args(&["+x", "/mnt/system/usb-ncm.sh"])
            .output()
            .map_err(|e| format!("Failed to set script permissions: {}", e))?;

        if !output.status.success() {
            warn!(
                "Failed to set script permissions: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let ip_parts: Vec<&str> = config.static_ip.ip.split('.').collect();
        let network_base = format!("{}.{}.{}", ip_parts[0], ip_parts[1], ip_parts[2]);

        let dnsmasq_conf = format!(
            r#"interface=usb0
dhcp-range={}.2,{}.242,1h
dhcp-option=3
dhcp-option=6
"#,
            network_base, network_base
        );

        fs::write("/etc/dnsmasq.conf", dnsmasq_conf)
            .map_err(|e| format!("Failed to write dnsmasq config: {}", e))?;

        let output = Command::new("ifconfig")
            .args(&["usb0", &config.static_ip.ip])
            .output()
            .map_err(|e| format!("Failed to set USB IP address: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to set USB IP address: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let output = Command::new("/etc/init.d/S80dnsmasq")
            .arg("restart")
            .output()
            .map_err(|e| format!("Failed to restart dnsmasq: {}", e))?;

        if !output.status.success() {
            warn!(
                "Failed to restart dnsmasq: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        info!("USB network configuration applied successfully");
    }

    if config.ntp.enabled {
        info!("Configuring NTP server: {}", config.ntp.server);

        let output = Command::new("ntpdate")
            .args(&["-s", &config.ntp.server])
            .output()
            .map_err(|e| format!("Failed to sync time with NTP server: {}", e))?;

        if !output.status.success() {
            let error_msg = String::from_utf8_lossy(&output.stderr);
            warn!("NTP sync warning: {}", error_msg);
        } else {
            info!("Time synchronized with NTP server successfully");
        }

        let output = Command::new("hwclock").args(&["-w"]).output();

        match output {
            Ok(output) => {
                if output.status.success() {
                    info!("Hardware clock synchronized");
                } else {
                    warn!(
                        "Failed to sync hardware clock: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
            Err(e) => {
                debug!("hwclock not available: {}", e);
            }
        }

        if let Ok(output) = Command::new("date")
            .args(&["-s", &format!("TZ={}", config.ntp.timezone)])
            .output()
        {
            if !output.status.success() {
                warn!(
                    "Failed to set timezone: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }

    info!("Camera configuration will be applied on next stream start");
    info!(
        "Resolution: {}, FPS: {}, Bitrate: {}kbps",
        config.camera.resolution, config.camera.fps, config.camera.bitrate
    );

    Ok(())
}
