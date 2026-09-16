//! CitizenFX state-bag compatibility: scope naming, key rules, strict mode.
//!
//! Maps the familiar surface — `GlobalState`, `Entity(id).state`,
//! `Player(id).state`, `LocalPlayer.state` — onto [`ald_state`] bags:
//!
//! ```text
//! GlobalState          -> BagScope::Global        -> bag "global"
//! Entity(12).state     -> BagScope::Entity(12)    -> bag "entity:12"
//! Player(4).state      -> BagScope::Player(4)     -> bag "player:4"
//! LocalPlayer.state    -> BagScope::Local         -> bag "localplayer"
//! ```
//!
//! [`BagScope::parse`] / [`BagScope::to_string`] round-trip exactly, so log
//! lines, Aegis views, and wire names cannot drift apart. Malformed names
//! are errors, never silent misroutes.
//!
//! Strict mode ([`StrictBags`]) is the ownership discipline FiveM servers
//! expect: every scoped bag has an owner token, and writes from anyone else
//! are rejected. Bag ids are namespaced (`entity:12`, never bare `12`) so a
//! player scope can never collide with an entity scope.
//!
//! What this crate does NOT do: replication transport, client sync, or the
//! script-language `.state` proxies — those bind in the script-host phases.
//! This crate proves naming, key rules, and write policy.

use ald_state::{OwnerId, Registry, StateBag, StateError, Value, WritePolicy};
use thiserror::Error;

/// Longest accepted state key. Aldivine rule (bounded replication payloads),
/// not a claimed CitizenFX-exact limit.
pub const MAX_STATE_KEY_LEN: usize = 128;

/// A bag address in CitizenFX terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BagScope {
    Global,
    Entity(u32),
    Player(u32),
    Local,
}

impl BagScope {
    /// Canonical bag id backing this scope in the [`Registry`].
    pub fn bag_id(&self) -> String {
        self.to_string()
    }

    /// Parse a canonical name. Accepts exactly the four [`ToString`] shapes
    /// (case-insensitive kind, trimmed). Ids must be plain digits — no signs,
    /// no prefixes, no overflow past u32.
    pub fn parse(s: &str) -> Result<Self, ScopeError> {
        let t = s.trim();
        let lower = t.to_ascii_lowercase();
        if lower == "global" {
            return Ok(BagScope::Global);
        }
        if lower == "localplayer" {
            return Ok(BagScope::Local);
        }
        let (kind, num) = t.split_once(':').ok_or_else(|| ScopeError::BadName(s.to_string()))?;
        // Ids are plain digits: reject signs explicitly (`"+4".parse::<u32>()`
        // would otherwise succeed) and let u32 bound the range.
        if num.starts_with(['+', '-']) || num.is_empty() {
            return Err(ScopeError::BadName(s.to_string()));
        }
        let id: u32 = num.parse().map_err(|_| ScopeError::BadName(s.to_string()))?;
        match kind.to_ascii_lowercase().as_str() {
            "entity" => Ok(BagScope::Entity(id)),
            "player" => Ok(BagScope::Player(id)),
            _ => Err(ScopeError::BadName(s.to_string())),
        }
    }
}

