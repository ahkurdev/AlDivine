//! AstraNet Event Firewall.
//! Network events declare direction, owner resource, max size, rate limit,
//! permission and validation hook. Malformed packets are rejected before
//! gameplay code runs.

use std::collections::HashMap;

use ald_protocol::Channel;

/// Direction an event may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}

/// ACL entry for one network event.
pub struct EventRule {
    pub direction: Direction,
    pub resource: String,
    pub max_size: u32,
    pub rate_per_sec: u32,
    pub permission: Option<String>,
}

/// Firewall evaluating declared event rules.
#[derive(Default)]
pub struct EventFirewall {
    rules: HashMap<String, EventRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirewallVerdict {
    Allow,
    DenyUnknownEvent,
    DenyDirection,
    DenyOversized,
    DenyPermission,
    DenyRate,
}

impl EventFirewall {
    pub fn new() -> Self {
        EventFirewall::default()
    }

    pub fn register(&mut self, name: &str, rule: EventRule) {
        self.rules.insert(name.to_string(), rule);
    }

    /// Check a packet against the firewall. `resource` is the owning resource
    /// of the handler; `permission` is the caller's granted permission set.
    pub fn check(&self, name: &str, direction: Direction, size: u32, permission: Option<&str>) -> FirewallVerdict {
        let Some(rule) = self.rules.get(name) else {
            return FirewallVerdict::DenyUnknownEvent;
        };
        if rule.direction != Direction::Bidirectional && rule.direction != direction {
            return FirewallVerdict::DenyDirection;
        }
        if size > rule.max_size {
            return FirewallVerdict::DenyOversized;
        }
        if let Some(req) = &rule.permission {
            match permission {
                Some(p) if p == req => {}
                _ => return FirewallVerdict::DenyPermission,
            }
        }
        FirewallVerdict::Allow
    }

    pub fn rule(&self, name: &str) -> Option<&EventRule> {
        self.rules.get(name)
    }

    /// Channel allowlist: only these channels accept client-originated traffic.
    /// ResourceTransfer carries client chunk requests; Admin/VoiceMetadata
    /// have no client handler and stay server-only.
    pub fn channel_allowed_client(channel: Channel) -> bool {
        matches!(
            channel,
            Channel::Control
                | Channel::Auth
                | Channel::EventReliable
                | Channel::EventUnreliable
                | Channel::Heartbeat
                | Channel::ResourceTransfer
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(dir: Direction, max: u32, perm: Option<&str>) -> EventRule {
        EventRule {
            direction: dir,
            resource: "core".into(),
            max_size: max,
            rate_per_sec: 10,
            permission: perm.map(str::to_string),
        }
    }

    #[test]
    fn unknown_denied() {
        let fw = EventFirewall::new();
        assert_eq!(fw.check("nope", Direction::ClientToServer, 1, None), FirewallVerdict::DenyUnknownEvent);
    }

    #[test]
    fn wrong_direction_denied() {
        let mut fw = EventFirewall::new();
        fw.register("ev", rule(Direction::ServerToClient, 100, None));
        assert_eq!(fw.check("ev", Direction::ClientToServer, 10, None), FirewallVerdict::DenyDirection);
    }

    #[test]
    fn oversized_denied() {
        let mut fw = EventFirewall::new();
        fw.register("ev", rule(Direction::Bidirectional, 10, None));
        assert_eq!(fw.check("ev", Direction::ClientToServer, 11, None), FirewallVerdict::DenyOversized);
    }

    #[test]
    fn permission_enforced() {
        let mut fw = EventFirewall::new();
        fw.register("ev", rule(Direction::Bidirectional, 100, Some("player.manage")));
        assert_eq!(fw.check("ev", Direction::ClientToServer, 5, None), FirewallVerdict::DenyPermission);
        assert_eq!(fw.check("ev", Direction::ClientToServer, 5, Some("player.manage")), FirewallVerdict::Allow);
    }

    #[test]
    fn client_channels_restricted() {
        assert!(EventFirewall::channel_allowed_client(Channel::EventReliable));
        assert!(!EventFirewall::channel_allowed_client(Channel::Admin));
    }
}
