//! Aldivine Framework — players & characters.
//!
//! Identity (the login) is owned by ald-identity; this service owns the
//! gameplay layer on top: characters per account, status effects, metadata.

use std::collections::HashMap;

use ald_core::AldivinePlayerId;

/// Character slot id, unique within an account.
pub type CharacterId = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerStatus {
    Online,
    Offline,
    Banned,
}

#[derive(Debug, Clone)]
pub struct Character {
    pub char_id: CharacterId,
    pub name: String,
    /// Job at last save; the live job lives in JobService.
    pub job: String,
}

#[derive(Debug)]
pub struct Player {
    pub identity: AldivinePlayerId,
    pub status: PlayerStatus,
    pub characters: Vec<Character>,
    /// Active character slot, None while in the character selection screen.
    pub active_character: Option<CharacterId>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("no character slot {0}")]
    NoCharacter(CharacterId),
    #[error("character slot limit reached")]
    SlotLimit,
    #[error("duplicate character name")]
    DuplicateName,
}

/// Player gameplay state. Not a global table: owned by this service and
/// reached through it so that permission checks cannot be skipped.
#[derive(Debug, Default)]
pub struct PlayerService {
    players: HashMap<AldivinePlayerId, Player>,
    /// Reverse index: character name -> owning player, for login lookup.
    names: HashMap<String, AldivinePlayerId>,
}

impl PlayerService {
    const MAX_CHARACTERS: usize = 5;

    pub fn new() -> Self {
        PlayerService::default()
    }

    /// Register a connected identity. Idempotent.
    pub fn join(&mut self, identity: AldivinePlayerId) -> &mut Player {
        self.players.entry(identity).or_insert(Player {
            identity,
            status: PlayerStatus::Online,
            characters: Vec::new(),
            active_character: None,
        })
    }

    pub fn leave(&mut self, identity: &AldivinePlayerId) {
        if let Some(p) = self.players.get_mut(identity) {
            p.status = PlayerStatus::Offline;
            p.active_character = None;
        }
    }

    /// Create a character in a free slot. Names are unique across the server.
    pub fn create_character(&mut self, identity: &AldivinePlayerId, name: String) -> Result<CharacterId, PlayerError> {
        if self.names.contains_key(&name) {
            return Err(PlayerError::DuplicateName);
        }
        let player = self.players.get_mut(identity).ok_or(PlayerError::NoCharacter(0))?;
        if player.characters.len() >= Self::MAX_CHARACTERS {
            return Err(PlayerError::SlotLimit);
        }
        let char_id = (player.characters.len() as u32) + 1;
        self.names.insert(name.clone(), *identity);
        player.characters.push(Character { char_id, name, job: "unemployed".into() });
        Ok(char_id)
    }

    pub fn select_character(&mut self, identity: &AldivinePlayerId, char_id: CharacterId) -> Result<(), PlayerError> {
        let player = self.players.get_mut(identity).ok_or(PlayerError::NoCharacter(char_id))?;
        if !player.characters.iter().any(|c| c.char_id == char_id) {
            return Err(PlayerError::NoCharacter(char_id));
        }
        player.active_character = Some(char_id);
        Ok(())
    }

    pub fn get(&self, identity: &AldivinePlayerId) -> Option<&Player> {
        self.players.get(identity)
    }

    pub fn online_count(&self) -> usize {
        self.players.values().filter(|p| p.status == PlayerStatus::Online).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_and_online_count() {
        let mut s = PlayerService::new();
        let a = AldivinePlayerId::new();
        let b = AldivinePlayerId::new();
        s.join(a);
        s.join(b);
        assert_eq!(s.online_count(), 2);
        s.leave(&a);
        assert_eq!(s.online_count(), 1);
    }

    #[test]
    fn character_lifecycle() {
        let mut s = PlayerService::new();
        let p = AldivinePlayerId::new();
        s.join(p);
        let id = s.create_character(&p, "Rimuru".into()).unwrap();
        assert!(s.select_character(&p, id).is_ok());
        assert_eq!(s.get(&p).unwrap().active_character, Some(id));
    }

    #[test]
    fn duplicate_name_rejected() {
        let mut s = PlayerService::new();
        let p = AldivinePlayerId::new();
        s.join(p);
        s.create_character(&p, "Rimuru".into()).unwrap();
        let q = AldivinePlayerId::new();
        s.join(q);
        assert!(matches!(s.create_character(&q, "Rimuru".into()), Err(PlayerError::DuplicateName)));
    }

    #[test]
    fn slot_limit_enforced() {
        let mut s = PlayerService::new();
        let p = AldivinePlayerId::new();
        s.join(p);
        for i in 0..5 {
            s.create_character(&p, format!("c{i}")).unwrap();
        }
        assert!(matches!(s.create_character(&p, "six".into()), Err(PlayerError::SlotLimit)));
    }

    #[test]
    fn select_unknown_character_fails() {
        let mut s = PlayerService::new();
        let p = AldivinePlayerId::new();
        s.join(p);
        assert!(matches!(s.select_character(&p, 99), Err(PlayerError::NoCharacter(99))));
    }
}
