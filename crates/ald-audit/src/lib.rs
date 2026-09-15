//! Audit log model. Every privileged action (kick, ban, config change, deploy,
//! console command) is recorded with actor, target, time, and outcome.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditCategory {
    Auth,
    Player,
    Resource,
    Server,
    Config,
    Deployment,
    Console,
    Security,
}

impl AuditCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditCategory::Auth => "AUTH",
            AuditCategory::Player => "PLAYER",
            AuditCategory::Resource => "RESOURCE",
            AuditCategory::Server => "SERVER",
            AuditCategory::Config => "CONFIG",
            AuditCategory::Deployment => "DEPLOYMENT",
            AuditCategory::Console => "CONSOLE",
            AuditCategory::Security => "SECURITY",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp_ms: u64,
    pub category: AuditCategory,
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub outcome: AuditOutcome,
    pub metadata: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditOutcome {
    Success,
    Failure,
}

impl AuditEntry {
    pub fn new(category: AuditCategory, actor: &str, action: &str, outcome: AuditOutcome) -> Self {
        AuditEntry {
            timestamp_ms: ald_core::now_ms(),
            category,
            actor: actor.to_string(),
            action: action.to_string(),
            target: None,
            outcome,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn with_target(mut self, target: &str) -> Self {
        self.target = Some(target.to_string());
        self
    }

    pub fn with_meta(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_carries_metadata() {
        let e = AuditEntry::new(AuditCategory::Player, "admin", "kick", AuditOutcome::Success)
            .with_target("player-7")
            .with_meta("reason", "spam");
        assert_eq!(e.target.as_deref(), Some("player-7"));
        assert_eq!(e.metadata.get("reason").map(|s| s.as_str()), Some("spam"));
        assert_eq!(e.category.as_str(), "PLAYER");
    }
}
