//! Guest state tracking and reboot detection.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::Duration;

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::topic::{Action, GuestType};

pub type GuestKey = (GuestType, NonZeroU32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAction {
    pub action: Action,
    pub started_at: tokio::time::Instant,
    pub deadline: tokio::time::Instant,
}

/// Manages guest runtime state: uptime tracking, pending actions, and reboot detection.
#[derive(Default)]
pub struct StateManager {
    // Last observed uptime for spontaneous reboot detection
    uptime: HashMap<GuestKey, u64>,
    // Track pending power actions and their deadlines
    pending_actions: HashMap<GuestKey, PendingAction>,
    // Track last observed reboot timestamps
    last_reboots: HashMap<GuestKey, String>,
}

impl StateManager {
    /// Record current uptime for a guest.
    pub fn record_uptime(&mut self, key: GuestKey, uptime_s: u64) {
        self.uptime.insert(key, uptime_s);
    }

    /// Get previously recorded uptime, if any.
    pub fn get_last_uptime(&self, key: GuestKey) -> Option<u64> {
        self.uptime.get(&key).copied()
    }

    /// Register a pending power action.
    pub fn add_pending_action(&mut self, key: GuestKey, action: Action, timeout: Duration) {
        let now = tokio::time::Instant::now();
        self.pending_actions.insert(
            key,
            PendingAction {
                action,
                started_at: now,
                deadline: now + timeout,
            },
        );
    }

    /// Get pending action for a guest, if any.
    pub fn get_pending_action(&self, key: GuestKey) -> Option<PendingAction> {
        self.pending_actions.get(&key).cloned()
    }

    /// Remove pending action (action completed or timed out).
    pub fn clear_pending_action(&mut self, key: GuestKey) {
        self.pending_actions.remove(&key);
    }

    /// Check if a pending action has timed out.
    pub fn is_action_timed_out(&self, key: GuestKey) -> bool {
        if let Some(pending) = self.pending_actions.get(&key) {
            tokio::time::Instant::now() > pending.deadline
        } else {
            false
        }
    }

    /// Record a reboot and return RFC3339 timestamp.
    pub fn record_reboot(&mut self, key: GuestKey) -> String {
        let ts = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        self.last_reboots.insert(key, ts.clone());
        ts
    }

    /// Get the last recorded reboot timestamp.
    pub fn get_last_reboot(&self, key: GuestKey) -> String {
        self.last_reboots.get(&key).cloned().unwrap_or_default()
    }

    /// Detect reboot based on uptime drop.
    /// Returns true if a significant uptime drop is detected.
    pub fn detect_uptime_drop(&self, key: GuestKey, current_uptime: u64, threshold: u64) -> bool {
        if let Some(prev_uptime) = self.get_last_uptime(key) {
            prev_uptime.saturating_sub(current_uptime) > threshold
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_retrieve_uptime() {
        let mut state = StateManager::default();
        let key = (GuestType::Qemu, NonZeroU32::new(100).unwrap());

        state.record_uptime(key, 1000);
        assert_eq!(state.get_last_uptime(key), Some(1000));
    }

    #[test]
    fn test_detect_uptime_drop() {
        let mut state = StateManager::default();
        let key = (GuestType::Qemu, NonZeroU32::new(100).unwrap());

        state.record_uptime(key, 1000);
        assert!(state.detect_uptime_drop(key, 100, 50)); // 1000 - 100 = 900, threshold 50
        assert!(!state.detect_uptime_drop(key, 950, 50)); // 1000 - 950 = 50, not > 50
    }

    #[test]
    fn test_pending_action_tracking() {
        let mut state = StateManager::default();
        let key = (GuestType::Qemu, NonZeroU32::new(100).unwrap());

        assert_eq!(state.get_pending_action(key), None);

        state.add_pending_action(key, Action::Reboot, Duration::from_secs(180));
        assert!(state.get_pending_action(key).is_some());

        state.clear_pending_action(key);
        assert_eq!(state.get_pending_action(key), None);
    }

    #[test]
    fn test_reboot_recording() {
        let mut state = StateManager::default();
        let key = (GuestType::Qemu, NonZeroU32::new(100).unwrap());

        let ts = state.record_reboot(key);
        assert!(!ts.is_empty());
        assert_eq!(state.get_last_reboot(key), ts);
    }
}
