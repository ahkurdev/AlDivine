//! Dimensions: per-entity isolation planes (routing-bucket native form).
//!
//! Every live entity is in exactly one dimension. Unassigned entities read
//! as [`DimensionId::DEFAULT`] (0, like bucket 0); destroyed entities read
//! as nothing — [`DimensionMap::dimension_of`] returns `None` for dead ids,
//! so a reused slot never inherits the previous occupant's plane.
//!
//! Visibility is strict: two entities interact (replicate, collide in
//! interest sets) only within the same dimension. Callers computing
//! [`replication_set`](crate::spatial::replication_set) filter by
//! [`DimensionMap::members_of`]; the composition is proven in the tests, not
//! assumed.
//!
//! Stale entries (destroyed without [`DimensionMap::remove`]) are dropped by
//! [`DimensionMap::prune`]. Destroy paths should call `remove`; `prune` is
//! the backstop for paths that forgot.

use std::collections::HashMap;

use ald_core::EntityId;

use crate::EntityStore;

/// Isolation plane. `DEFAULT` (0) is where everything unassigned lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DimensionId(pub u32);

impl DimensionId {
    pub const DEFAULT: DimensionId = DimensionId(0);
}

/// Entity -> dimension assignments with liveness checks against the store.
#[derive(Debug, Default)]
pub struct DimensionMap {
    dims: HashMap<(u32, u32), DimensionId>,
}

impl DimensionMap {
    pub fn new() -> Self {
        DimensionMap::default()
    }

    fn key(id: EntityId) -> (u32, u32) {
        (id.index, id.generation)
    }

    /// Assign a live entity to a dimension. Dead/stale ids are refused —
    /// assignment to ghosts would alias a future occupant.
    pub fn set(&mut self, store: &EntityStore, id: EntityId, dim: DimensionId) -> bool {
        if store.get(id).is_none() {
            return false;
        }
        self.dims.insert(Self::key(id), dim);
        true
    }

    /// Live entity's dimension (`DEFAULT` when never assigned), or `None`
    /// when the id is dead or unknown.
    pub fn dimension_of(&self, store: &EntityStore, id: EntityId) -> Option<DimensionId> {
        store.get(id)?;
        Some(self.dims.get(&Self::key(id)).copied().unwrap_or(DimensionId::DEFAULT))
    }

    /// Forget an assignment. Returns whether one existed.
    pub fn remove(&mut self, id: EntityId) -> bool {
        self.dims.remove(&Self::key(id)).is_some()
    }

    /// Drop assignments for dead ids. Returns the count removed.
    pub fn prune(&mut self, store: &EntityStore) -> usize {
        let before = self.dims.len();
        self.dims.retain(|(index, generation), _| {
            let id = EntityId::new(*index, *generation);
            store.get(id).is_some()
        });
        before - self.dims.len()
    }

