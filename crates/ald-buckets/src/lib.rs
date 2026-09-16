//! Citizen routing-bucket compatibility over native dimensions.
//!
//! FiveM resources speak buckets (`SetPlayerRoutingBucket`, entity buckets,
//! lockdown, density). Aldivine speaks dimensions (`ald_ecs::dimensions`).
//! This crate is the faithful adapter between them: bucket ids map 1:1 onto
//! [`DimensionId`]s, and the adapter adds the three bucket-level policies
//! dimensions alone do not carry:
//!
//! - **Assignment**: players default to bucket 0; entities default to
//!   dimension 0. Setting either records the change and fires change hooks
//!   with old/new values for audit and Aegis.
//! - **Lockdown**: a lockdown bucket is visible only to same-bucket players
//!   holding an explicit grant. Without lockdown, same-bucket visibility is
//!   open (the dimension rule). Grants are allowlists, never inferred.
//! - **Density**: per-bucket [`DensityPolicy`](ald_population::DensityPolicy)
//!   synced into [`PopulationManager`](ald_population::PopulationManager)
//!   dimensions via [`BucketManager::sync_density`]. One policy type
//!   end to end — buckets never carry a second, divergent density model.
//!
//! Aldivine rule, stated plainly: these are Aldivine's bucket semantics,
//! proven by the tests below — not a claim of bit-exact CitizenFX behavior.
//! The script-language bucket natives bind to this API in later phases.

use std::collections::{HashMap, HashSet};

use ald_ecs::{DimensionId, DimensionMap, EntityId, EntityStore};
use ald_population::{DensityPolicy, PopulationManager};
use thiserror::Error;

/// Citizen bucket id. Bucket 0 is the default plane.
pub type BucketId = u32;

/// Default bucket for players and entities.
pub const DEFAULT_BUCKET: BucketId = 0;

/// Observer of player bucket changes: (player, old_bucket, new_bucket).
pub type BucketChangeHook = Box<dyn FnMut(u32, BucketId, BucketId)>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BucketError {
    #[error("unknown entity")]
    UnknownEntity,
}

/// Player bucket assignments with change hooks.
#[derive(Default)]
pub struct BucketManager {
    player_buckets: HashMap<u32, BucketId>,
    grants: HashSet<(BucketId, u32)>,
    lockdown: HashSet<BucketId>,
    densities: HashMap<BucketId, DensityPolicy>,
    hooks: Vec<BucketChangeHook>,
}

impl BucketManager {
    pub fn new() -> Self {
        BucketManager::default()
    }

    /// Player's bucket (0 when never assigned).
    pub fn player_bucket(&self, player: u32) -> BucketId {
        self.player_buckets.get(&player).copied().unwrap_or(DEFAULT_BUCKET)
    }

    /// Assign a player's bucket, firing hooks with (player, old, new).
    /// Re-assigning the same bucket is a no-op (no hook, no lie).
    pub fn set_player_bucket(&mut self, player: u32, bucket: BucketId) {
        let old = self.player_bucket(player);
        if old == bucket {
            return;
        }
        self.player_buckets.insert(player, bucket);
        for hook in &mut self.hooks {
            hook(player, old, bucket);
        }
    }

    /// Observe player bucket changes.
    pub fn on_player_bucket_change(&mut self, hook: BucketChangeHook) {
        self.hooks.push(hook);
    }

    /// Assign an entity's bucket (= its dimension). Refuses dead ids.
    pub fn set_entity_bucket(
        &self,
        map: &mut DimensionMap,
        store: &EntityStore,
        id: EntityId,
        bucket: BucketId,
    ) -> Result<(), BucketError> {
        if map.dimension_of(store, id).is_none() {
            return Err(BucketError::UnknownEntity);
        }
        map.set(store, id, DimensionId(bucket));
        Ok(())
    }

    /// Entity's bucket (0 when unassigned, `None` when dead).
    pub fn entity_bucket(&self, map: &DimensionMap, store: &EntityStore, id: EntityId) -> Option<BucketId> {
        map.dimension_of(store, id).map(|d| d.0)
    }

    /// Lock (or unlock) a bucket. Locked buckets admit only granted players.
    pub fn set_lockdown(&mut self, bucket: BucketId, locked: bool) {
        if locked {
            self.lockdown.insert(bucket);
        } else {
            self.lockdown.remove(&bucket);
        }
    }

    pub fn is_locked(&self, bucket: BucketId) -> bool {
        self.lockdown.contains(&bucket)
    }

    /// Grant a player visibility into a locked bucket. Idempotent.
    pub fn grant(&mut self, bucket: BucketId, player: u32) {
        self.grants.insert((bucket, player));
    }

    /// Revoke. Returns whether a grant existed.
    pub fn revoke(&mut self, bucket: BucketId, player: u32) -> bool {
        self.grants.remove(&(bucket, player))
    }

    /// Can `player` (in `player_bucket`) see content of `entity_bucket`?
    /// Different buckets never meet (dimension isolation). Same bucket meets
    /// unless locked without a grant.
    pub fn can_see(&self, player: u32, player_bucket: BucketId, entity_bucket: BucketId) -> bool {
        if player_bucket != entity_bucket {
            return false;
        }
        if self.lockdown.contains(&entity_bucket) {
            return self.grants.contains(&(entity_bucket, player));
        }
        true
    }

