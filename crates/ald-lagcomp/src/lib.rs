//! Lag compensation: rewind hit-resolution against tick snapshots.
//!
//! The server is authoritative over time ([`ald_timesync`] clocks and claim
//! bounds decide which client timestamps are even believable). This crate
//! answers the next question: "the shooter fired at their tick T — where
//! was the target at T, and did the shot land?"
//!
//! - [`HistoryBuffer`] keeps a bounded ring of per-entity positions per
//!   tick. `at()` returns the exact sample, lerps between bracketing
//!   samples, or `None` when T is too old (pruned), too new (from the
//!   future), or the entity is unknown. No guessing across gaps.
//! - [`rewind_allowed`] gates the claim in the tick domain: T must not be
//!   from the future and must be within `max_rewind_ticks` of the server
//!   tick. Saturating math throughout — tick wrap is not a thing here
//!   (u64 monotonic process ticks), but future claims always fail.
//! - [`hit_landed`] compares the rewound target position against the
//!   shooter's aimed point within a radius, on the horizontal plane — the
//!   same convention as `ald-ecs` interest, so visibility and hits agree on
//!   what "near" means.
//!
//! What this crate does NOT do: wall-clock conversion (callers map validated
//! client time to ticks), damage application, or anti-cheat verdicts. It
//! resolves geometry; policy decides what a landed hit means.

use std::collections::{BTreeMap, HashMap};

use ald_ecs::Vec3;
use thiserror::Error;

/// Ticks of history retained per entity.
pub const DEFAULT_HISTORY_TICKS: usize = 64;

/// Why a rewind/hit query failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LagError {
    #[error("claim tick {claim} is newer than server tick {server}")]
    FromFuture { claim: u64, server: u64 },
    #[error("claim tick {claim} is {age} ticks behind server tick {server} (max rewind {max})")]
    TooOld { claim: u64, server: u64, age: u64, max: u64 },
    #[error("no position for entity {0} at tick {1}")]
    NoSample(u64, u64),
}

/// Tick-ring position history per entity.
#[derive(Debug, Default)]
pub struct HistoryBuffer {
    capacity_ticks: usize,
    samples: HashMap<u64, BTreeMap<u64, Vec3>>,
}

impl HistoryBuffer {
    pub fn new(capacity_ticks: usize) -> Self {
        HistoryBuffer { capacity_ticks: capacity_ticks.max(2), samples: HashMap::new() }
    }

    /// Record one entity's position at one tick. Out-of-order and duplicate
    /// ticks are accepted (latest write wins); pruning keeps the newest
    /// `capacity_ticks` ticks per entity.
    pub fn record(&mut self, entity: u64, tick: u64, pos: Vec3) {
        let entry = self.samples.entry(entity).or_default();
        entry.insert(tick, pos);
        while entry.len() > self.capacity_ticks {
            entry.pop_first();
        }
    }

    /// Position at exactly `tick`: sample, lerp between brackets, or `None`.
    pub fn at(&self, entity: u64, tick: u64) -> Option<Vec3> {
        let entry = self.samples.get(&entity)?;
        if let Some(pos) = entry.get(&tick) {
            return Some(*pos);
        }
        let before = entry.range(..tick).next_back()?;
        let after = entry.range(tick..).next()?;
        let (t0, p0) = (*before.0, *before.1);
        let (t1, p1) = (*after.0, *after.1);
        if t1 == t0 {
            return Some(p0);
        }
        let f = (tick - t0) as f32 / (t1 - t0) as f32;
        Some(Vec3::new(p0.x + (p1.x - p0.x) * f, p0.y + (p1.y - p0.y) * f, p0.z + (p1.z - p0.z) * f))
    }

    /// Forget an entity entirely (despawn cleanup). Returns whether tracked.
    pub fn forget(&mut self, entity: u64) -> bool {
        self.samples.remove(&entity).is_some()
    }

    pub fn tracked_entities(&self) -> usize {
        self.samples.len()
    }
}

/// Gate a rewind claim: `claim_tick` must satisfy
/// `server_tick - max_rewind <= claim <= server_tick`.
pub fn rewind_allowed(server_tick: u64, claim_tick: u64, max_rewind_ticks: u64) -> Result<(), LagError> {
    if claim_tick > server_tick {
        return Err(LagError::FromFuture { claim: claim_tick, server: server_tick });
    }
    let age = server_tick - claim_tick;
    if age > max_rewind_ticks {
        return Err(LagError::TooOld { claim: claim_tick, server: server_tick, age, max: max_rewind_ticks });
    }
    Ok(())
}

