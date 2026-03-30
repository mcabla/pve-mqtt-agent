//! Command handling from MQTT topics.

use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use anyhow::Result;
use rumqttc::AsyncClient;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::error;

use crate::{
    config::RootConfig,
    proxmox::{GuestRef, ProxmoxClient},
    publisher as pub_module,
    state::StateManager,
    topic::{parse_command_topic, Action, GuestType},
};

type GuestMap = HashMap<(GuestType, NonZeroU32), GuestRef>;

/// Parse, authorize, and execute one MQTT command topic.
///
/// This function enforces allow-list membership before issuing any Proxmox action.
pub async fn handle_command(
    cfg: &RootConfig,
    client: &AsyncClient,
    pve: &ProxmoxClient,
    guests: &GuestMap,
    state: &Arc<Mutex<StateManager>>,
    topic: &str,
) -> Result<()> {
    let cmd = parse_command_topic(&cfg.mqtt.topic_prefix, topic)
        .map_err(|e| anyhow::anyhow!(e))?;

    let Some(g) = guests.get(&(cmd.guest_type, cmd.vmid)) else {
        let msg = format!("Denied command for non-whitelisted guest: {topic}");
        pub_module::publish_alert(client, &cfg.mqtt.alert_topic, &msg).await?;
        return Ok(());
    };

    // Enforce action preconditions to avoid unnecessary/no-op API calls.
    let st = pve.guest_status(g).await?;
    let is_running = st.status == "running";

    match cmd.action {
        Action::Start if is_running => {
            let msg = format!(
                "Refused start: guest already running ({} {})",
                g.guest_type.api_segment(),
                g.vmid.get()
            );
            pub_module::publish_alert(client, &cfg.mqtt.alert_topic, &msg).await?;
            return Ok(());
        }
        Action::Stop | Action::Shutdown | Action::Reboot if !is_running => {
            let msg = format!(
                "Refused {}: guest not running ({} {}): {}",
                cmd.action.api_segment(),
                g.guest_type.api_segment(),
                g.vmid.get(),
                st.status
            );
            pub_module::publish_alert(client, &cfg.mqtt.alert_topic, &msg).await?;
            return Ok(());
        }
        _ => {}
    }

    // For reboot, mark action as pending for UI feedback and reboot detection.
    if cmd.action == Action::Reboot {
        let mut state_mgr = state.lock().await;
        state_mgr.add_pending_action(
            (cmd.guest_type, cmd.vmid),
            cmd.action,
            std::time::Duration::from_secs(cfg.agent.action_timeout_secs),
        );
    }

    // Execute power action
    let upid = pve.power_action(g, cmd.action).await?;
    
    let resp_topic = format!(
        "{}/resp/{}/{}",
        cfg.mqtt.topic_prefix,
        g.guest_type.api_segment(),
        g.vmid.get()
    );
    
    let resp = json!({
        "ok": true,
        "action": cmd.action.api_segment(),
        "upid": upid,
        "node": g.node,
        "vmid": g.vmid.get(),
        "type": g.guest_type.api_segment(),
    });
    
    if let Err(e) = client.publish(resp_topic, rumqttc::QoS::AtLeastOnce, false, resp.to_string()).await {
        error!(error = ?e, "failed to publish command response");
    }
    
    Ok(())
}