    /// Set a bucket's population density policy.
    pub fn set_density(&mut self, bucket: BucketId, policy: DensityPolicy) {
        self.densities.insert(bucket, policy);
    }

    pub fn density_for(&self, bucket: BucketId) -> Option<&DensityPolicy> {
        self.densities.get(&bucket)
    }

    /// Push every bucket density into the population manager's dimensions.
    pub fn sync_density(&self, population: &mut PopulationManager) {
        for (bucket, policy) in &self.densities {
            population.set_policy(DimensionId(*bucket), policy.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_ecs::Authority;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn defaults_are_zero() {
        let b = BucketManager::new();
        assert_eq!(b.player_bucket(42), DEFAULT_BUCKET);
        assert!(!b.is_locked(1));
        assert_eq!(b.density_for(0), None);
    }

    #[test]
    fn assign_fires_hooks_with_old_new() {
        let mut b = BucketManager::new();
        let seen: Rc<RefCell<Vec<(u32, BucketId, BucketId)>>> = Rc::new(RefCell::new(vec![]));
        let probe = Rc::clone(&seen);
        b.on_player_bucket_change(Box::new(move |p, old, new| probe.borrow_mut().push((p, old, new))));
        b.set_player_bucket(7, 3);
        b.set_player_bucket(7, 3); // no-op: no second event
        b.set_player_bucket(7, 5);
        assert_eq!(*seen.borrow(), vec![(7, 0, 3), (7, 3, 5)]);
        assert_eq!(b.player_bucket(7), 5);
    }

    #[test]
    fn entity_bucket_roundtrip_and_dead_refused() {
        let b = BucketManager::new();
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let id = s.spawn(Authority::Server);
        assert_eq!(b.entity_bucket(&m, &s, id), Some(0));
        b.set_entity_bucket(&mut m, &s, id, 9).unwrap();
        assert_eq!(b.entity_bucket(&m, &s, id), Some(9));
        assert!(s.destroy(id));
        assert_eq!(b.entity_bucket(&m, &s, id), None);
        assert_eq!(b.set_entity_bucket(&mut m, &s, id, 1), Err(BucketError::UnknownEntity));
    }

    #[test]
    fn open_bucket_visibility() {
        let b = BucketManager::new();
        assert!(b.can_see(1, 0, 0));
        assert!(!b.can_see(1, 0, 2));
        assert!(!b.can_see(1, 2, 0));
    }

    #[test]
    fn lockdown_needs_grant() {
        let mut b = BucketManager::new();
        b.set_lockdown(4, true);
        assert!(b.is_locked(4));
        // Same bucket, no grant: invisible.
        assert!(!b.can_see(1, 4, 4));
        b.grant(4, 1);
        assert!(b.can_see(1, 4, 4));
        assert!(!b.can_see(2, 4, 4));
        // Revoke restores invisibility; unlock restores openness.
        assert!(b.revoke(4, 1));
        assert!(!b.revoke(4, 1));
        assert!(!b.can_see(1, 4, 4));
        b.set_lockdown(4, false);
        assert!(b.can_see(1, 4, 4));
    }

    #[test]
    fn grants_are_bucket_scoped() {
        let mut b = BucketManager::new();
        b.set_lockdown(4, true);
        b.set_lockdown(5, true);
        b.grant(4, 1);
        assert!(b.can_see(1, 4, 4));
        assert!(!b.can_see(1, 5, 5)); // grant for 4 does not open 5
    }

    #[test]
    fn density_sync_drives_spawns() {
        let mut b = BucketManager::new();
        let mut pop = PopulationManager::new();
        b.set_density(2, DensityPolicy::new(2, 0, 1.0, 0.0).unwrap());
        b.sync_density(&mut pop);
        use ald_population::PopulationKind;
        assert_eq!(pop.request_spawn(PopulationKind::Ped, DimensionId(2)), ald_population::SpawnDecision::Allow);
        assert_eq!(pop.request_spawn(PopulationKind::Ped, DimensionId(2)), ald_population::SpawnDecision::Allow);
        assert!(matches!(
            pop.request_spawn(PopulationKind::Ped, DimensionId(2)),
            ald_population::SpawnDecision::Deny { .. }
        ));
        // Unsynced bucket still denies.
        assert!(matches!(
            pop.request_spawn(PopulationKind::Ped, DimensionId(3)),
            ald_population::SpawnDecision::Deny { .. }
        ));
    }

    #[test]
    fn buckets_agree_with_dimensions() {
        // Same-plane visibility through both lenses at once.
        let mut b = BucketManager::new();
        let mut s = EntityStore::new();
        let mut m = DimensionMap::new();
        let e1 = s.spawn(Authority::Server);
        let e2 = s.spawn(Authority::Server);
        b.set_entity_bucket(&mut m, &s, e2, 6).unwrap();
        b.set_player_bucket(1, 6);
        // Dimension lens and bucket lens agree: e1 (plane 0) apart, e2 together.
        assert!(!m.visible_to(&s, e1, e2));
        assert!(b.can_see(1, b.player_bucket(1), b.entity_bucket(&m, &s, e2).unwrap()));
        assert!(!b.can_see(1, b.player_bucket(1), b.entity_bucket(&m, &s, e1).unwrap()));
    }
}
