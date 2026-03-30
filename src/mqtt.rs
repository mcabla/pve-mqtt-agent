//! MQTT connection and event loop management.

use std::fs;

use anyhow::{anyhow, Context, Result};
use rumqttc::{AsyncClient, Event, EventLoop, Incoming, MqttOptions, QoS, Transport};
use tokio::time::Duration;

use crate::config::RootConfig;

/// Establish MQTT connection with TLS and LWT.
pub async fn connect_mqtt(cfg: &RootConfig) -> Result<(AsyncClient, EventLoop)> {
    let mut opts = MqttOptions::new(&cfg.mqtt.client_id, &cfg.mqtt.host, cfg.mqtt.port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_credentials(&cfg.mqtt.username, &cfg.mqtt.password);

    // LWT: retained offline marker
    opts.set_last_will(rumqttc::LastWill::new(
        cfg.mqtt.availability_topic.clone(),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));

    if cfg.mqtt.use_tls {
        // CA is required when use_tls is true (validated in config).
        let ca_path = cfg
            .mqtt
            .ca_file
            .as_deref()
            .ok_or_else(|| anyhow!("mqtt.ca_file missing (config validation should have caught this)"))?;
        let ca = fs::read(ca_path).with_context(|| format!("read mqtt ca_file: {}", ca_path))?;

        // Client auth is optional; requires both cert and key.
        let client_auth = match (
            cfg.mqtt.client_cert_file.as_deref(),
            cfg.mqtt.client_key_file.as_deref(),
        ) {
            (Some(cert_path), Some(key_path)) if !cert_path.trim().is_empty() && !key_path.trim().is_empty() => {
                let cert = fs::read(cert_path)
                    .with_context(|| format!("read mqtt client_cert_file: {}", cert_path))?;
                let key = fs::read(key_path)
                    .with_context(|| format!("read mqtt client_key_file: {}", key_path))?;
                Some((cert, key))
            }
            _ => None,
        };

        opts.set_transport(Transport::tls(ca, client_auth, None));
    }

    let (client, eventloop) = AsyncClient::new(opts, 50);
    Ok((client, eventloop))
}

/// Subscribe to required MQTT topics.
pub async fn subscribe_topics(client: &AsyncClient, cfg: &RootConfig) -> Result<()> {
    client
        .subscribe("homeassistant/status", QoS::AtLeastOnce)
        .await?;
    client
        .subscribe(format!("{}/cmd/#", cfg.mqtt.topic_prefix), QoS::AtLeastOnce)
        .await?;
    Ok(())
}

/// Normalized MQTT publish event used by the main loop.
pub struct IncomingEvent {
    /// Full MQTT topic as received from the broker.
    pub topic: String,
    /// Raw payload bytes.
    pub payload: Vec<u8>,
}

impl IncomingEvent {
    /// Returns true when Home Assistant announces it is online.
    pub fn is_homeassistant_online(&self) -> bool {
        self.topic == "homeassistant/status" && self.payload.as_slice() == b"online"
    }

    /// Returns true when this topic matches the configured command namespace.
    pub fn is_command_topic(&self, topic_prefix: &str) -> bool {
        let prefix = topic_prefix.trim_end_matches('/');
        self.topic
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with("/cmd/"))
    }
}

/// Poll MQTT event loop and return incoming event if any.
pub async fn poll_event(ev: &mut EventLoop) -> Result<Option<IncomingEvent>> {
    match ev.poll().await {
        Ok(Event::Incoming(Incoming::Publish(p))) => Ok(Some(IncomingEvent {
            topic: p.topic.clone(),
            payload: p.payload.to_vec(),
        })),
        Ok(_) => Ok(None),
        Err(e) => Err(e).context("mqtt event loop error"),
    }
}
