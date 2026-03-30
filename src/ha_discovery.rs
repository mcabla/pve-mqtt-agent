//! Home Assistant MQTT discovery payloads.
//!
//! Discovery is documented in the HA MQTT integration docs. :contentReference[oaicite:21]{index=21}
//! We publish retained config messages so entities survive restarts.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub identifiers: Vec<String>,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct MqttButtonConfig {
    pub name: String,
    pub unique_id: String,
    pub command_topic: String,
    pub payload_press: String,
    pub availability_topic: String,
    pub device: Device,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_category: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MqttSensorConfig {
    pub name: String,
    pub unique_id: String,
    pub state_topic: String,
    pub value_template: String,
    pub availability_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_of_measurement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_after: Option<u32>,
    pub device: Device,
}

pub fn discovery_button_topic(prefix: &str, unique_id: &str) -> String {
    format!("{}/button/{}/config", prefix.trim_end_matches('/'), unique_id)
}

pub fn discovery_sensor_topic(prefix: &str, unique_id: &str) -> String {
    format!("{}/sensor/{}/config", prefix.trim_end_matches('/'), unique_id)
}
