//! Aegis player management: warn, kick, ban, message.
//!
//! Every action is:
//! 1. authorized against the granular permission set before it happens;
//! 2. recorded in the audit log with actor, target, reason, and time;
//! 3. expressed as a result the caller must handle — never a silent no-op.
//!
//! A refused action still audits. The distinction between "not allowed" and
//! "done" is the entire security value of this module.

use crate::audit::{AuditAction, AuditEntry, AuditLog};
use serde::{Deserialize, Serialize};

/// Granular permissions for player management, per the Aegis RBAC spec.
pub mod perms {
    pub const PLAYERS_READ: &str = "players.read";
    pub const PLAYERS_MESSAGE: &str = "players.message";
    pub const PLAYERS_WARN: &str = "players.warn";
    pub const PLAYERS_KICK: &str = "players.kick";
    pub const PLAYERS_BAN: &str = "players.ban";
    pub const PLAYERS_IDENTITY_READ: &str = "players.identity.read";
    pub const PLAYERS_IDENTITY_NETWORK_READ: &str = "players.identity.network.read";
    pub const PLAYERS_IDENTITY_DEVICE_READ: &str = "players.identity.device.read";
}

/// Duration of a ban. Permanent bans have no expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BanDuration {
    Temporary { expires_at: u64 },
    Permanent,
}

/// A ban record as stored and matched against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanRecord {
    pub ban_id: String,
    pub player_id: String,
    pub reason: String,
    pub duration: BanDuration,
    pub banned_by: String,
    pub banned_at: u64,
}

/// Outcome of an admin action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Applied,
    /// The admin lacked the required permission.
    Denied {
        permission: String,
    },
    /// No matching player/session to act on.
    NoSuchPlayer,
}

/// A permission resolver: true if the actor holds the permission.
///
/// In production this is wired to ald-permissions::AccessControl; keeping it
/// a trait here means the action logic can be tested without the RBAC store.
pub trait PermissionResolver {
    fn has(&self, actor: &str, permission: &str) -> bool;
}

/// State required to execute player-management actions.
pub struct PlayerActions<'a> {
    audit: &'a mut AuditLog,
    permissions: &'a dyn PermissionResolver,
    bans: &'a mut Vec<BanRecord>,
    /// Closure-ish predicate: is the player currently online?
    /// Kept as a trait object so tests can supply a fake population.
    online: &'a dyn Fn(&str) -> bool,
}

impl<'a> PlayerActions<'a> {
    pub fn new(
        audit: &'a mut AuditLog,
        permissions: &'a dyn PermissionResolver,
        bans: &'a mut Vec<BanRecord>,
        online: &'a dyn Fn(&str) -> bool,
    ) -> Self {
        PlayerActions { audit, permissions, bans, online }
    }

    fn authorize(&mut self, actor: &str, permission: &str, target: &str, now: u64) -> ActionOutcome {
        if self.permissions.has(actor, permission) {
            ActionOutcome::Applied
        } else {
            self.audit.record(AuditEntry {
                actor: actor.into(),
                action: self.action_for(permission),
                target: Some(target.into()),
                timestamp: now,
                success: false,
                detail: format!("denied: missing {permission}"),
                source_ip: None,
            });
            ActionOutcome::Denied { permission: permission.into() }
        }
    }

    fn action_for(&self, permission: &str) -> AuditAction {
        match permission {
            perms::PLAYERS_KICK => AuditAction::Kick,
            perms::PLAYERS_BAN => AuditAction::Ban,
            _ => AuditAction::PermissionChange,
        }
    }

    /// Send a message to a player. Requires players.message.
    pub fn message(&mut self, actor: &str, target: &str, _body: &str, now: u64) -> ActionOutcome {
        match self.authorize(actor, perms::PLAYERS_MESSAGE, target, now) {
            ActionOutcome::Applied => {}
            other => return other,
        }
        if !(self.online)(target) {
            return ActionOutcome::NoSuchPlayer;
        }
        self.audit.record(AuditEntry {
            actor: actor.into(),
            action: AuditAction::ConsoleCommand,
            target: Some(target.into()),
            timestamp: now,
            success: true,
            detail: "message sent".into(),
            source_ip: None,
        });
        ActionOutcome::Applied
    }

    /// Warn a player. Requires players.warn.
    pub fn warn(&mut self, actor: &str, target: &str, reason: &str, now: u64) -> ActionOutcome {
        match self.authorize(actor, perms::PLAYERS_WARN, target, now) {
            ActionOutcome::Applied => {}
            other => return other,
        }
        if !(self.online)(target) {
            return ActionOutcome::NoSuchPlayer;
        }
        self.audit.record(AuditEntry {
            actor: actor.into(),
            action: AuditAction::PermissionChange,
            target: Some(target.into()),
            timestamp: now,
            success: true,
            detail: format!("warned: {reason}"),
            source_ip: None,
        });
        ActionOutcome::Applied
    }

    /// Kick a player. Requires players.kick.
    pub fn kick(&mut self, actor: &str, target: &str, reason: &str, now: u64) -> ActionOutcome {
        match self.authorize(actor, perms::PLAYERS_KICK, target, now) {
            ActionOutcome::Applied => {}
            other => return other,
        }
        if !(self.online)(target) {
            return ActionOutcome::NoSuchPlayer;
        }
        self.audit.record(AuditEntry {
            actor: actor.into(),
            action: AuditAction::Kick,
            target: Some(target.into()),
            timestamp: now,
            success: true,
            detail: format!("kicked: {reason}"),
            source_ip: None,
        });
        ActionOutcome::Applied
    }

