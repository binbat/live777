use super::auth::{AppState, Claims};
use super::config::{CameraSettings, NetworkConfig, NtpConfig, StaticIpConfig};
use super::utils;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use std::process::Command;
use tokio::fs;
use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkConfigRequest {
    pub protocol: String,
    pub static_ip: StaticIpConfig,
    pub ntp: NtpConfig,
    pub camera: CameraSettings,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub name: String,
    pub ip: Option<String>,
    pub netmask: Option<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    pub dhcp_enabled: bool,
    pub status: InterfaceStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum InterfaceStatus {
    Up,
    Down,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub platform_type: String,
    pub arch: String,
    pub os: String,
    pub kernel: String,
    pub hostname: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkInfoResponse {
    pub system: SystemInfo,
    pub interfaces: Vec<NetworkInterface>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DHCPServerConfig {
    pub interface: String,
    pub range_start: String,
    pub range_end: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DHCPControlRequest {
    pub start: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimeSyncRequest {
    pub server: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InterfaceStateRequest {
    pub up: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: Option<T>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            message: "Success".to_string(),
            data: Some(data),
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            message,
            data: None,
        }
    }
}

#[derive(Debug)]
pub enum NetworkManager {
    Systemd(SystemdNetworkManager),
    RiscV(RiscVNetworkManager),
}

impl NetworkManager {
    pub async fn get_system_info(&self) -> Result<SystemInfo, String> {
        match self {
            NetworkManager::Systemd(manager) => manager.get_system_info().await,
            NetworkManager::RiscV(manager) => manager.get_system_info().await,
        }
    }

    pub async fn list_interfaces(&self) -> Result<Vec<NetworkInterface>, String> {
        match self {
            NetworkManager::Systemd(manager) => manager.list_interfaces().await,
            NetworkManager::RiscV(manager) => manager.list_interfaces().await,
        }
    }

    pub async fn configure_interface(&self, interface: &NetworkInterface) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.configure_interface(interface).await,
            NetworkManager::RiscV(manager) => manager.configure_interface(interface).await,
        }
    }

    pub async fn set_interface_state(&self, name: &str, up: bool) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.set_interface_state(name, up).await,
            NetworkManager::RiscV(manager) => manager.set_interface_state(name, up).await,
        }
    }

    pub async fn configure_dhcp(&self, config: &DHCPServerConfig) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.configure_dhcp(config).await,
            NetworkManager::RiscV(manager) => manager.configure_dhcp(config).await,
        }
    }

    pub async fn control_dhcp_service(&self, start: bool) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.control_dhcp_service(start).await,
            NetworkManager::RiscV(manager) => manager.control_dhcp_service(start).await,
        }
    }

    pub async fn configure_ntp(&self, config: &NtpConfig) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.configure_ntp(config).await,
            NetworkManager::RiscV(manager) => manager.configure_ntp(config).await,
        }
    }

    pub async fn sync_time(&self, server: &str) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.sync_time(server).await,
            NetworkManager::RiscV(manager) => manager.sync_time(server).await,
        }
    }

    pub async fn restart_network(&self) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.restart_network().await,
            NetworkManager::RiscV(manager) => manager.restart_network().await,
        }
    }

    pub async fn validate_config(&self, config: &NetworkConfigRequest) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.validate_config(config).await,
            NetworkManager::RiscV(manager) => manager.validate_config(config).await,
        }
    }

    pub async fn apply_network_config(&self, config: &NetworkConfigRequest) -> Result<(), String> {
        match self {
            NetworkManager::Systemd(manager) => manager.apply_network_config(config).await,
            NetworkManager::RiscV(manager) => manager.apply_network_config(config).await,
        }
    }
}

