//! MQTT publishing utilities with consistent error handling.

use rumqttc::{AsyncClient, QoS};
use serde::Serialize;
use tracing::warn;
use anyhow::{Result, Context};

use crate::{
    config::RootConfig,
    ha_discovery::{Device, MqttButtonConfig, MqttSensorConfig},
};

/// Sensor discovery configuration builder.
#[derive(Clone)]
pub struct SensorConfig<'a> {
    pub name: &'a str,
    pub unique_id: &'a str,
    pub value_template: &'a str,
    pub unit_of_measurement: Option<&'a str>,
    pub device_class: Option<&'a str>,
    pub icon: Option<&'a str>,
}

/// Publish a serializable payload to MQTT topic with error logging.
pub async fn publish_json<T: Serialize>(
    client: &AsyncClient,
    topic: &str,
    payload: &T,
    entity_name: &str,
    vmid: u32,
) -> Result<()> {
    let payload_bytes = serde_json::to_vec(payload)
        .context("failed to serialize payload")?;
    
    if payload_bytes.is_empty() {
        warn!(vmid, entity = entity_name, "skipping empty payload");
        return Err(anyhow::anyhow!("empty payload"));
    }
    
    client
        .publish(topic, QoS::AtLeastOnce, true, payload_bytes)
        .await
        .context("failed to publish")?;
    
    Ok(())
}

/// Publish state message (non-retained).
pub async fn publish_state(
    client: &AsyncClient,
    topic: &str,
    payload: &str,
    vmid: u32,
    node: &str,
) -> Result<()> {
    if payload.is_empty() || payload == "{}" {
        warn!(vmid, node, "skipping empty state payload");
        return Err(anyhow::anyhow!("empty payload"));
    }

    client
        .publish(topic, QoS::AtLeastOnce, false, payload)
        .await
        .context("failed to publish state")?;
    
    Ok(())
}

/// Publish alert message.
pub async fn publish_alert(
    client: &AsyncClient,
    topic: &str,
    message: &str,
) -> Result<()> {
    let payload = serde_json::json!({
        "severity": "error",
        "message": message,
    });

    client
        .publish(topic, QoS::AtLeastOnce, false, payload.to_string())
        .await
        .context("failed to publish alert")?;
    
    Ok(())
}

/// Publish availability (online/offline) marker.
pub async fn publish_availability(
    client: &AsyncClient,
    topic: &str,
    online: bool,
) -> Result<()> {
    let status = if online { "online" } else { "offline" };
    client
        .publish(topic, QoS::AtLeastOnce, true, status)
        .await
        .context("failed to publish availability")?;
    
    Ok(())
}

/// Build and publish a sensor discovery config.
pub async fn publish_sensor_discovery(
    client: &AsyncClient,
    cfg: &RootConfig,
    vmid: u32,
    sensor: &SensorConfig<'_>,
    device: &Device,
) {
    let uid = format!("pve_{}", sensor.unique_id);
    let sensor_topic = crate::ha_discovery::discovery_sensor_topic(&cfg.mqtt.discovery_prefix, &uid);

    let sensor_config = MqttSensorConfig {
        name: sensor.name.to_string(),
        unique_id: uid,
        state_topic: format!(
            "{}/state/{}/{}",
            cfg.mqtt.topic_prefix,
            sensor.unique_id.split('_').next().unwrap_or("unknown"),
            vmid
        ),
        value_template: sensor.value_template.to_string(),
        availability_topic: cfg.mqtt.availability_topic.clone(),
        unit_of_measurement: sensor.unit_of_measurement.map(|s| s.to_string()),
        device_class: sensor.device_class.map(|s| s.to_string()),
        icon: sensor.icon.map(|s| s.to_string()),
        expire_after: Some((cfg.agent.poll_interval_secs * 4) as u32),
        device: device.clone(),
    };

    if let Err(e) = publish_json(client, &sensor_topic, &sensor_config, sensor.name, vmid).await {
        warn!(vmid, sensor = sensor.name, error = ?e, "failed to publish sensor discovery");
    }
}

/// Build and publish a button discovery config.
pub async fn publish_button_discovery(
    client: &AsyncClient,
    cfg: &RootConfig,
    vmid: u32,
    button_name: &str,
    unique_id: &str,
    device: &Device,
) {
    let uid = format!("pve_{unique_id}");
    let btn_topic = crate::ha_discovery::discovery_button_topic(&cfg.mqtt.discovery_prefix, &uid);

    let button = MqttButtonConfig {
        name: button_name.to_string(),
        unique_id: uid,
        command_topic: format!(
            "{}/cmd/{}/{}",
            cfg.mqtt.topic_prefix,
            unique_id.split('_').next().unwrap_or("unknown"),
            vmid
        ),
        payload_press: "1".to_string(),
        availability_topic: cfg.mqtt.availability_topic.clone(),
        device: device.clone(),
        entity_category: Some("config".to_string()),
    };

    if let Err(e) = publish_json(client, &btn_topic, &button, button_name, vmid).await {
        warn!(vmid, button = button_name, error = ?e, "failed to publish button discovery");
    }
}
