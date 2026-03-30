//! pve-mqtt-agent
//!
//! Secure control plane:
//! - HA dashboard publishes MQTT commands.
//! - This agent validates + executes via Proxmox API token restricted to a pool.
//! - Publishes metrics + discovery so HA can render sensors/buttons without HACS.

mod commands;
mod config;
mod discovery;
mod ha_discovery;
mod metrics;
mod mqtt;
mod proxmox;
mod publisher;
mod state;
mod topic;

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::{signal, time};
use tokio::time::MissedTickBehavior;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::{
    config::RootConfig,
    proxmox::{index_members, ProxmoxClient, ProxmoxConfig},
    publisher as pub_module,
    state::StateManager,
};

type GuestMap = std::collections::HashMap<(topic::GuestType, std::num::NonZeroU32), proxmox::GuestRef>;
type AllowList = Arc<tokio::sync::RwLock<GuestMap>>;
const DEFAULT_CONFIG_PATH: &str = "/etc/pve-mqtt-agent/config.toml";
const REFRESH_ALERT_COOLDOWN_SECS: u64 = 300;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cfg = load_config()?;
    let pve = create_proxmox_client(&cfg)?;
    let (client, mut ev) = mqtt::connect_mqtt(&cfg).await?;

    // Publish online status
    pub_module::publish_availability(&client, &cfg.mqtt.availability_topic, true)
        .await
        .context("failed to publish online status")?;

    // Subscribe to topics
    mqtt::subscribe_topics(&client, &cfg).await?;

    // Shared state
    let allow: AllowList = Arc::new(tokio::sync::RwLock::new(GuestMap::new()));
    let state = Arc::new(tokio::sync::Mutex::new(StateManager::default()));
    let refresh_alert_gate = Arc::new(tokio::sync::Mutex::new(None));

    // Initial pool refresh + discovery
    refresh_pool_and_publish(&cfg, &client, &pve, &allow, &refresh_alert_gate).await;

    // Spawn polling task
    spawn_polling_task(
        cfg.clone(),
        client.clone(),
        pve.clone(),
        allow.clone(),
        state.clone(),
        refresh_alert_gate.clone(),
    );

    // Main event loop
    run_event_loop(&cfg, &client, &pve, &allow, &state, &mut ev).await?;

    // Publish offline status (best-effort)
    let _ = pub_module::publish_availability(&client, &cfg.mqtt.availability_topic, false).await;

    Ok(())
}

/// Load and validate configuration.
fn load_config() -> Result<RootConfig> {
    let cfg_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.into());

    RootConfig::load(&cfg_path)
        .with_context(|| format!("failed to load config: {}", cfg_path.display()))
}

/// Create Proxmox API client.
fn create_proxmox_client(cfg: &RootConfig) -> Result<ProxmoxClient> {
    let ca_pem = cfg
        .proxmox
        .ca_file
        .as_ref()
        .map(|p| {
            std::fs::read(p)
                .with_context(|| format!("read proxmox ca_file: {}", p))
        })
        .transpose()?;

    ProxmoxClient::new(ProxmoxConfig {
        base_url: cfg.proxmox.base_url.clone(),
        token_id: cfg.proxmox.token_id.clone(),
        token_secret: cfg.proxmox.token_secret.clone(),
        pool: cfg.proxmox.pool.clone(),
        timeout: Duration::from_secs(cfg.proxmox.timeout_secs),
        ca_pem,
    })
        .context("failed to create proxmox client")
}

/// Spawn the background polling task.
fn spawn_polling_task(
    cfg: RootConfig,
    client: rumqttc::AsyncClient,
    pve: ProxmoxClient,
    allow: AllowList,
    state: Arc<tokio::sync::Mutex<StateManager>>,
    refresh_alert_gate: Arc<tokio::sync::Mutex<Option<tokio::time::Instant>>>,
) {
    tokio::spawn(async move {
        let mut tick = time::interval(Duration::from_secs(cfg.agent.poll_interval_secs));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            refresh_pool_and_publish(&cfg, &client, &pve, &allow, &refresh_alert_gate).await;

            // Work on a snapshot to avoid holding the allow-list read lock across network calls.
            let guests = snapshot_guests(&allow).await;
            metrics::publish_metrics(&cfg, &client, &pve, &guests, &state).await;
        }
    });
}

/// Clone the current allow-list so async operations do not hold the RW lock.
async fn snapshot_guests(allow: &AllowList) -> GuestMap {
    allow.read().await.clone()
}

/// Refresh pool members and publish discovery if enabled.
///
/// Reliability behavior:
/// - Discovery is only republished when pool membership changes.
/// - Refresh failure alerts are throttled to avoid MQTT/HA alert floods.
async fn refresh_pool_and_publish(
    cfg: &RootConfig,
    client: &rumqttc::AsyncClient,
    pve: &ProxmoxClient,
    allow: &AllowList,
    refresh_alert_gate: &Arc<tokio::sync::Mutex<Option<tokio::time::Instant>>>,
) {
    match pve.pool_members().await {
        Ok(members) => {
            let idx = index_members(members);
            let changed = {
                let mut w = allow.write().await;
                let changed = *w != idx;
                *w = idx;
                changed
            };
            if cfg.agent.publish_discovery && changed {
                let guests = snapshot_guests(allow).await;
                discovery::publish_discovery_all(cfg, client, &guests).await;
            }
        }
        Err(e) => {
            error!(error = %e, "failed to refresh pool members");
            let now = tokio::time::Instant::now();
            let should_alert = {
                let mut gate = refresh_alert_gate.lock().await;
                let allowed = gate
                    .map(|last| now.duration_since(last).as_secs() >= REFRESH_ALERT_COOLDOWN_SECS)
                    .unwrap_or(true);
                if allowed {
                    *gate = Some(now);
                }
                allowed
            };

            if should_alert {
                let msg = format!("Failed to refresh Proxmox pool members: {e}");
                if let Err(alert_err) = pub_module::publish_alert(client, &cfg.mqtt.alert_topic, &msg).await {
                    error!(error = ?alert_err, "failed to publish alert");
                }
            }
        }
    }
}

/// Main event loop: handle HA commands and HA restarts.
async fn run_event_loop(
    cfg: &RootConfig,
    client: &rumqttc::AsyncClient,
    pve: &ProxmoxClient,
    allow: &AllowList,
    state: &Arc<tokio::sync::Mutex<StateManager>>,
    ev: &mut rumqttc::EventLoop,
) -> Result<()> {
    let shutdown = async { signal::ctrl_c().await.ok() };
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("shutdown requested");
                return Ok(());
            }
            result = mqtt::poll_event(ev) => {
                match result {
                    Ok(Some(event)) => {
                        if event.is_homeassistant_online() && cfg.agent.publish_discovery {
                            let guests = snapshot_guests(allow).await;
                            discovery::publish_discovery_all(cfg, client, &guests).await;
                            continue;
                        }

                        if event.is_command_topic(&cfg.mqtt.topic_prefix) {
                            let guests = snapshot_guests(allow).await;
                            if let Err(e) = commands::handle_command(cfg, client, pve, &guests, state, &event.topic).await {
                                error!(topic = %event.topic, error = %e, "command failed");
                                let msg = format!("Command failed on {}: {}", event.topic, e);
                                if let Err(alert_err) = pub_module::publish_alert(client, &cfg.mqtt.alert_topic, &msg).await {
                                    error!(error = ?alert_err, "failed to publish alert");
                                }
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        error!(error = %e, "mqtt event loop error");
                        time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    }
}