impl std::fmt::Display for BagScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BagScope::Global => write!(f, "global"),
            BagScope::Entity(id) => write!(f, "entity:{id}"),
            BagScope::Player(id) => write!(f, "player:{id}"),
            BagScope::Local => write!(f, "localplayer"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScopeError {
    #[error("bad bag name '{0}' (expected global, localplayer, entity:<n>, player:<n>)")]
    BadName(String),
    #[error("bad state key: {0}")]
    BadKey(String),
    #[error(transparent)]
    State(#[from] StateError),
}

/// Validate a state key under compat rules: non-empty, no NUL, bounded.
pub fn validate_state_key(key: &str) -> Result<(), ScopeError> {
    if key.is_empty() {
        return Err(ScopeError::BadKey("key is empty".into()));
    }
    if key.len() > MAX_STATE_KEY_LEN {
        return Err(ScopeError::BadKey("key too long".into()));
    }
    if key.contains('\0') {
        return Err(ScopeError::BadKey("key contains NUL".into()));
    }
    // Core keys are stricter (64); compat callers must fit both. Enforce the
    // tighter bound here so a key passing this function always passes core.
    if key.len() > ald_state::MAX_KEY_LEN {
        return Err(ScopeError::BadKey("key too long".into()));
    }
    Ok(())
}

/// Strict-mode bags: owner-gated writes per scope over a shared [`Registry`].
pub struct StrictBags {
    registry: Registry,
    owners: std::collections::HashMap<String, OwnerId>,
}

impl StrictBags {
    pub fn new() -> Self {
        StrictBags { registry: Registry::new(), owners: std::collections::HashMap::new() }
    }

    /// Create (or fetch) a scoped bag and record its owner. Re-creating with
    /// a *different* owner is refused: ownership transfer needs an explicit
    /// API, not an accidental re-create.
    pub fn create(&mut self, scope: BagScope, owner: OwnerId) -> Result<(), ScopeError> {
        let id = scope.bag_id();
        if let Some(existing) = self.owners.get(&id) {
            if *existing != owner {
                return Err(ScopeError::BadKey(format!("bag {id} already owned")));
            }
            return Ok(());
        }
        let bag = self.registry.get_or_create(&id, WritePolicy::OwnerOnly);
        bag.set_owner(Some(owner));
        self.owners.insert(id, owner);
        Ok(())
    }

    /// Transfer ownership explicitly. The old owner must match (or the bag be
    /// unowned, which `create` prevents — so this is a checked handoff).
    pub fn transfer(&mut self, scope: BagScope, from: OwnerId, to: OwnerId) -> Result<(), ScopeError> {
        let id = scope.bag_id();
        match self.owners.get(&id) {
            Some(o) if *o == from => {
                if let Some(bag) = self.registry.get_mut(&id) {
                    bag.set_owner(Some(to));
                }
                self.owners.insert(id, to);
                Ok(())
            }
            _ => Err(StateError::NotOwner(from).into()),
        }
    }

    fn bag_mut(&mut self, scope: BagScope) -> Result<&mut StateBag, ScopeError> {
        let id = scope.bag_id();
        self.registry.get_mut(&id).ok_or(ScopeError::BadName(id))
    }

    /// Write through the ownership gate. Unknown scopes (never `create`d)
    /// are errors, not implicit bag creation — strict mode creates nothing
    /// on the write path.
    pub fn set(&mut self, scope: BagScope, key: &str, value: Value, writer: OwnerId) -> Result<u64, ScopeError> {
        validate_state_key(key)?;
        Ok(self.bag_mut(scope)?.set(key, value, writer)?)
    }

    pub fn get(&self, scope: BagScope, key: &str) -> Option<Value> {
        self.registry.get(&scope.bag_id())?.get(key).cloned()
    }

    pub fn owner_of(&self, scope: BagScope) -> Option<OwnerId> {
        self.owners.get(&scope.bag_id()).copied()
    }
}

impl Default for StrictBags {
    fn default() -> Self {
        StrictBags::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_names_round_trip() {
        for scope in [BagScope::Global, BagScope::Entity(12), BagScope::Player(4), BagScope::Local] {
            assert_eq!(BagScope::parse(&scope.to_string()), Ok(scope), "{scope}");
        }
    }

    #[test]
    fn scope_ids_do_not_collide() {
        assert_ne!(BagScope::Entity(4).bag_id(), BagScope::Player(4).bag_id());
        assert_ne!(BagScope::Global.bag_id(), BagScope::Local.bag_id());
    }

    #[test]
    fn malformed_names_rejected() {
        for bad in [
            "",
            "   ",
            "entity",
            "player",
            "entity:",
            "entity:-1",
            "entity:+4",
            "entity:0x10",
            "entity:4294967296",
            "entity:1:2",
            "vehicle:4",
            "global:1",
            "local player",
            "local_player",
        ] {
            assert!(BagScope::parse(bad).is_err(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn parse_tolerates_case_and_whitespace() {
        assert_eq!(BagScope::parse("  GLOBAL "), Ok(BagScope::Global));
        assert_eq!(BagScope::parse("Entity:7"), Ok(BagScope::Entity(7)));
        assert_eq!(BagScope::parse("PLAYER:9"), Ok(BagScope::Player(9)));
    }

    #[test]
    fn key_rules() {
        assert!(validate_state_key("health").is_ok());
        assert!(validate_state_key("").is_err());
        assert!(validate_state_key("a\0b").is_err());
        assert!(validate_state_key(&"k".repeat(MAX_STATE_KEY_LEN + 1)).is_err());
        // Tighter core bound enforced here too: fits compat max but not core.
        assert!(validate_state_key(&"k".repeat(ald_state::MAX_KEY_LEN + 1)).is_err());
        // Boundary: exactly the core max passes both.
        assert!(validate_state_key(&"k".repeat(ald_state::MAX_KEY_LEN)).is_ok());
    }

    #[test]
    fn strict_create_write_transfer() {
        let mut bags = StrictBags::new();
        let scope = BagScope::Entity(12);
        // Write before create: no implicit bags.
        assert!(bags.set(scope, "hp", Value::Int(1), 7).is_err());
        bags.create(scope, 7).unwrap();
        assert_eq!(bags.owner_of(scope), Some(7));
        // Non-owner refused; owner writes.
        assert!(matches!(bags.set(scope, "hp", Value::Int(1), 9), Err(ScopeError::State(StateError::NotOwner(9)))));
        bags.set(scope, "hp", Value::Int(100), 7).unwrap();
        assert_eq!(bags.get(scope, "hp"), Some(Value::Int(100)));
        // Re-create same owner: idempotent. Different owner: refused.
        bags.create(scope, 7).unwrap();
        assert!(bags.create(scope, 9).is_err());
        // Explicit handoff works; old owner loses access.
        bags.transfer(scope, 7, 9).unwrap();
        assert!(bags.set(scope, "hp", Value::Int(50), 7).is_err());
        bags.set(scope, "hp", Value::Int(50), 9).unwrap();
        // Wrong `from` cannot steal.
        assert!(bags.transfer(scope, 7, 10).is_err());
    }

    #[test]
    fn scopes_are_isolated() {
        let mut bags = StrictBags::new();
        bags.create(BagScope::Entity(4), 1).unwrap();
        bags.create(BagScope::Player(4), 2).unwrap();
        bags.set(BagScope::Entity(4), "hp", Value::Int(10), 1).unwrap();
        bags.set(BagScope::Player(4), "hp", Value::Int(20), 2).unwrap();
        assert_eq!(bags.get(BagScope::Entity(4), "hp"), Some(Value::Int(10)));
        assert_eq!(bags.get(BagScope::Player(4), "hp"), Some(Value::Int(20)));
    }

    #[test]
    fn transfer_unknown_scope_refused() {
        let mut bags = StrictBags::new();
        assert!(bags.transfer(BagScope::Player(1), 1, 2).is_err());
        assert_eq!(bags.owner_of(BagScope::Player(1)), None);
    }
}
