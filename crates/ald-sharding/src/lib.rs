//! Aldivine Sharding Groundwork, Player Handoff & Distributed Ownership
//!
//! Provides spatial world partitioning across shards, atomic two-phase player
//! handoffs across shard boundaries, and epoch-versioned distributed ownership leases.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

pub type ShardId = u32;

/// A bounding rectangular region assigned to an individual server shard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShardRegion {
    pub shard_id: ShardId,
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
}

impl ShardRegion {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }
}

/// Lifecycle phase of an atomic player handoff between two shards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandoffPhase {
    Initiated,
    Prepared,
    Committed,
    Aborted { reason: String },
}

/// Handoff contract describing a player and attached entities in transit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffTicket {
    pub ticket_id: String,
    pub player_id: u32,
    pub from_shard: ShardId,
    pub to_shard: ShardId,
    pub entities: Vec<u32>,
    pub phase: HandoffPhase,
}

/// An epoch-fenced ownership lease over an in-game entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipLease {
    pub entity_id: u32,
    pub owner_shard: ShardId,
    pub epoch: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ShardingError {
    #[error("shard {0} not registered")]
    ShardNotFound(ShardId),
    #[error("handoff ticket '{0}' not found")]
    TicketNotFound(String),
    #[error("invalid handoff state transition from {from:?} to {to:?}")]
    InvalidTransition { from: HandoffPhase, to: HandoffPhase },
    #[error("entity {0} lease conflict with shard {1}")]
    LeaseConflict(u32, ShardId),
}

/// Shard coordinator managing spatial topology, player transfers, and entity leases.
pub struct ShardCoordinator {
    regions: Vec<ShardRegion>,
    tickets: HashMap<String, HandoffTicket>,
    leases: HashMap<u32, OwnershipLease>,
    current_epoch: u64,
}

impl ShardCoordinator {
    pub fn new() -> Self {
        Self { regions: Vec::new(), tickets: HashMap::new(), leases: HashMap::new(), current_epoch: 1 }
    }

    pub fn register_shard(&mut self, region: ShardRegion) {
        self.regions.push(region);
    }

    pub fn locate_shard(&self, x: f32, y: f32) -> Option<ShardId> {
        self.regions.iter().find(|r| r.contains(x, y)).map(|r| r.shard_id)
    }

    /// Initiate an atomic two-phase player handoff.
    pub fn initiate_handoff(
        &mut self,
        ticket_id: impl Into<String>,
        player_id: u32,
        from_shard: ShardId,
        to_shard: ShardId,
        entities: Vec<u32>,
    ) -> Result<HandoffTicket, ShardingError> {
        if !self.regions.iter().any(|r| r.shard_id == from_shard) {
            return Err(ShardingError::ShardNotFound(from_shard));
        }
        if !self.regions.iter().any(|r| r.shard_id == to_shard) {
            return Err(ShardingError::ShardNotFound(to_shard));
        }

        let tid = ticket_id.into();
        let ticket = HandoffTicket {
            ticket_id: tid.clone(),
            player_id,
            from_shard,
            to_shard,
            entities,
            phase: HandoffPhase::Initiated,
        };

        self.tickets.insert(tid, ticket.clone());
        Ok(ticket)
    }

    /// Phase 1: Prepare handoff by acquiring or verifying entity lease transfer.
    pub fn prepare_handoff(&mut self, ticket_id: &str, now_ms: u64) -> Result<(), ShardingError> {
        let ticket =
            self.tickets.get_mut(ticket_id).ok_or_else(|| ShardingError::TicketNotFound(ticket_id.to_string()))?;

        if ticket.phase != HandoffPhase::Initiated {
            return Err(ShardingError::InvalidTransition { from: ticket.phase.clone(), to: HandoffPhase::Prepared });
        }

        // Verify entity leases belong to source shard
        for &ent in &ticket.entities {
            if let Some(lease) = self.leases.get(&ent) {
                if lease.owner_shard != ticket.from_shard && lease.expires_at_ms > now_ms {
                    return Err(ShardingError::LeaseConflict(ent, lease.owner_shard));
                }
            }
        }

        ticket.phase = HandoffPhase::Prepared;
        Ok(())
    }

