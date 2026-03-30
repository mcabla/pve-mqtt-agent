//! Home Assistant MQTT Discovery configuration publishing.

use std::{collections::HashMap, num::NonZeroU32};

use rumqttc::AsyncClient;

use crate::{
    config::RootConfig,
    ha_discovery::Device,
    proxmox::GuestRef,
    publisher as pub_module,
    topic::GuestType,
};

type GuestMap = HashMap<(GuestType, NonZeroU32), GuestRef>;

/// Publish discovery configs for all allowed guests.
pub async fn publish_discovery_all(
    cfg: &RootConfig,
    client: &AsyncClient,
    guests: &GuestMap,
) {
    for g in guests.values() {
        publish_guest_discovery(cfg, client, g).await;
    }
}

/// Publish all discovery configs for a single guest (button + sensors).
pub async fn publish_guest_discovery(cfg: &RootConfig, client: &AsyncClient, guest: &GuestRef) {
    let dev = Device {
        identifiers: vec![format!("pve_{}_{}", guest.guest_type.api_segment(), guest.vmid.get())],
        name: guest.name.clone()
            .unwrap_or_else(|| format!("{} {}", guest.guest_type.api_segment(), guest.vmid.get())),
        manufacturer: "Proxmox VE".into(),
        model: guest.guest_type.api_segment().to_uppercase(),
    };

    let vmid = guest.vmid.get();
    let type_str = guest.guest_type.api_segment();
    let unique_base = format!("{}_{}", type_str, vmid);

    // Publish button discovery
    pub_module::publish_button_discovery(
        client,
        cfg,
        vmid,
        "Reboot",
        &format!("{}_reboot", unique_base),
        &dev,
    )
    .await;

    // Build sensor configurations
    let sensor_ids = [
        format!("{}_status", unique_base),
        format!("{}_cpu", unique_base),
        format!("{}_mem", unique_base),
        format!("{}_disk", unique_base),
        format!("{}_uptime", unique_base),
        format!("{}_last_reboot", unique_base),
    ];
    
    let sensors = build_sensor_configs(&sensor_ids);

    for sensor_cfg in sensors {
        pub_module::publish_sensor_discovery(
            client,
            cfg,
            vmid,
            &sensor_cfg,
            &dev,
        )
        .await;
    }
}

/// Build the fixed set of sensor descriptors exposed for each guest.
fn build_sensor_configs(ids: &[String; 6]) -> [pub_module::SensorConfig<'_>; 6] {
    [
        pub_module::SensorConfig {
            name: "Status",
            unique_id: &ids[0],
            value_template: "{{ value_json.status }}",
            unit_of_measurement: None,
            device_class: None,
            icon: Some("mdi:server"),
        },
        pub_module::SensorConfig {
            name: "CPU",
            unique_id: &ids[1],
            value_template: "{{ (value_json.cpu * 100) | round(1) }}",
            unit_of_measurement: Some("%"),
            device_class: None,
            icon: Some("mdi:cpu-64-bit"),
        },
        pub_module::SensorConfig {
            name: "Memory",
            unique_id: &ids[2],
            value_template: "{{ ((value_json.mem / value_json.maxmem) * 100) | round(1) }}",
            unit_of_measurement: Some("%"),
            device_class: None,
            icon: Some("mdi:memory"),
        },
        pub_module::SensorConfig {
            name: "Disk",
            unique_id: &ids[3],
            value_template: "{{ ((value_json.disk / value_json.maxdisk) * 100) | round(1) }}",
            unit_of_measurement: Some("%"),
            device_class: None,
            icon: Some("mdi:harddisk"),
        },
        pub_module::SensorConfig {
            name: "Uptime",
            unique_id: &ids[4],
            value_template: "{{ value_json.uptime_s | default(0) }}",
            unit_of_measurement: Some("s"),
            device_class: None,
            icon: Some("mdi:timer-outline"),
        },
        pub_module::SensorConfig {
            name: "Last Reboot",
            unique_id: &ids[5],
            value_template: "{{ value_json.last_reboot | default(none, true) }}",
            unit_of_measurement: None,
            device_class: Some("timestamp"),
            icon: Some("mdi:calendar-clock"),
        },
    ]
}
