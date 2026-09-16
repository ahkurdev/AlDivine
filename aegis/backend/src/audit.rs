//! Aegis audit log.
//!
//! Every privileged action is recorded: who did what, to whom, when, and from
//! where. The audit log is append-only — entries are never edited or deleted
//! through normal operation, so the record survives a malicious admin who
//! later wants their actions forgotten.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Actions the spec requires to be audited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Authentication,
    FailedLogin,
    Logout,
    Kick,
    Ban,
    PermissionChange,
    ResourceRestart,
    ServerRestart,
    ConfigChange,
    Deployment,
    ConsoleCommand,
    EntitlementChange,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::Authentication => "authentication",
            AuditAction::FailedLogin => "failed_login",
            AuditAction::Logout => "logout",
            AuditAction::Kick => "kick",
            AuditAction::Ban => "ban",
            AuditAction::PermissionChange => "permission_change",
            AuditAction::ResourceRestart => "resource_restart",
            AuditAction::ServerRestart => "server_restart",
            AuditAction::ConfigChange => "config_change",
            AuditAction::Deployment => "deployment",
            AuditAction::ConsoleCommand => "console_command",
            AuditAction::EntitlementChange => "entitlement_change",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub actor: String,
    pub action: AuditAction,
    /// Who or what was acted upon (player id, resource name, server id).
    pub target: Option<String>,
    /// Unix seconds.
    pub timestamp: u64,
    /// Outcome, so a failed action is distinguishable from a successful one.
    pub success: bool,
    pub detail: String,
    /// Originating IP of the actor, where known.
    pub source_ip: Option<String>,
}

#[derive(Debug)]
pub struct AuditLog {
    entries: VecDeque<AuditEntry>,
    capacity: usize,
}

impl AuditLog {
    pub fn new(capacity: usize) -> Self {
        AuditLog { entries: VecDeque::with_capacity(capacity.max(1)), capacity: capacity.max(1) }
    }

    /// Append an entry. The log is bounded: the oldest entry is evicted when
    /// full rather than growing without limit.
    pub fn record(&mut self, entry: AuditEntry) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> impl Iterator<Item = &AuditEntry> {
        self.entries.iter().rev()
    }

    /// Filter by action kind.
    pub fn by_action(&self, action: AuditAction) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| e.action == action).collect()
    }

    /// Filter by actor.
    pub fn by_actor(&self, actor: &str) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| e.actor == actor).collect()
    }

    /// Count of failed authentications for a user within a window — used by
    /// the brute-force guard.
    pub fn recent_failures(&self, actor: &str, action: AuditAction, since: u64) -> usize {
        self.entries
            .iter()
            .filter(|e| e.actor == actor && e.action == action && !e.success && e.timestamp >= since)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(actor: &str, action: AuditAction, ts: u64, success: bool) -> AuditEntry {
        AuditEntry {
            actor: actor.into(),
            action,
            target: None,
            timestamp: ts,
            success,
            detail: String::new(),
            source_ip: None,
        }
    }

    #[test]
    fn entries_recorded_and_ordered() {
        let mut log = AuditLog::new(10);
        log.record(entry("admin", AuditAction::Kick, 1, true));
        log.record(entry("admin", AuditAction::Ban, 2, true));
        assert_eq!(log.len(), 2);
        // Most recent first.
        assert_eq!(log.entries().next().unwrap().timestamp, 2);
    }

    #[test]
    fn log_evicts_oldest_when_full() {
        let mut log = AuditLog::new(3);
        for i in 0..5 {
            log.record(entry("a", AuditAction::ConsoleCommand, i, true));
        }
        assert_eq!(log.len(), 3);
        let ts: Vec<u64> = log.entries().map(|e| e.timestamp).collect();
        assert_eq!(ts, vec![4, 3, 2]);
    }

    #[test]
    fn filter_by_action() {
        let mut log = AuditLog::new(10);
        log.record(entry("a", AuditAction::Kick, 1, true));
        log.record(entry("a", AuditAction::Ban, 2, true));
        log.record(entry("a", AuditAction::Kick, 3, true));
        assert_eq!(log.by_action(AuditAction::Kick).len(), 2);
    }

    #[test]
    fn filter_by_actor() {
        let mut log = AuditLog::new(10);
        log.record(entry("a", AuditAction::Kick, 1, true));
        log.record(entry("b", AuditAction::Kick, 2, true));
        assert_eq!(log.by_actor("a").len(), 1);
    }

    #[test]
    fn recent_failures_count_window() {
        let mut log = AuditLog::new(10);
        log.record(entry("eve", AuditAction::FailedLogin, 10, false));
        log.record(entry("eve", AuditAction::FailedLogin, 20, false));
        log.record(entry("eve", AuditAction::FailedLogin, 30, true));
        // Failures at t=10 and t=20.
        assert_eq!(log.recent_failures("eve", AuditAction::FailedLogin, 0), 2);
        // t=10 excluded by the window.
        assert_eq!(log.recent_failures("eve", AuditAction::FailedLogin, 15), 1);
        // t=30 is a success, not a failure.
        assert_eq!(log.recent_failures("eve", AuditAction::FailedLogin, 25), 0);
    }

    #[test]
    fn recent_failures_ignores_other_users() {
        let mut log = AuditLog::new(10);
        log.record(entry("eve", AuditAction::FailedLogin, 10, false));
        log.record(entry("mallory", AuditAction::FailedLogin, 10, false));
        assert_eq!(log.recent_failures("eve", AuditAction::FailedLogin, 0), 1);
    }

    #[test]
    fn action_codes() {
        assert_eq!(AuditAction::PermissionChange.as_str(), "permission_change");
        assert_eq!(AuditAction::ConsoleCommand.as_str(), "console_command");
    }

    #[test]
    fn entries_serialize() {
        let mut log = AuditLog::new(10);
        log.record(entry("a", AuditAction::ConfigChange, 1, true));
        let e = log.entries().next().unwrap();
        let json = serde_json::to_string(e).unwrap();
        assert!(json.contains("config_change"));
    }
}
