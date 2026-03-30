# pve-mqtt-agent

Proxmox guest control and metrics for Home Assistant using MQTT.

This agent:
1. Publishes guest state/metrics for VM and LXC guests.
2. Accepts power commands over MQTT (`start`, `stop`, `shutdown`, `reboot`).
3. Publishes Home Assistant discovery payloads so entities appear automatically.

The intended deployment is an unprivileged LXC with a restricted Proxmox API token scoped to a single pool.

## Why This Exists (Instead of Direct HA Proxmox Integration)

The main goal is to reduce security blast radius.

You can absolutely control Proxmox directly from Home Assistant, but that usually means storing a Proxmox credential in Home Assistant with meaningful infrastructure privileges.

This project uses a stricter model:

1. Home Assistant only talks this agent using MQTT.
2. This agent talks to Proxmox.
3. The Proxmox token is stored only on the agent host.
4. Firewall policy can block Home Assistant from reaching Proxmox entirely.

In other words, this agent acts as a security boundary.

### Threat model in plain language

The design assumes any component can eventually be compromised, including a public-facing Home Assistant instance.

If Home Assistant is compromised in this model:

1. The attacker gets MQTT-level control paths that you explicitly expose.
2. The attacker does not automatically get direct Proxmox API credentials from Home Assistant.
3. Network rules can still prevent direct HA -> Proxmox access.
4. Network rules can restrict HA to only have MQTT access to this agent. Other network communication can be blocked.

That does not make the system invulnerable, but it does make compromise impact smaller and easier to contain.

### What this architecture improves

1. Secret isolation: Proxmox token does not need to live in Home Assistant.
2. Least privilege: agent token can be limited to one pool and power/state actions only.
3. Network segmentation: only agent needs Proxmox API access.
4. Operational control: a single place to enforce topic validation, allow-list checks, and rate/alert guardrails.

## How It Works

Data flow:

1. Home Assistant sends a command to MQTT.
2. `pve-mqtt-agent` validates the topic and checks if the guest is in the allowed pool.
3. Agent executes the action against Proxmox API.
4. Agent publishes guest state, response messages, alerts, and availability.

## Safety and Load Controls

The current code includes guardrails to avoid overloading Proxmox, the broker, or Home Assistant:

1. Polling loop does not "catch up" in bursts after delays (missed ticks are delayed, not replayed).
2. Discovery payloads are only republished when pool membership changes (not every cycle).
3. Pool refresh failure alerts are throttled with a cooldown (5 minutes) to avoid alert floods.
4. Poll interval is validated (`poll_interval_secs >= 5`).
5. Proxmox calls use a request timeout (`proxmox.timeout_secs`).

These controls significantly reduce overload risk, but no distributed system can guarantee zero load impact under all external conditions.

## Prerequisites

1. Proxmox VE reachable from the agent.
2. MQTT broker reachable from the agent (Mosquitto is common).
3. Home Assistant MQTT integration enabled.
4. A Proxmox pool containing only guests you want to expose.
5. A Proxmox API token with minimum required rights on that pool.

## Quick Start

### 1) Prepare config

Create a local runtime config first (required by the installer script):

```bash
cp config.example.toml config.toml
```

Edit `config.toml` with your actual MQTT and Proxmox values.

### 2) Install using the helper script (recommended)

Use [build_install.sh](build_install.sh). It automates service-user setup, directory creation, config install, binary install, and service restart/start.

```bash
sudo sh ./build_install.sh
```

Optional overrides:

```bash
sudo BIN_DIR=/opt/pve-mqtt-agent/bin \
  CONFIG_DIR=/etc/pve-mqtt-agent \
  STATE_DIR=/var/lib/pve-mqtt-agent \
  SERVICE_NAME=pve-mqtt-agent \
  sh ./build_install.sh
```

### 3) Manual build path (alternative)

```bash
cargo build --release
```

Binary output:

```text
target/release/pve-mqtt-agent
```

### 2) Create config

If you use the helper script, this section is optional. Use it only for manual installs.

Copy [config.example.toml](config.example.toml) to your runtime path:

```bash
cp config.example.toml /etc/pve-mqtt-agent/config.toml
```

Or pass a custom path as first argument:

```bash
pve-mqtt-agent /path/to/config.toml
```

Default config path used by the binary:

```text
/etc/pve-mqtt-agent/config.toml
```

### 4) Minimal Proxmox setup

1. Create pool `ha-mqtt` (or your own name).
2. Add target VM/LXC guests to that pool.
3. Create user + API token.
4. Assign ACL on `/pool/<your-pool>` with minimum required privileges.

### 5) Start agent

```bash
/opt/pve-mqtt-agent/bin/pve-mqtt-agent /etc/pve-mqtt-agent/config.toml
```

### 6) Verify in Home Assistant