    /// Phase 2: Commit handoff, atomically updating entity ownership to target shard.
    pub fn commit_handoff(&mut self, ticket_id: &str, lease_ttl_ms: u64, now_ms: u64) -> Result<(), ShardingError> {
        let ticket =
            self.tickets.get_mut(ticket_id).ok_or_else(|| ShardingError::TicketNotFound(ticket_id.to_string()))?;

        if ticket.phase != HandoffPhase::Prepared {
            return Err(ShardingError::InvalidTransition { from: ticket.phase.clone(), to: HandoffPhase::Committed });
        }

        self.current_epoch += 1;
        let epoch = self.current_epoch;
        let to_shard = ticket.to_shard;
        let expires_at_ms = now_ms + lease_ttl_ms;

        for &ent in &ticket.entities {
            self.leases.insert(ent, OwnershipLease { entity_id: ent, owner_shard: to_shard, epoch, expires_at_ms });
        }

        ticket.phase = HandoffPhase::Committed;
        Ok(())
    }

    /// Abort a prepared or initiated handoff.
    pub fn abort_handoff(&mut self, ticket_id: &str, reason: impl Into<String>) -> Result<(), ShardingError> {
        let ticket =
            self.tickets.get_mut(ticket_id).ok_or_else(|| ShardingError::TicketNotFound(ticket_id.to_string()))?;
        ticket.phase = HandoffPhase::Aborted { reason: reason.into() };
        Ok(())
    }

    /// Explicitly acquire an entity lease for a shard.
    pub fn acquire_lease(
        &mut self,
        entity_id: u32,
        shard_id: ShardId,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<OwnershipLease, ShardingError> {
        if let Some(existing) = self.leases.get(&entity_id) {
            if existing.owner_shard != shard_id && existing.expires_at_ms > now_ms {
                return Err(ShardingError::LeaseConflict(entity_id, existing.owner_shard));
            }
        }

        self.current_epoch += 1;
        let lease = OwnershipLease {
            entity_id,
            owner_shard: shard_id,
            epoch: self.current_epoch,
            expires_at_ms: now_ms + ttl_ms,
        };

        self.leases.insert(entity_id, lease.clone());
        Ok(lease)
    }

    pub fn get_lease(&self, entity_id: u32) -> Option<&OwnershipLease> {
        self.leases.get(&entity_id)
    }

    pub fn reclaim_expired_leases(&mut self, now_ms: u64) -> usize {
        let before = self.leases.len();
        self.leases.retain(|_, lease| lease.expires_at_ms > now_ms);
        before - self.leases.len()
    }
}

impl Default for ShardCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_coordinator() -> ShardCoordinator {
        let mut coord = ShardCoordinator::new();
        // Shard 1: South Los Santos (-2000..2000 X, -4000..0 Y)
        coord.register_shard(ShardRegion { shard_id: 1, min_x: -2000.0, max_x: 2000.0, min_y: -4000.0, max_y: 0.0 });
        // Shard 2: North Los Santos / Blaine County (-2000..2000 X, 0..5000 Y)
        coord.register_shard(ShardRegion { shard_id: 2, min_x: -2000.0, max_x: 2000.0, min_y: 0.0, max_y: 5000.0 });
        coord
    }

    #[test]
    fn spatial_grid_maps_coordinates_to_correct_shard() {
        let coord = setup_test_coordinator();

        assert_eq!(coord.locate_shard(0.0, -1000.0), Some(1));
        assert_eq!(coord.locate_shard(0.0, 2500.0), Some(2));
        assert_eq!(coord.locate_shard(9999.0, 9999.0), None);
    }

