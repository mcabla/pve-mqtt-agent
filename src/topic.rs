//! MQTT topic parsing for Proxmox control commands.
//!
//! Command topic format (no payload required):
//! `{topic_prefix}/cmd/{type}/{vmid}/{action}`
//!
//! Example:
//! `proxmox/ha/cmd/qemu/100/reboot`

use std::num::NonZeroU32;

use thiserror::Error;

/// Guest type supported by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuestType {
    /// QEMU virtual machine.
    Qemu,
    /// LXC container.
    Lxc,
}

impl GuestType {
    /// Parse `qemu` or `lxc`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "qemu" => Some(Self::Qemu),
            "lxc" => Some(Self::Lxc),
            _ => None,
        }
    }

    /// Return the API path segment.
    pub fn api_segment(self) -> &'static str {
        match self {
            Self::Qemu => "qemu",
            Self::Lxc => "lxc",
        }
    }
}

/// Power actions we allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Reboot guest.
    Reboot,
    /// Start guest.
    Start,
    /// Shutdown guest (graceful).
    Shutdown,
    /// Stop guest (hard).
    Stop,
}

impl Action {
    /// Parse action string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reboot" => Some(Self::Reboot),
            "start" => Some(Self::Start),
            "shutdown" => Some(Self::Shutdown),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }

    /// API subpath segment (Proxmox uses these under `/status/`).
    pub fn api_segment(self) -> &'static str {
        match self {
            Self::Reboot => "reboot",
            Self::Start => "start",
            Self::Shutdown => "shutdown",
            Self::Stop => "stop",
        }
    }
}

/// Parsed command from a topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Guest type.
    pub guest_type: GuestType,
    /// VMID (strictly positive).
    pub vmid: NonZeroU32,
    /// Requested action.
    pub action: Action,
}

#[derive(Debug, Error)]
pub enum TopicParseError {
    /// Topic does not match expected prefix or structure.
    #[error("topic does not match expected format")]
    InvalidFormat,
    /// Unknown guest type.
    #[error("unsupported guest type")]
    UnsupportedType,
    /// Invalid VMID.
    #[error("invalid vmid")]
    InvalidVmid,
    /// Unsupported action.
    #[error("unsupported action")]
    UnsupportedAction,
}

/// Parse a command topic of the form:
/// `{topic_prefix}/cmd/{type}/{vmid}/{action}`
pub fn parse_command_topic(topic_prefix: &str, topic: &str) -> Result<Command, TopicParseError> {
    // We avoid allocations: split and walk.
    let mut it = topic.split('/');

    // prefix may itself contain slashes.
    let prefix_parts: Vec<&str> = topic_prefix.split('/').collect();
    for p in &prefix_parts {
        if it.next() != Some(*p) {
            return Err(TopicParseError::InvalidFormat);
        }
    }

    if it.next() != Some("cmd") {
        return Err(TopicParseError::InvalidFormat);
    }

    let typ = it.next().ok_or(TopicParseError::InvalidFormat)?;
    let guest_type = GuestType::parse(typ).ok_or(TopicParseError::UnsupportedType)?;

    let vmid_str = it.next().ok_or(TopicParseError::InvalidFormat)?;
    let vmid_u32: u32 = vmid_str.parse().map_err(|_| TopicParseError::InvalidVmid)?;
    let vmid = NonZeroU32::new(vmid_u32).ok_or(TopicParseError::InvalidVmid)?;

    let act = it.next().ok_or(TopicParseError::InvalidFormat)?;
    let action = Action::parse(act).ok_or(TopicParseError::UnsupportedAction)?;

    // No extra path segments allowed.
    if it.next().is_some() {
        return Err(TopicParseError::InvalidFormat);
    }

    Ok(Command { guest_type, vmid, action })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid() {
        let c = parse_command_topic("proxmox/ha", "proxmox/ha/cmd/qemu/100/reboot").unwrap();
        assert_eq!(c.guest_type, GuestType::Qemu);
        assert_eq!(c.vmid.get(), 100);
        assert_eq!(c.action, Action::Reboot);
    }

    #[test]
    fn rejects_extra_segments() {
        assert!(parse_command_topic("proxmox/ha", "proxmox/ha/cmd/qemu/100/reboot/x").is_err());
    }

    #[test]
    fn rejects_zero_vmid() {
        assert!(parse_command_topic("proxmox/ha", "proxmox/ha/cmd/qemu/0/reboot").is_err());
    }
}
