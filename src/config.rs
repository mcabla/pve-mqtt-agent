//! TOML configuration loader with validation.

use std::{fs, path::Path};

use serde::Deserialize;
use thiserror::Error;


#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct RootConfig {
    pub agent: Agent,
    pub mqtt: Mqtt,
    pub proxmox: Proxmox,
}

fn default_action_timeout_secs() -> u64 { 180 }
fn default_reboot_detect_window_secs() -> u64 { 120 }
fn default_reboot_uptime_drop_secs() -> u64 { 10 }

#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub poll_interval_secs: u64,
    pub publish_discovery: bool,
    /// Maximum time we consider a power action "in progress" before timing out.
    #[serde(default = "default_action_timeout_secs")]
    pub action_timeout_secs: u64,
    /// If uptime is below this value after a reboot request, we consider the reboot observed.
    #[serde(default = "default_reboot_detect_window_secs")]
    pub reboot_detect_window_secs: u64,
    /// If uptime drops by more than this between polls, we consider a reboot observed.
    #[serde(default = "default_reboot_uptime_drop_secs")]
    pub reboot_uptime_drop_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mqtt {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub username: String,
    pub password: String,
    /// Enable TLS for MQTT (MQTTS). When false, connect over plain TCP.
    pub use_tls: bool,
    /// CA certificate (PEM) to validate the broker certificate (required when `use_tls = true`).
    pub ca_file: Option<String>,
    /// Optional client certificate (PEM) for mutual TLS.
    pub client_cert_file: Option<String>,
    /// Optional client private key (PEM) for mutual TLS.
    pub client_key_file: Option<String>,
    pub topic_prefix: String,
    pub discovery_prefix: String,
    pub availability_topic: String,
    pub alert_topic: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Proxmox {
    pub base_url: String,
    pub token_id: String,
    pub token_secret: String,
    pub pool: String,
    pub timeout_secs: u64,
    pub ca_file: Option<String>,
}

impl RootConfig {
    /// Load config and perform defensive validation.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let txt = fs::read_to_string(path)?;
        let cfg: RootConfig = toml::from_str(&txt)?;

        if cfg.agent.poll_interval_secs < 5 {
            return Err(ConfigError::Invalid("poll_interval_secs too small (<5)".into()));
        }
        if cfg.mqtt.port == 0 {
            return Err(ConfigError::Invalid("mqtt.port must be non-zero".into()));
        }

        // MQTT TLS validation is conditional.
        if cfg.mqtt.use_tls {
            let ca_ok = cfg
                .mqtt
                .ca_file
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
           if !ca_ok {
                return Err(ConfigError::Invalid(
                    "mqtt.use_tls=true requires mqtt.ca_file (PEM)".into(),
                ));
            }

            let cert_set = cfg
                .mqtt
                .client_cert_file
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let key_set = cfg
                .mqtt
                .client_key_file
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);

            // XOR: if only one is set, error.
            if cert_set ^ key_set {
                return Err(ConfigError::Invalid(
                    "mqtt client TLS auth requires both mqtt.client_cert_file and mqtt.client_key_file".into(),
                ));
            }
        }

	let b = cfg.proxmox.base_url.as_str();
        if !(b.starts_with("https://") || b.starts_with("http://")) {
            return Err(ConfigError::Invalid("proxmox.base_url must start with https:// or http://".into()));
        }

        Ok(cfg)
    }
}