    #[test]
    fn handoff_state_machine_happy_path_commit() {
        let mut coord = setup_test_coordinator();
        let ticket = coord.initiate_handoff("t-1", 100, 1, 2, vec![501, 502]).unwrap();
        assert_eq!(ticket.phase, HandoffPhase::Initiated);

        coord.prepare_handoff("t-1", 1000).unwrap();
        coord.commit_handoff("t-1", 30_000, 1000).unwrap();

        let l1 = coord.get_lease(501).unwrap();
        assert_eq!(l1.owner_shard, 2);
        assert_eq!(l1.expires_at_ms, 31_000);
    }

    #[test]
    fn handoff_rollback_when_aborted() {
        let mut coord = setup_test_coordinator();
        coord.initiate_handoff("t-2", 101, 1, 2, vec![601]).unwrap();
        coord.prepare_handoff("t-2", 1000).unwrap();

        coord.abort_handoff("t-2", "Target shard congested").unwrap();
        assert_eq!(coord.get_lease(601), None);
    }

    #[test]
    fn distributed_ownership_lease_grant_and_renewal() {
        let mut coord = setup_test_coordinator();
        let l1 = coord.acquire_lease(700, 1, 10_000, 1000).unwrap();
        assert_eq!(l1.owner_shard, 1);
        assert_eq!(l1.expires_at_ms, 11_000);

        // Same shard can renew lease
        let l2 = coord.acquire_lease(700, 1, 10_000, 5000).unwrap();
        assert_eq!(l2.expires_at_ms, 15_000);
        assert!(l2.epoch > l1.epoch);
    }

    #[test]
    fn lease_conflict_prevents_unauthorized_steal() {
        let mut coord = setup_test_coordinator();
        coord.acquire_lease(800, 1, 10_000, 1000).unwrap();

        // Shard 2 attempts to claim unexpired entity from Shard 1
        let res = coord.acquire_lease(800, 2, 10_000, 2000);
        assert_eq!(res, Err(ShardingError::LeaseConflict(800, 1)));
    }

    #[test]
    fn stale_lease_expires_and_is_reclaimed() {
        let mut coord = setup_test_coordinator();
        coord.acquire_lease(900, 1, 5000, 1000).unwrap();

        // At timestamp 7000, lease has expired
        let reclaimed = coord.reclaim_expired_leases(7000);
        assert_eq!(reclaimed, 1);
        assert_eq!(coord.get_lease(900), None);
    }

    #[test]
    fn unregistered_shard_handoff_fails() {
        let mut coord = setup_test_coordinator();
        let res = coord.initiate_handoff("t-err", 10, 1, 999, vec![]);
        assert_eq!(res, Err(ShardingError::ShardNotFound(999)));
    }

    #[test]
    fn commit_before_prepare_is_rejected() {
        let mut coord = setup_test_coordinator();
        coord.initiate_handoff("t-seq", 10, 1, 2, vec![]).unwrap();

        let res = coord.commit_handoff("t-seq", 5000, 1000);
        assert!(matches!(res, Err(ShardingError::InvalidTransition { .. })));
    }

    #[test]
    fn expired_lease_can_be_claimed_by_new_shard() {
        let mut coord = setup_test_coordinator();
        coord.acquire_lease(1001, 1, 2000, 1000).unwrap();

        // After expiry at 3000ms, Shard 2 can claim it
        let l = coord.acquire_lease(1001, 2, 5000, 4000).unwrap();
        assert_eq!(l.owner_shard, 2);
    }

    #[test]
    fn multiple_shards_coexist() {
        let mut coord = setup_test_coordinator();
        coord.acquire_lease(2001, 1, 5000, 1000).unwrap();
        coord.acquire_lease(2002, 2, 5000, 1000).unwrap();

        assert_eq!(coord.get_lease(2001).unwrap().owner_shard, 1);
        assert_eq!(coord.get_lease(2002).unwrap().owner_shard, 2);
    }
}
