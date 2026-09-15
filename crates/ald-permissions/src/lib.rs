//! Role-based access control for Aegis and runtime admin actions.
//! Permissions are granular strings; roles are named sets.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Owner,
    SuperAdmin,
    Admin,
    Moderator,
    Developer,
    Support,
    ReadOnly,
}

impl Role {
    pub fn name(&self) -> &'static str {
        match self {
            Role::Owner => "Owner",
            Role::SuperAdmin => "SuperAdmin",
            Role::Admin => "Admin",
            Role::Moderator => "Moderator",
            Role::Developer => "Developer",
            Role::Support => "Support",
            Role::ReadOnly => "ReadOnly",
        }
    }

    /// Default permission set granted to this role.
    pub fn default_permissions(&self) -> Vec<&'static str> {
        match self {
            Role::Owner => vec!["*"],
            Role::SuperAdmin => vec![
                "players.read",
                "players.kick",
                "players.ban",
                "players.identity.read",
                "players.identity.device.read",
                "players.identity.network.read",
                "resources.*",
                "server.*",
                "console.execute",
                "deployment.*",
                "audit.read",
                "config.edit",
            ],
            Role::Admin => vec![
                "players.read",
                "players.kick",
                "players.ban",
                "players.identity.read",
                "resources.restart",
                "resources.configure",
                "server.restart",
                "console.execute",
                "audit.read",
            ],
            Role::Moderator => vec!["players.read", "players.kick", "players.identity.read"],
            Role::Developer => vec![
                "players.read",
                "resources.restart",
                "resources.configure",
                "server.restart",
                "console.execute",
                "audit.read",
            ],
            Role::Support => vec!["players.read", "players.identity.read", "audit.read"],
            Role::ReadOnly => vec!["players.read", "audit.read"],
        }
    }
}

#[derive(Debug, Default)]
pub struct AccessControl {
    role_permissions: HashMap<Role, HashSet<String>>,
    /// Per-principal role assignments (e.g. Aegis user id -> role).
    assignments: HashMap<String, Role>,
    /// Per-principal granular grants layered on top of the role set.
    grants: HashMap<String, HashSet<String>>,
}

impl AccessControl {
    pub fn new() -> Self {
        let mut ac = AccessControl::default();
        for role in [
            Role::Owner,
            Role::SuperAdmin,
            Role::Admin,
            Role::Moderator,
            Role::Developer,
            Role::Support,
            Role::ReadOnly,
        ] {
            let set: HashSet<String> = role.default_permissions().iter().map(|s| s.to_string()).collect();
            ac.role_permissions.insert(role, set);
        }
        ac
    }

    pub fn assign(&mut self, principal: &str, role: Role) {
        self.assignments.insert(principal.to_string(), role);
    }

    pub fn unassign(&mut self, principal: &str) {
        self.assignments.remove(principal);
    }

    /// Grant a single granular permission directly to a principal.
    pub fn grant(&mut self, principal: &str, permission: &str) {
        self.grants.entry(principal.to_string()).or_default().insert(permission.to_string());
    }

    /// Check permission. Wildcard `*` and `scope.*` matching supported.
    pub fn check(&self, principal: &str, permission: &str) -> bool {
        let role = match self.assignments.get(principal) {
            Some(r) => r,
            None => return false,
        };
        let extra = self.grants.get(principal);
        let allowed = |p: &str| {
            p == "*"
                || p == permission
                || permission.split_once('.').is_some_and(|(scope, _)| p == format!("{scope}.*"))
        };
        if let Some(perms) = self.role_permissions.get(role) {
            if perms.iter().any(|p| allowed(p.as_str())) {
                return true;
            }
        }
        if let Some(perms) = extra {
            if perms.iter().any(|p| allowed(p.as_str())) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_has_wildcard() {
        let mut ac = AccessControl::new();
        ac.assign("u1", Role::Owner);
        assert!(ac.check("u1", "anything.here"));
    }

    #[test]
    fn readonly_cannot_kick() {
        let mut ac = AccessControl::new();
        ac.assign("u2", Role::ReadOnly);
        assert!(ac.check("u2", "players.read"));
        assert!(!ac.check("u2", "players.kick"));
    }

    #[test]
    fn scope_wildcard() {
        let mut ac = AccessControl::new();
        ac.assign("u3", Role::Developer);
        ac.grant("u3", "resources.*");
        assert!(ac.check("u3", "resources.stop")); // resources.* grants
        assert!(!ac.check("u3", "deployment.release")); // not granted
    }

    #[test]
    fn unassigned_denied() {
        let ac = AccessControl::new();
        assert!(!ac.check("ghost", "players.read"));
    }
}
