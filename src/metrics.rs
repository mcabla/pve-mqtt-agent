//! Guest metrics publishing and state management.

use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use anyhow::Result;
use rumqttc::AsyncClient;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::{error, warn};

use crate::{
    config::RootConfig,
    proxmox::{GuestRef, ProxmoxClient},
    publisher as pub_module,
    state::{GuestKey, StateManager},
    topic::GuestType,
};

type GuestMap = HashMap<(GuestType, NonZeroU32), GuestRef>;

/// Publish metrics for all allowed guests.
pub async fn publish_metrics(
    cfg: &RootConfig,
    client: &AsyncClient,
    pve: &ProxmoxClient,
    guests: &GuestMap,
    state: &Arc<Mutex<StateManager>>,
) {
    for g in guests.values() {
        if let Err(e) = publish_guest_metrics(cfg, client, pve, g, state).await {
            warn!(vmid = g.vmid.get(), node = %g.node, error = %e, "failed to publish guest metrics");
        }
    }
}

/// Poll one guest status from Proxmox and publish normalized state to MQTT.
pub async fn publish_guest_metrics(
    cfg: &RootConfig,
    client: &AsyncClient,
    pve: &ProxmoxClient,
    guest: &GuestRef,
    state: &Arc<Mutex<StateManager>>,
) -> Result<()> {
    let st = pve.guest_status(guest).await?;
    let key = (guest.guest_type, guest.vmid);
    let uptime_s = st.uptime.unwrap_or(0);

    let mut state_mgr = state.lock().await;

    // Determine effective status and handle reboot detection
    let (effective_status, mut alert) = determine_effective_status(
        cfg,
        key,
        &st,
        uptime_s,
        &mut state_mgr,
    );

    // Record uptime for next poll
    state_mgr.record_uptime(key, uptime_s);

    // Detect spontaneous reboot
    if state_mgr.detect_uptime_drop(key, uptime_s, cfg.agent.reboot_uptime_drop_secs)
        && st.status == "running"
        && state_mgr.get_pending_action(key).is_none()
    {
        state_mgr.record_reboot(key);
    }

    let last_reboot = state_mgr.get_last_reboot(key);
    let last_reboot_state = if last_reboot.is_empty() {
        None
    } else {
        Some(last_reboot)
    };
    drop(state_mgr);

    // Send alert if needed
    if let Some(msg) = alert.take() {
        if let Err(e) = pub_module::publish_alert(client, &cfg.mqtt.alert_topic, &msg).await {
            error!(error = ?e, "failed to publish alert");
        }
    }

    // Build and publish state
    let payload = json!({
        "status": effective_status,
        "status_raw": st.status,
        "cpu": st.cpu.unwrap_or(0.0),
        "mem": st.mem.unwrap_or(0),
        "maxmem": st.maxmem.unwrap_or(1),
        "disk": st.disk.unwrap_or(0),
        "maxdisk": st.maxdisk.unwrap_or(1),
        "uptime_s": uptime_s,
        "last_reboot": last_reboot_state,
        "node": guest.node,
        "vmid": guest.vmid.get(),
        "type": guest.guest_type.api_segment(),
    });

    let topic = format!(
        "{}/state/{}/{}",
        cfg.mqtt.topic_prefix,
        guest.guest_type.api_segment(),
        guest.vmid.get()
    );
    pub_module::publish_state(client, &topic, &payload.to_string(), guest.vmid.get(), &guest.node).await?;

    Ok(())
}

/// Determine the status exposed to HA while preserving reboot feedback semantics.
///
/// Returns `(effective_status, optional_alert)`.
pub fn determine_effective_status(
    cfg: &RootConfig,
    key: GuestKey,
    status: &crate::proxmox::GuestStatus,
    uptime_s: u64,
    state: &mut StateManager,
) -> (String, Option<String>) {
    use std::time::Duration;

    let mut effective = status.status.clone();
    let mut alert = None;

    if let Some(pending) = state.get_pending_action(key) {
        if state.is_action_timed_out(key) {
            state.clear_pending_action(key);
            alert = Some(format!(
                "Timeout: action '{}' on {} {} lasted longer than {}s",
                pending.action.api_segment(),
                key.0.api_segment(),
                key.1.get(),
                cfg.agent.action_timeout_secs
            ));
        } else {
            effective = "rebooting".to_string();

            // Check for reboot completion
            let drop_detected = state
                .get_last_uptime(key)
                .map(|p| p.saturating_sub(uptime_s) > cfg.agent.reboot_uptime_drop_secs)
                .unwrap_or(false);
            let low_uptime_after_delay = pending.started_at.elapsed() > Duration::from_secs(3)
                && uptime_s < cfg.agent.reboot_detect_window_secs;

            if drop_detected || low_uptime_after_delay {
                state.clear_pending_action(key);
                state.record_reboot(key);
                effective = status.status.clone();
            }
        }
    }

    (effective, alert)
}