1. MQTT integration is connected.
2. Discovery entities appear automatically.
3. Availability topic reports `online`.

## Configuration Reference

Top-level sections:

1. `[agent]`
2. `[mqtt]`
3. `[proxmox]`

### `[agent]`

1. `poll_interval_secs`: poll frequency for state collection.
2. `publish_discovery`: enable/disable HA discovery publishing.
3. `action_timeout_secs`: timeout window for pending actions.
4. `reboot_detect_window_secs`: low-uptime reboot detection window.
5. `reboot_uptime_drop_secs`: uptime drop threshold for reboot detection.

### `[mqtt]`

1. `host`, `port`, `client_id`, `username`, `password`.
2. `use_tls`:
   1. `false`: plain MQTT (typically 1883).
   2. `true`: TLS enabled; `ca_file` is required.
3. Optional mTLS fields: `client_cert_file`, `client_key_file` (must be provided together).
4. Topics:
   1. `topic_prefix`
   2. `discovery_prefix`
   3. `availability_topic`
   4. `alert_topic`

### `[proxmox]`

1. `base_url`: `https://<host>:8006`.
2. `token_id`, `token_secret`.
3. `pool`: allow-list source.
4. `timeout_secs`: API timeout.
5. `ca_file`: optional custom CA bundle.

## MQTT Topics

Assuming `topic_prefix = "proxmox/ha"`.

### Commands (HA -> agent)

```text
proxmox/ha/cmd/{qemu|lxc}/{vmid}/{action}
```

Allowed actions:

1. `start`
2. `stop`
3. `shutdown`
4. `reboot`

Example:

```text
proxmox/ha/cmd/qemu/100/reboot
```

### State (agent -> HA)

```text
proxmox/ha/state/{qemu|lxc}/{vmid}
```

Example payload:

```json
{
  "status": "running",
  "status_raw": "running",
  "cpu": 0.02,
  "mem": 123456789,
  "maxmem": 2147483648,
  "disk": 1234567890,
  "maxdisk": 8589934592,
  "uptime_s": 12345,
  "last_reboot": "2026-03-30T10:00:00Z",
  "node": "pve1",
  "vmid": 100,
  "type": "qemu"
}
```

### Responses (agent -> HA)

```text
proxmox/ha/resp/{qemu|lxc}/{vmid}
```

### Alerts (agent -> HA)

```text
proxmox/ha/alert
```

### Availability (retained)

```text
proxmox/ha/availability
```

Payload:

1. `online`
2. `offline`

## Running as OpenRC Service (Alpine)

Example `/etc/init.d/pve-mqtt-agent`:

```sh
#!/sbin/openrc-run

name="pve-mqtt-agent"
description="Proxmox MQTT Control Agent for Home Assistant"
command="/opt/pve-mqtt-agent/bin/pve-mqtt-agent"
command_args="/etc/pve-mqtt-agent/config.toml"
command_user="pveagent:pveagent"
directory="/var/lib/pve-mqtt-agent"
pidfile="/run/${RC_SVCNAME}.pid"

supervisor="supervise-daemon"
respawn_delay=2
respawn_max=0

output_log="/var/log/pve-mqtt-agent.log"
error_log="/var/log/pve-mqtt-agent.log"

depend() {
  need net
  after firewall
}

start_pre() {
  checkpath -d -o "${command_user}" -m 0750 /var/lib/pve-mqtt-agent
  checkpath -f -o "${command_user}" -m 0640 "${output_log}"
}
```

Enable and start:

```sh
rc-update add pve-mqtt-agent default
rc-service pve-mqtt-agent start
```

## Troubleshooting

### MQTT connection refused

1. Verify broker host/port.
2. Verify TLS settings match broker mode.
3. Verify network ACL/firewall from agent to broker.

### Proxmox TLS errors (`UnknownIssuer`)

1. Configure `proxmox.ca_file` with correct CA chain.
2. Prefer a proper trusted certificate for Proxmox API.

### Discovery entities do not appear

1. Verify Home Assistant MQTT integration is connected.
2. Verify `discovery_prefix` matches HA integration setting.
3. Restart HA or publish birth message if needed.

### Commands ignored

1. Verify topic format exactly matches:
   1. `{topic_prefix}/cmd/{type}/{vmid}/{action}`
2. Verify guest is in configured Proxmox pool.
3. Verify token ACL includes required power permissions.

## Production Recommendations

1. Use TLS for MQTT (`use_tls = true`) with CA validation.
2. Use a dedicated API token and least-privilege ACL on one pool.
3. Keep `poll_interval_secs` conservative (10-30s is a practical range for most setups).
4. Keep the agent in a restricted network segment.
5. Monitor alert topic and service logs.

## License

Apache License, Version 2.0. See [LICENSE](LICENSE).