async fn get_kernel_version() -> Result<String, String> {
    let output = Command::new("uname")
        .arg("-r")
        .output()
        .map_err(|e| format!("Failed to get kernel version: {}", e))?;

    if !output.status.success() {
        return Err("Failed to get kernel version".to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn get_hostname() -> Result<String, String> {
    let output = Command::new("hostname")
        .output()
        .map_err(|e| format!("Failed to get hostname: {}", e))?;

    if !output.status.success() {
        return Err("Failed to get hostname".to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn common_validate_config(config: &NetworkConfigRequest) -> Result<(), String> {
    if config.protocol != "rtp" && config.protocol != "rtsp" {
        return Err("Invalid protocol. Must be 'rtp' or 'rtsp'".to_string());
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

async fn parse_interfaces_from_ip_command() -> Result<Vec<String>, String> {
    let output = Command::new("ip")
        .args(["link", "show"])
        .output()
        .map_err(|e| format!("Failed to execute ip command: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "ip link show command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    Ok(parse_ip_link_output(&output_str))
}

fn parse_ip_link_output(output: &str) -> Vec<String> {
    let mut interfaces = Vec::new();

    for line in output.lines() {
        let line = line.trim();

        if let Some(first_colon) = line.find(':') {
            let after_number = &line[first_colon + 1..].trim();
            if let Some(second_colon) = after_number.find(':') {
                let interface_name = after_number[..second_colon].trim();
                if !interface_name.is_empty()
                    && !interface_name.starts_with("veth")
                    && !interface_name.starts_with("docker")
                    && !interface_name.starts_with("br-")
                {
                    interfaces.push(interface_name.to_string());
                }
            }
        }
    }

    debug!("Parsed interfaces from ip link: {:?}", interfaces);
    interfaces
}

async fn get_interface_details(interface_name: &str) -> Result<NetworkInterface, String> {
    debug!("Getting details for interface: {}", interface_name);

    let output = Command::new("ip")
        .args(["addr", "show", interface_name])
        .output()
        .map_err(|e| format!("Failed to execute ip addr command: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "ip addr show {} failed: {}",
            interface_name,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    debug!("ip addr show {} output: {}", interface_name, output_str);

    parse_ip_addr_output(interface_name, &output_str).await
}

async fn parse_ip_addr_output(name: &str, output: &str) -> Result<NetworkInterface, String> {
    let mut interface = NetworkInterface {
        name: name.to_string(),
        ip: None,
        netmask: None,
        gateway: None,
        dns: vec![],
        dhcp_enabled: false,
        status: InterfaceStatus::Unknown,
    };

    for line in output.lines() {
        let line = line.trim();

        if line.contains(&format!("{}: <", name)) || line.contains(&format!("{}@", name)) {
            if line.contains("UP") && line.contains("LOWER_UP") {
                interface.status = InterfaceStatus::Up;
            } else if line.contains("DOWN") {
                interface.status = InterfaceStatus::Down;
            }
        }

        if line.contains("state UP") {
            interface.status = InterfaceStatus::Up;
        } else if line.contains("state DOWN") {
            interface.status = InterfaceStatus::Down;
        }

        if line.starts_with("inet ") && !line.contains("127.0.0.1") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let ip_with_prefix = parts[1];
                if let Some(slash_pos) = ip_with_prefix.find('/') {
                    interface.ip = Some(ip_with_prefix[..slash_pos].to_string());

                    if let Ok(prefix_len) = ip_with_prefix[slash_pos + 1..].parse::<u8>() {
                        interface.netmask = Some(cidr_to_netmask(prefix_len));
                    }
                } else {
                    interface.ip = Some(ip_with_prefix.to_string());
                }

                if line.contains("dynamic") {
                    interface.dhcp_enabled = true;
                }
            }
        }
    }

    if interface.ip.is_some() {
        interface.gateway = get_default_gateway_for_interface(name).await;
    }

    interface.dns = get_dns_servers().await;

    debug!(
        "Parsed interface {}: IP={:?}, Status={:?}",
        name, interface.ip, interface.status
    );
    Ok(interface)
}

fn cidr_to_netmask(prefix_len: u8) -> String {
    if prefix_len > 32 {
        return "255.255.255.255".to_string();
    }

    let mask = if prefix_len == 0 {
        0u32
    } else {
        (!0u32) << (32 - prefix_len)
    };

    format!(
        "{}.{}.{}.{}",
        (mask >> 24) & 0xFF,
        (mask >> 16) & 0xFF,
        (mask >> 8) & 0xFF,
        mask & 0xFF
    )
}

async fn get_default_gateway_for_interface(interface_name: &str) -> Option<String> {
    let output = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    for line in output_str.lines() {
        if line.contains("default via") && line.contains(&format!("dev {}", interface_name)) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            for i in 0..parts.len() {
                if parts[i] == "via" && i + 1 < parts.len() {
                    return Some(parts[i + 1].to_string());
                }
            }
        }
    }

    None
}

async fn get_dns_servers() -> Vec<String> {
    let mut dns_servers = Vec::new();

    if let Ok(content) = fs::read_to_string("/etc/resolv.conf").await {
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("nameserver ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    dns_servers.push(parts[1].to_string());
                }
            }
        }
    }

    dns_servers
}

#[derive(Debug)]
pub struct SystemdNetworkManager;

impl SystemdNetworkManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn get_system_info(&self) -> Result<SystemInfo, String> {
        let arch = std::env::consts::ARCH.to_string();
        let os = "linux".to_string();
        let kernel = get_kernel_version().await?;
        let hostname = get_hostname().await?;

        Ok(SystemInfo {
            platform_type: "Systemd".to_string(),
            arch,
            os,
            kernel,
            hostname,
        })
    }

    pub async fn list_interfaces(&self) -> Result<Vec<NetworkInterface>, String> {
        let mut interfaces = Vec::new();

        info!("Starting to list network interfaces using systemd");

        let interface_names = parse_interfaces_from_ip_command().await?;
        info!("Found interfaces from ip command: {:?}", interface_names);

        for interface_name in interface_names {
            match get_interface_details(&interface_name).await {
                Ok(interface) => {
                    info!("Successfully got details for interface: {}", interface_name);
                    interfaces.push(interface);
                }
                Err(e) => {
                    warn!(
                        "Failed to get details for interface {}: {}",
                        interface_name, e
                    );
                }
            }
        }

        info!("Total interfaces found: {}", interfaces.len());
        Ok(interfaces)
    }

    pub async fn configure_interface(&self, interface: &NetworkInterface) -> Result<(), String> {
        info!("Configuring interface: {} using systemd", interface.name);

        match interface.name.as_str() {
            name if name.starts_with("eth") || name.starts_with("enp") => {
                self.configure_ethernet_systemd(interface).await
            }
            name if name.starts_with("wlan") => self.configure_wifi_systemd(interface).await,
            _ => Err(format!(
                "Unsupported interface for systemd: {}",
                interface.name
            )),
        }
    }

    pub async fn set_interface_state(&self, interface_name: &str, up: bool) -> Result<(), String> {
        let state = if up { "up" } else { "down" };

        let output = Command::new("ip")
            .args(["link", "set", interface_name, state])
            .output()
            .map_err(|e| format!("Failed to set interface state: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to set interface {} {}",
                interface_name, state
            ));
        }

        info!("Interface {} set to {} via systemd", interface_name, state);
        Ok(())
    }

    pub async fn configure_dhcp(&self, dhcp_config: &DHCPServerConfig) -> Result<(), String> {
        info!(
            "Configuring DHCP server for interface {} using systemd",
            dhcp_config.interface
        );

        let dnsmasq_conf = format!(
            r#"interface={}
dhcp-range={},{},1h
dhcp-option=3
dhcp-option=6,8.8.8.8,8.8.4.4
"#,
            dhcp_config.interface, dhcp_config.range_start, dhcp_config.range_end
        );

        fs::write("/etc/dnsmasq.conf", dnsmasq_conf)
            .await
            .map_err(|e| format!("Failed to write dnsmasq config: {}", e))?;

        info!("DHCP server configuration written for systemd");
        Ok(())
    }

    pub async fn control_dhcp_service(&self, start: bool) -> Result<(), String> {
        let action = if start { "start" } else { "stop" };

        let output = Command::new("systemctl")
            .args([action, "dnsmasq"])
            .output()
            .map_err(|e| format!("Failed to {} dnsmasq: {}", action, e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to {} dnsmasq: {}",
                action,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        info!(
            "dnsmasq {} via systemctl",
            if start { "started" } else { "stopped" }
        );
        Ok(())
    }

    pub async fn configure_ntp(&self, config: &NtpConfig) -> Result<(), String> {
        info!(
            "Configuring NTP: server={}, timezone={} using systemd",
            config.server, config.timezone
        );

        let output = Command::new("timedatectl")
            .args(["set-ntp", "true"])
            .output()
            .map_err(|e| format!("Failed to enable NTP: {}", e))?;

        if !output.status.success() {
            return Err("Failed to enable NTP".to_string());
        }

        let timesyncd_conf = format!("[Time]\nNTP={}\nFallbackNTP=pool.ntp.org\n", config.server);

        fs::write("/etc/systemd/timesyncd.conf", timesyncd_conf)
            .await
            .map_err(|e| format!("Failed to write timesyncd config: {}", e))?;

        let output = Command::new("timedatectl")
            .args(["set-timezone", &config.timezone])
            .output()
            .map_err(|e| format!("Failed to set timezone: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to set timezone: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Command::new("systemctl")
            .args(["restart", "systemd-timesyncd"])
            .output()
            .map_err(|e| format!("Failed to restart timesyncd: {}", e))?;

        info!("NTP configured successfully using systemd");
        Ok(())
    }

    pub async fn sync_time(&self, server: &str) -> Result<(), String> {
        info!("Syncing time with server: {} using systemd", server);

        let output = Command::new("systemctl")
            .args(["restart", "systemd-timesyncd"])
            .output()
            .map_err(|e| format!("Failed to restart timesyncd: {}", e))?;

        if !output.status.success() {
            return Err("Failed to restart timesyncd".to_string());
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        info!("Time synchronized successfully using systemd");
        Ok(())
    }

    pub async fn restart_network(&self) -> Result<(), String> {
        info!("Restarting network service using systemd");

        let output = Command::new("systemctl")
            .args(["restart", "systemd-networkd"])
            .output()
            .map_err(|e| format!("Failed to restart networkd: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to restart networkd: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        info!("Network service restarted successfully using systemd");
        Ok(())
    }

    pub async fn validate_config(&self, config: &NetworkConfigRequest) -> Result<(), String> {
        if config.static_ip.enabled && config.static_ip.ip.is_empty() {
            return Err("IP address is required when static IP is enabled".to_string());
        }

        common_validate_config(config)
    }

    pub async fn apply_network_config(&self, config: &NetworkConfigRequest) -> Result<(), String> {
        info!("Applying network configuration using systemd");

        if config.static_ip.enabled {
            info!("Configuring static IP: {}", config.static_ip.ip);
        }

        if config.ntp.enabled {
            self.configure_ntp(&config.ntp).await?;
        }

        info!("Network configuration applied successfully using systemd");
        Ok(())
    }

    async fn configure_ethernet_systemd(&self, interface: &NetworkInterface) -> Result<(), String> {
        if interface.dhcp_enabled {
            let network_config = format!(
                r#"[Match]
Name={}

[Network]
DHCP=ipv4
"#,
                interface.name
            );

            let config_path = format!("/etc/systemd/network/{}.network", interface.name);
            fs::write(&config_path, network_config)
                .await
                .map_err(|e| format!("Failed to write network config: {}", e))?;
        } else {
            self.configure_static_ip_networkd(interface).await?;
        }

        Command::new("systemctl")
            .args(["restart", "systemd-networkd"])
            .output()
            .map_err(|e| format!("Failed to restart networkd: {}", e))?;

        info!(
            "Ethernet interface {} configured successfully using systemd",
            interface.name
        );
        Ok(())
    }

    async fn configure_static_ip_networkd(
        &self,
        interface: &NetworkInterface,
    ) -> Result<(), String> {
        let network_config = format!(
            r#"[Match]
Name={}

[Network]
Address={}/24
Gateway={}
DNS={}
"#,
            interface.name,
            interface
                .ip
                .as_ref()
                .unwrap_or(&"192.168.1.100".to_string()),
            interface
                .gateway
                .as_ref()
                .unwrap_or(&"192.168.1.1".to_string()),
            interface.dns.join(" ")
        );

        let config_path = format!("/etc/systemd/network/{}.network", interface.name);
        fs::write(&config_path, network_config)
            .await
            .map_err(|e| format!("Failed to write network config: {}", e))?;

        Ok(())
    }

    async fn configure_wifi_systemd(&self, _interface: &NetworkInterface) -> Result<(), String> {
        warn!("WiFi configuration using systemd not fully implemented");
        Ok(())
    }
}

impl Default for SystemdNetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct RiscVNetworkManager;

impl RiscVNetworkManager {
    pub fn new() -> Self {
        Self
    }

    pub async fn get_system_info(&self) -> Result<SystemInfo, String> {
        let arch = std::env::consts::ARCH.to_string();
        let os = "linux".to_string();
        let kernel = get_kernel_version().await?;
        let hostname = get_hostname().await?;

        Ok(SystemInfo {
            platform_type: "RISC-V Script".to_string(),
            arch,
            os,
            kernel,
            hostname,
        })
    }

    pub async fn list_interfaces(&self) -> Result<Vec<NetworkInterface>, String> {
        let mut interfaces = Vec::new();

        info!("Starting to list network interfaces using RISC-V scripts");

        let interface_names = parse_interfaces_from_ip_command().await?;
        info!("Found interfaces from ip command: {:?}", interface_names);

        for interface_name in interface_names {
            match get_interface_details(&interface_name).await {
                Ok(interface) => {
                    info!("Successfully got details for interface: {}", interface_name);
                    interfaces.push(interface);
                }
                Err(e) => {
                    warn!(
                        "Failed to get details for interface {}: {}",
                        interface_name, e
                    );
                }
            }
        }

        info!("Total interfaces found: {}", interfaces.len());
        Ok(interfaces)
    }

    pub async fn configure_interface(&self, interface: &NetworkInterface) -> Result<(), String> {
        info!(
            "Configuring interface: {} using RISC-V scripts",
            interface.name
        );

        match interface.name.as_str() {
            "usb0" => self.configure_usb_ncm(interface).await,
            "eth0" => self.configure_ethernet_script(interface).await,
            "wlan0" => self.configure_wifi_script(interface).await,
            _ => Err(format!(
                "Unsupported interface for RISC-V: {}",
                interface.name
            )),
        }
    }

    pub async fn set_interface_state(&self, interface_name: &str, up: bool) -> Result<(), String> {
        let state = if up { "up" } else { "down" };

        let output = Command::new("ifconfig")
            .args([interface_name, state])
            .output()
            .map_err(|e| format!("Failed to set interface state: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to set interface {} {}",
                interface_name, state
            ));
        }

        info!("Interface {} set to {} via ifconfig", interface_name, state);
        Ok(())
    }

    pub async fn configure_dhcp(&self, dhcp_config: &DHCPServerConfig) -> Result<(), String> {
        info!(
            "Configuring DHCP server for interface {} using RISC-V scripts",
            dhcp_config.interface
        );

        let dnsmasq_conf = format!(
            r#"interface={}
dhcp-range={},{},1h
dhcp-option=3
dhcp-option=6,8.8.8.8
"#,
            dhcp_config.interface, dhcp_config.range_start, dhcp_config.range_end
        );

        fs::write("/etc/dnsmasq.conf", dnsmasq_conf)
            .await
            .map_err(|e| format!("Failed to write dnsmasq config: {}", e))?;

        info!("DHCP server configuration written for RISC-V");
        Ok(())
    }

    pub async fn control_dhcp_service(&self, start: bool) -> Result<(), String> {
        let action = if start { "start" } else { "stop" };

        let output = Command::new("/etc/init.d/S80dnsmasq")
            .arg(action)
            .output()
            .map_err(|e| format!("Failed to {} dnsmasq: {}", action, e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to {} dnsmasq: {}",
                action,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        info!(
            "dnsmasq {} via init script",
            if start { "started" } else { "stopped" }
        );
        Ok(())
    }

    pub async fn configure_ntp(&self, config: &NtpConfig) -> Result<(), String> {
        info!(
            "Configuring NTP: server={}, timezone={} using RISC-V scripts",
            config.server, config.timezone
        );

        self.sync_time(&config.server).await?;

        let output = Command::new("ln")
            .args([
                "-sf",
                &format!("/usr/share/zoneinfo/{}", config.timezone),
                "/etc/localtime",
            ])
            .output()
            .map_err(|e| format!("Failed to set timezone: {}", e))?;

        if !output.status.success() {
            return Err("Failed to set timezone".to_string());
        }

        info!("NTP configured successfully using RISC-V scripts");
        Ok(())
    }

    pub async fn sync_time(&self, server: &str) -> Result<(), String> {
        info!("Syncing time with server: {} using RISC-V scripts", server);

        let output = Command::new("ntpdate")
            .args(["-s", server])
            .output()
            .map_err(|e| format!("Failed to run ntpdate: {}", e))?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to sync time: {}", error));
        }

        let output = Command::new("hwclock")
            .args(["-w"])
            .output()
            .map_err(|e| format!("Failed to sync hardware clock: {}", e))?;

        if !output.status.success() {
            warn!(
                "Failed to sync hardware clock: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        info!("Time synchronized successfully using RISC-V scripts");
        Ok(())
    }

    pub async fn restart_network(&self) -> Result<(), String> {
        info!("Restarting network service using RISC-V scripts");

        let output = Command::new("ifconfig")
            .args(["usb0", "down"])
            .output()
            .map_err(|e| format!("Failed to bring down usb0: {}", e))?;

        if !output.status.success() {
            warn!(
                "Failed to bring down usb0: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let output = Command::new("ifconfig")
            .args(["usb0", "up"])
            .output()
            .map_err(|e| format!("Failed to bring up usb0: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to bring up usb0: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        info!("Network service restarted successfully using RISC-V scripts");
        Ok(())
    }

    pub async fn validate_config(&self, config: &NetworkConfigRequest) -> Result<(), String> {
        if config.static_ip.enabled {
            if config.static_ip.ip.is_empty() {
                return Err("IP address is required when static IP is enabled".to_string());
            }

            if !config.static_ip.ip.starts_with("192.168.42.") {
                return Err("RISC-V device requires IP in 192.168.42.x range".to_string());
            }

            let ip_parts: Vec<&str> = config.static_ip.ip.split('.').collect();
            if ip_parts.len() == 4
                && let Ok(last_octet) = ip_parts[3].parse::<u8>()
                && !(2..=242).contains(&last_octet)
            {
                return Err(
                    "IP address last octet should be between 2-242 for DHCP compatibility"
                        .to_string(),
                );
            }
        }

        common_validate_config(config)
    }

    pub async fn apply_network_config(&self, config: &NetworkConfigRequest) -> Result<(), String> {
        info!("Applying network configuration on RISC-V device");

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
                .await
                .map_err(|e| format!("Failed to write USB NCM script: {}", e))?;

            let output = Command::new("chmod")
                .args(["+x", "/mnt/system/usb-ncm.sh"])
                .output()
                .map_err(|e| format!("Failed to set script permissions: {}", e))?;

            if !output.status.success() {
                return Err(format!(
                    "Failed to set script permissions: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
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
                .await
                .map_err(|e| format!("Failed to write dnsmasq config: {}", e))?;

            let output = Command::new("ifconfig")
                .args(["usb0", &config.static_ip.ip])
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
                return Err(format!(
                    "Failed to restart dnsmasq: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }

            info!("USB network configuration applied successfully");
        }

        if config.ntp.enabled {
            self.configure_ntp(&config.ntp).await?;
        }

        info!("Camera configuration will be applied on next stream start");
        info!(
            "Resolution: {}, FPS: {}, Bitrate: {}kbps",
            config.camera.resolution, config.camera.fps, config.camera.bitrate
        );

        Ok(())
    }

    async fn configure_usb_ncm(&self, interface: &NetworkInterface) -> Result<(), String> {
        info!("Configuring USB NCM interface for RISC-V device");

        let usb_ncm_script = format!(
            r#"#!/bin/sh
/etc/uhubon.sh device >> /tmp/ncm.log 2>&1
/etc/run_usb.sh probe ncm >> /tmp/ncm.log 2>&1
/etc/run_usb.sh start ncm >> /tmp/ncm.log 2>&1
sleep 0.5
ifconfig usb0 {}
"#,
            interface.ip.as_ref().unwrap_or(&"192.168.42.1".to_string())
        );

        fs::write("/mnt/system/usb-ncm.sh", usb_ncm_script)
            .await
            .map_err(|e| format!("Failed to write USB NCM script: {}", e))?;

        let _ = Command::new("chmod")
            .args(["+x", "/mnt/system/usb-ncm.sh"])
            .output();

        if let Some(ip) = &interface.ip {
            let output = Command::new("ifconfig")
                .args(["usb0", ip])
                .output()
                .map_err(|e| format!("Failed to set USB IP: {}", e))?;

            if !output.status.success() {
                return Err("Failed to configure USB interface".to_string());
            }
        }

        info!("USB interface configured successfully");
        Ok(())
    }

    async fn configure_ethernet_script(&self, interface: &NetworkInterface) -> Result<(), String> {
        if interface.dhcp_enabled {
            let output = Command::new("dhclient")
                .arg("eth0")
                .output()
                .map_err(|e| format!("Failed to start DHCP client: {}", e))?;

            if !output.status.success() {
                warn!("DHCP client may have failed, but continuing");
            }
        } else if let Some(ip) = &interface.ip {
            let output = Command::new("ifconfig")
                .args(["eth0", ip])
                .output()
                .map_err(|e| format!("Failed to set ethernet IP: {}", e))?;

            if !output.status.success() {
                return Err("Failed to configure ethernet interface".to_string());
            }

            if let Some(gateway) = &interface.gateway {
                let _ = Command::new("route")
                    .args(["add", "default", "gw", gateway])
                    .output();
            }
        }

        info!("Ethernet interface configured successfully");
        Ok(())
    }

    async fn configure_wifi_script(&self, _interface: &NetworkInterface) -> Result<(), String> {
        warn!("WiFi configuration not fully implemented for RISC-V");
        Ok(())
    }
}

impl Default for RiscVNetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn create_network_manager() -> NetworkManager {
    if is_systemd_available() {
        info!("Detected systemd environment, using SystemdNetworkManager");
        NetworkManager::Systemd(SystemdNetworkManager::new())
    } else {
        info!("Detected RISC-V/script environment, using RiscVNetworkManager");
        NetworkManager::RiscV(RiscVNetworkManager::new())
    }
}

fn is_systemd_available() -> bool {
    Command::new("systemctl")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn create_network_router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/api/network/config", get(get_config))
        .route("/api/network/config", post(set_config))
        .route("/api/network/info", get(get_network_info))
        .route("/api/network/interfaces", get(list_interfaces))
        .route("/api/network/interfaces/:name", put(configure_interface))
        .route(
            "/api/network/interfaces/:name/state",
            post(set_interface_state),
        )
        .route("/api/network/dhcp", post(configure_dhcp_server))
        .route("/api/network/dhcp/control", post(control_dhcp_service))
        .route("/api/network/ntp", post(configure_ntp))
        .route("/api/network/ntp/sync", post(sync_time))
        .route("/api/network/restart", post(restart_network_service))
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

    let manager = create_network_manager();

    if let Err(e) = manager.validate_config(&payload).await {
        warn!("Invalid network configuration: {}", e);
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    if let Err(e) = manager.apply_network_config(&payload).await {
        error!("Failed to apply network configuration: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to apply configuration: {}", e),
        )
            .into_response();
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

async fn get_network_info(State(_state): State<AppState>, _claims: Claims) -> impl IntoResponse {
    info!("Getting network information");

    let manager = create_network_manager();

    match manager.get_system_info().await {
        Ok(system) => match manager.list_interfaces().await {
            Ok(interfaces) => {
                let response = NetworkInfoResponse { system, interfaces };
                Json(ApiResponse::success(response))
            }
            Err(e) => {
                error!("Failed to list interfaces: {}", e);
                Json(ApiResponse::error(format!(
                    "Failed to list interfaces: {}",
                    e
                )))
            }
        },
        Err(e) => {
            error!("Failed to get system info: {}", e);
            Json(ApiResponse::error(format!(
                "Failed to get system info: {}",
                e
            )))
        }
    }
}

async fn list_interfaces(State(_state): State<AppState>, _claims: Claims) -> impl IntoResponse {
    info!("Listing network interfaces");

    let manager = create_network_manager();

    match manager.list_interfaces().await {
        Ok(interfaces) => Json(ApiResponse::success(interfaces)),
        Err(e) => {
            error!("Failed to list interfaces: {}", e);
            Json(ApiResponse::error(format!(
                "Failed to list interfaces: {}",
                e
            )))
        }
    }
}

async fn configure_interface(
    State(_state): State<AppState>,
    _claims: Claims,
    Path(interface_name): Path<String>,
    Json(mut interface_config): Json<NetworkInterface>,
) -> impl IntoResponse {
    info!("Configuring interface: {}", interface_name);

    let manager = create_network_manager();
    interface_config.name = interface_name.clone();

    match manager.configure_interface(&interface_config).await {
        Ok(_) => {
            info!("Interface {} configured successfully", interface_name);
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            error!("Failed to configure interface {}: {}", interface_name, e);
            Json(ApiResponse::error(e))
        }
    }
}

async fn set_interface_state(
    State(_state): State<AppState>,
    _claims: Claims,
    Path(interface_name): Path<String>,
    Json(state_request): Json<InterfaceStateRequest>,
) -> impl IntoResponse {
    info!(
        "Setting interface {} state to {}",
        interface_name,
        if state_request.up { "up" } else { "down" }
    );

    let manager = create_network_manager();

    match manager
        .set_interface_state(&interface_name, state_request.up)
        .await
    {
        Ok(_) => {
            info!("Interface {} state changed successfully", interface_name);
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            error!("Failed to set interface {} state: {}", interface_name, e);
            Json(ApiResponse::error(e))
        }
    }
}

async fn configure_dhcp_server(
    State(_state): State<AppState>,
    _claims: Claims,
    Json(dhcp_config): Json<DHCPServerConfig>,
) -> impl IntoResponse {
    info!(
        "Configuring DHCP server for interface: {}",
        dhcp_config.interface
    );

    let manager = create_network_manager();

    match manager.configure_dhcp(&dhcp_config).await {
        Ok(_) => {
            info!("DHCP server configured successfully");
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            error!("Failed to configure DHCP server: {}", e);
            Json(ApiResponse::error(e))
        }
    }
}

async fn control_dhcp_service(
    State(_state): State<AppState>,
    _claims: Claims,
    Json(control_request): Json<DHCPControlRequest>,
) -> impl IntoResponse {
    info!(
        "Controlling DHCP service: {}",
        if control_request.start {
            "start"
        } else {
            "stop"
        }
    );

    let manager = create_network_manager();

    match manager.control_dhcp_service(control_request.start).await {
        Ok(_) => {
            info!("DHCP service controlled successfully");
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            error!("Failed to control DHCP service: {}", e);
            Json(ApiResponse::error(e))
        }
    }
}

async fn configure_ntp(
    State(_state): State<AppState>,
    _claims: Claims,
    Json(ntp_config): Json<NtpConfig>,
) -> impl IntoResponse {
    info!(
        "Configuring NTP: server={}, timezone={}",
        ntp_config.server, ntp_config.timezone
    );

    let manager = create_network_manager();

    match manager.configure_ntp(&ntp_config).await {
        Ok(_) => {
            info!("NTP configured successfully");
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            error!("Failed to configure NTP: {}", e);
            Json(ApiResponse::error(e))
        }
    }
}

async fn sync_time(
    State(_state): State<AppState>,
    _claims: Claims,
    Json(sync_request): Json<TimeSyncRequest>,
) -> impl IntoResponse {
    info!("Syncing time with server: {}", sync_request.server);

    let manager = create_network_manager();

    match manager.sync_time(&sync_request.server).await {
        Ok(_) => {
            info!("Time synchronized successfully");
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            error!("Failed to sync time: {}", e);
            Json(ApiResponse::error(e))
        }
    }
}

async fn restart_network_service(
    State(_state): State<AppState>,
    _claims: Claims,
) -> impl IntoResponse {
    info!("Restarting network service");

    let manager = create_network_manager();

    match manager.restart_network().await {
        Ok(_) => {
            info!("Network service restarted successfully");
            Json(ApiResponse::success(()))
        }
        Err(e) => {
            error!("Failed to restart network service: {}", e);
            Json(ApiResponse::error(e))
        }
    }
}