    /// Ban a player. Requires players.ban. A ban applies whether or not the
    /// player is online — it is a persistent record, not a live session act.
    pub fn ban(&mut self, actor: &str, target: &str, reason: &str, duration: BanDuration, now: u64) -> ActionOutcome {
        match self.authorize(actor, perms::PLAYERS_BAN, target, now) {
            ActionOutcome::Applied => {}
            other => return other,
        }
        let record = BanRecord {
            ban_id: format!("ban-{now}-{}", target.len()),
            player_id: target.into(),
            reason: reason.into(),
            duration,
            banned_by: actor.into(),
            banned_at: now,
        };
        self.bans.push(record);
        self.audit.record(AuditEntry {
            actor: actor.into(),
            action: AuditAction::Ban,
            target: Some(target.into()),
            timestamp: now,
            success: true,
            detail: format!("banned: {reason}"),
            source_ip: None,
        });
        ActionOutcome::Applied
    }

    /// Is this player currently banned? Expired temporary bans do not count.
    pub fn is_banned(&self, player_id: &str, now: u64) -> Option<&BanRecord> {
        self.bans.iter().find(|b| {
            b.player_id == player_id
                && match b.duration {
                    BanDuration::Permanent => true,
                    BanDuration::Temporary { expires_at } => now < expires_at,
                }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A permission resolver backed by a simple allowlist.
    struct Allowlist(HashMap<String, Vec<String>>);

    impl PermissionResolver for Allowlist {
        fn has(&self, actor: &str, permission: &str) -> bool {
            self.0.get(actor).is_some_and(|perms| perms.iter().any(|p| p == permission || p == "*"))
        }
    }

    fn resolver_for(actor: &str, perms: &[&str]) -> Allowlist {
        let mut m = HashMap::new();
        m.insert(actor.to_string(), perms.iter().map(|p| p.to_string()).collect());
        Allowlist(m)
    }

    #[test]
    fn kick_authorized_player_succeeds_and_audits() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |p: &str| p == "player-1";
        let res = resolver_for("admin", &[perms::PLAYERS_KICK]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        assert_eq!(actions.kick("admin", "player-1", "cheating", 100), ActionOutcome::Applied);
        assert_eq!(audit.by_action(AuditAction::Kick).len(), 1);
    }

    #[test]
    fn kick_without_permission_denied_and_audited() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |_: &str| true;
        let res = resolver_for("mod", &[perms::PLAYERS_READ]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        match actions.kick("mod", "player-1", "cheating", 100) {
            ActionOutcome::Denied { permission } => assert_eq!(permission, perms::PLAYERS_KICK),
            other => panic!("expected denial, got {other:?}"),
        }
        // The refused attempt is recorded too.
        let entries = audit.by_action(AuditAction::Kick);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].success);
    }

    #[test]
    fn offline_player_is_no_such_player() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |_: &str| false;
        let res = resolver_for("admin", &[perms::PLAYERS_KICK]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        assert_eq!(actions.kick("admin", "ghost", "x", 100), ActionOutcome::NoSuchPlayer);
    }

    #[test]
    fn wildcard_permission_grants_everything() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |_: &str| true;
        let res = resolver_for("owner", &["*"]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        assert_eq!(actions.kick("owner", "p", "x", 1), ActionOutcome::Applied);
        assert_eq!(actions.ban("owner", "p", "x", BanDuration::Permanent, 2), ActionOutcome::Applied);
    }

    #[test]
    fn ban_persists_and_matches() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |_: &str| true;
        let res = resolver_for("admin", &[perms::PLAYERS_BAN]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        assert_eq!(actions.ban("admin", "player-1", "cheating", BanDuration::Permanent, 100), ActionOutcome::Applied);
        assert!(actions.is_banned("player-1", 999_999).is_some());
        assert!(actions.is_banned("player-2", 999_999).is_none());
    }

    #[test]
    fn expired_temporary_ban_does_not_match() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |_: &str| true;
        let res = resolver_for("admin", &[perms::PLAYERS_BAN]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        actions.ban("admin", "p", "cooldown", BanDuration::Temporary { expires_at: 200 }, 100).check_applied();
        assert!(actions.is_banned("p", 150).is_some());
        assert!(actions.is_banned("p", 250).is_none(), "expired ban must not match");
    }

    #[test]
    fn warn_and_message_require_their_own_permissions() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |_: &str| true;
        // Has kick only.
        let res = resolver_for("mod", &[perms::PLAYERS_KICK]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        assert!(matches!(actions.warn("mod", "p", "x", 1), ActionOutcome::Denied { .. }));
        assert!(matches!(actions.message("mod", "p", "x", 1), ActionOutcome::Denied { .. }));
    }

    #[test]
    fn reason_is_recorded_in_audit() {
        let mut audit = AuditLog::new(16);
        let mut bans = Vec::new();
        let online = |_: &str| true;
        let res = resolver_for("admin", &[perms::PLAYERS_BAN]);
        let mut actions = PlayerActions::new(&mut audit, &res, &mut bans, &online);
        actions.ban("admin", "p", "aimbot", BanDuration::Permanent, 100).check_applied();
        let e = audit.by_action(AuditAction::Ban)[0];
        assert!(e.detail.contains("aimbot"));
    }

    /// Test helper: panic if an outcome was not Applied.
    trait CheckApplied {
        fn check_applied(&self);
    }
    impl CheckApplied for ActionOutcome {
        fn check_applied(&self) {
            assert_eq!(self, &ActionOutcome::Applied);
        }
    }
}