/// Did a shot aimed at `aimed` land on a target whose rewound position is
/// `target`, within `radius` (horizontal plane)?
pub fn hit_landed(aimed: Vec3, target: Vec3, radius: f32) -> bool {
    aimed.horizontal_distance(target) <= radius.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec3 {
        Vec3::new(x, y, 0.0)
    }

    #[test]
    fn exact_sample_returned() {
        let mut h = HistoryBuffer::new(8);
        h.record(1, 100, v(1.0, 2.0));
        assert_eq!(h.at(1, 100), Some(v(1.0, 2.0)));
    }

    #[test]
    fn midpoints_lerp() {
        let mut h = HistoryBuffer::new(8);
        h.record(1, 100, v(0.0, 0.0));
        h.record(1, 110, v(10.0, 20.0));
        let mid = h.at(1, 105).unwrap();
        assert!((mid.x - 5.0).abs() < 1e-5 && (mid.y - 10.0).abs() < 1e-5);
        // Quarter point.
        let q = h.at(1, 102).unwrap();
        assert!((q.x - 2.0).abs() < 1e-4);
    }

    #[test]
    fn gaps_report_none() {
        let mut h = HistoryBuffer::new(8);
        assert_eq!(h.at(99, 100), None); // unknown entity
        h.record(1, 100, v(0.0, 0.0));
        assert_eq!(h.at(1, 90), None); // before first sample
        assert_eq!(h.at(1, 110), None); // after last sample: no extrapolation
        h.record(1, 200, v(5.0, 5.0));
        assert!(h.at(1, 150).is_some()); // bracketed: fine
    }

    #[test]
    fn ring_prunes_oldest() {
        let mut h = HistoryBuffer::new(4);
        for t in 100..110 {
            h.record(1, t, v(t as f32, 0.0));
        }
        assert_eq!(h.at(1, 105), None); // pruned
        assert!(h.at(1, 106).is_some());
        assert!(h.at(1, 109).is_some());
    }

    #[test]
    fn entities_isolated_and_forgettable() {
        let mut h = HistoryBuffer::new(8);
        h.record(1, 100, v(1.0, 1.0));
        h.record(2, 100, v(9.0, 9.0));
        assert_eq!(h.tracked_entities(), 2);
        assert!(h.forget(1));
        assert!(!h.forget(1));
        assert_eq!(h.at(1, 100), None);
        assert_eq!(h.at(2, 100), Some(v(9.0, 9.0)));
    }

    #[test]
    fn rewind_bounds() {
        assert!(rewind_allowed(1000, 1000, 30).is_ok());
        assert!(rewind_allowed(1000, 970, 30).is_ok());
        assert_eq!(rewind_allowed(1000, 969, 30), Err(LagError::TooOld { claim: 969, server: 1000, age: 31, max: 30 }));
        assert_eq!(rewind_allowed(1000, 1001, 30), Err(LagError::FromFuture { claim: 1001, server: 1000 }));
        // Zero rewind budget: only the current tick.
        assert!(rewind_allowed(1000, 1000, 0).is_ok());
        assert!(rewind_allowed(1000, 999, 0).is_err());
    }

    #[test]
    fn hit_radius_boundary() {
        let target = v(3.0, 4.0); // 5.0 from origin
        assert!(hit_landed(v(0.0, 0.0), target, 5.0));
        assert!(!hit_landed(v(0.0, 0.0), target, 4.99));
        // Negative radius clamps to 0: exact overlap still lands.
        assert!(hit_landed(target, target, -2.0));
    }

    #[test]
    fn full_resolve_path() {
        // Shooter claims tick 100 while server is at 105; target moved.
        let mut h = HistoryBuffer::new(16);
        h.record(9, 98, v(0.0, 0.0));
        h.record(9, 102, v(8.0, 0.0));
        h.record(9, 105, v(20.0, 0.0)); // current: far away
        rewind_allowed(105, 100, 10).unwrap();
        let rewound = h.at(9, 100).unwrap();
        assert!((rewound.x - 4.0).abs() < 1e-5);
        // Aimed at the rewound spot: lands. Aimed at current: misses.
        assert!(hit_landed(v(4.0, 0.0), rewound, 1.0));
        assert!(!hit_landed(v(20.0, 0.0), rewound, 1.0));
    }
}
