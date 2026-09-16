//! Minimal ECS-style entity store with generation-safe IDs, components, and
//! ownership/authority tracking. Spatial indexing and replication live in later
//! phases (ald-network / replication crate).

use std::collections::HashMap;

pub use ald_core::EntityId;

pub mod dimensions;
pub mod ownership;
pub mod spatial;

pub use dimensions::{DimensionId, DimensionMap};
pub use ownership::{owner_of, transfer, TransferOutcome, TransferRefusal};
pub use spatial::{interest_tier, replication_set, InterestTier, SpatialGrid, Vec3};

/// Owner of an entity. Server-authoritative by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    Server,
    Player(u64),
    Resource(u64),
}

/// Component storage keyed by name per entity.
#[derive(Debug, Clone)]
pub struct Entity {
    pub id: EntityId,
    pub authority: Authority,
    pub components: HashMap<String, Vec<u8>>,
    pub active: bool,
}

#[derive(Default)]
pub struct EntityStore {
    slots: Vec<Option<Entity>>,
    free: Vec<u32>,
    generations: Vec<u32>,
}

impl EntityStore {
    pub fn new() -> Self {
        EntityStore::default()
    }

    /// Spawn a new entity with a fresh generation. Returns generation-safe id.
    pub fn spawn(&mut self, authority: Authority) -> EntityId {
        let index = if let Some(i) = self.free.pop() {
            i
        } else {
            self.slots.push(None);
            self.generations.push(0);
            (self.slots.len() - 1) as u32
        };
        let gen = self.generations[index as usize];
        let id = EntityId::new(index, gen);
        self.slots[index as usize] = Some(Entity { id, authority, components: HashMap::new(), active: true });
        id
    }

    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        let slot = self.slots.get(id.index as usize)?;
        match slot {
            Some(e) if e.id.generation == id.generation && e.active => Some(e),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        let slot = self.slots.get_mut(id.index as usize)?;
        match slot {
            Some(e) if e.id.generation == id.generation && e.active => Some(e),
            _ => None,
        }
    }

    /// Destroy an entity, bumping its generation so stale ids never match.
    pub fn destroy(&mut self, id: EntityId) -> bool {
        let slot = self.slots.get_mut(id.index as usize);
        match slot {
            Some(s) if s.as_ref().map(|e| e.id.generation) == Some(id.generation) => {
                *s = None;
                self.free.push(id.index);
                self.generations[id.index as usize] += 1;
                true
            }
            _ => false,
        }
    }

    pub fn set_component(&mut self, id: EntityId, name: &str, data: Vec<u8>) -> Result<(), &'static str> {
        let e = self.get_mut(id).ok_or("entity not found or stale")?;
        e.components.insert(name.to_string(), data);
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.as_ref().map(|e| e.active).unwrap_or(false)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_get() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Server);
        assert!(s.get(id).is_some());
        assert!(s.get_mut(id).is_some());
        s.set_component(id, "position", vec![1, 2, 3]).unwrap();
        assert_eq!(s.get(id).unwrap().components["position"], vec![1, 2, 3]);
    }

    #[test]
    fn stale_id_after_destroy() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Server);
        assert!(s.destroy(id));
        assert!(s.get(id).is_none());
        // new spawn at same index gets a new generation
        let id2 = s.spawn(Authority::Server);
        assert_ne!(id.generation, id2.generation);
        assert!(s.get(id2).is_some());
        assert!(s.get(id).is_none());
    }

    #[test]
    fn authority_tracked() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        assert_eq!(s.get(id).unwrap().authority, Authority::Player(7));
    }
}