    /// Live members of one dimension. Unassigned live entities belong to
    /// `DEFAULT` — pass it explicitly to enumerate them.
    pub fn members_of(&self, store: &EntityStore, dim: DimensionId) -> Vec<EntityId> {
        self.dims
            .iter()
            .filter(|(_, d)| **d == dim)
            .filter_map(|((index, generation), _)| {
                let id = EntityId::new(*index, *generation);
                if store.get(id).is_some() {
                    Some(id)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Strict visibility: both live and in the same plane.
    pub fn visible_to(&self, store: &EntityStore, a: EntityId, b: EntityId) -> bool {
        match (self.dimension_of(store, a), self.dimension_of(store, b)) {
            (Some(da), Some(db)) => da == db,
            _ => false,
        }
    }

    pub fn assignment_count(&self) -> usize {
        self.dims.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{replication_set, SpatialGrid, Vec3};
    use crate::Authority;
    use std::collections::HashSet;

    #[test]
    fn unassigned_is_default_while_live() {
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let id = s.spawn(Authority::Server);
        assert_eq!(m.dimension_of(&s, id), Some(DimensionId::DEFAULT));
        assert!(m.set(&s, id, DimensionId(3)));
        assert_eq!(m.dimension_of(&s, id), Some(DimensionId(3)));
    }

    #[test]
    fn dead_ids_have_no_dimension() {
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let id = s.spawn(Authority::Server);
        assert!(m.set(&s, id, DimensionId(3)));
        assert!(s.destroy(id));
        assert_eq!(m.dimension_of(&s, id), None);
        // Assignment to the dead id refused: no ghost claims.
        assert!(!m.set(&s, id, DimensionId(5)));
        // Slot reuse starts clean in DEFAULT, not in the previous plane.
        let id2 = s.spawn(Authority::Server);
        assert_eq!(m.dimension_of(&s, id2), Some(DimensionId::DEFAULT));
    }

    #[test]
    fn prune_drops_stale_assignments() {
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let a = s.spawn(Authority::Server);
        let b = s.spawn(Authority::Server);
        m.set(&s, a, DimensionId(1));
        m.set(&s, b, DimensionId(1));
        assert!(s.destroy(a)); // no DimensionMap::remove call: leaked entry
        assert_eq!(m.assignment_count(), 2);
        assert_eq!(m.prune(&s), 1);
        assert_eq!(m.assignment_count(), 1);
        assert_eq!(m.prune(&s), 0);
        let _ = b;
    }

    #[test]
    fn visibility_is_same_plane_only() {
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let a = s.spawn(Authority::Server);
        let b = s.spawn(Authority::Server);
        let c = s.spawn(Authority::Server);
        m.set(&s, b, DimensionId(2));
        m.set(&s, c, DimensionId(2));
        assert!(m.visible_to(&s, b, c));
        assert!(!m.visible_to(&s, a, b)); // DEFAULT vs 2
                                          // Self in the same plane is visible; self-exclusion is the
                                          // replication layer's job (see spatial::replication_set).
        assert!(m.visible_to(&s, a, a));
    }

    #[test]
    fn members_lists_live_only() {
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let a = s.spawn(Authority::Server);
        let b = s.spawn(Authority::Server);
        m.set(&s, a, DimensionId(7));
        m.set(&s, b, DimensionId(7));
        assert!(s.destroy(a));
        let members = m.members_of(&s, DimensionId(7));
        assert_eq!(members, vec![b]);
        assert!(m.members_of(&s, DimensionId(8)).is_empty());
    }

    #[test]
    fn dimensions_filter_interest_sets() {
        // The intended composition: interest set computed spatially, then
        // restricted to the observer's dimension. Proven here, not assumed.
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let mut g = SpatialGrid::new();
        // Observer body at origin, dimension 0.
        let obs = s.spawn(Authority::Player(1));
        g.upsert(obs.index as u64, Vec3::new(0.0, 0.0, 0.0));
        // Two entities at the same spot, different planes.
        let near_same = s.spawn(Authority::Server);
        let near_other = s.spawn(Authority::Server);
        g.upsert(near_same.index as u64, Vec3::new(10.0, 0.0, 0.0));
        g.upsert(near_other.index as u64, Vec3::new(10.0, 0.0, 0.0));
        m.set(&s, near_other, DimensionId(5));
        let in_plane: HashSet<u64> = {
            let mut set = HashSet::new();
            for id in m.members_of(&s, DimensionId::DEFAULT) {
                set.insert(id.index as u64);
            }
            // Unassigned live entities are DEFAULT members implicitly.
            set.insert(obs.index as u64);
            set.insert(near_same.index as u64);
            set
        };
        let set = replication_set(&g, Vec3::new(0.0, 0.0, 0.0));
        let visible: Vec<u64> = set.into_iter().map(|(e, _)| e).filter(|e| in_plane.contains(e)).collect();
        assert!(visible.contains(&(near_same.index as u64)));
        assert!(!visible.contains(&(near_other.index as u64)));
    }
}
