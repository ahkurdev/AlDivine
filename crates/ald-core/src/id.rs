use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Platform-level internal player identity.
/// ULID gives time-sortable, collision-resistant, 128-bit identifiers.
/// External platform IDs (Steam/RS/Epic) must never be the primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct AldivinePlayerId(pub Ulid);

impl AldivinePlayerId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    pub fn from_string(s: &str) -> Result<Self, ulid::DecodeError> {
        Ok(Self(Ulid::from_string(s)?))
    }
}

impl Default for AldivinePlayerId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AldivinePlayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Entity identifier with generation/version safety.
/// Reusing a slot index requires the generation to match, preventing
/// stale references to destroyed entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct EntityId {
    pub index: u32,
    pub generation: u32,
}

impl EntityId {
    pub fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "e{}.{}", self.index, self.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_id_roundtrip() {
        let id = AldivinePlayerId::new();
        let s = id.to_string();
        let back = AldivinePlayerId::from_string(&s).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn player_id_unique() {
        let a = AldivinePlayerId::new();
        let b = AldivinePlayerId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn entity_id_equality() {
        let e1 = EntityId::new(1, 1);
        let e2 = EntityId::new(1, 2);
        assert_ne!(e1, e2);
        assert_eq!(e1, EntityId::new(1, 1));
    }
}
