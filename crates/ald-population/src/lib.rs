//! Server-side population manager: density policy, budgets, creation hooks.
//!
//! Ambient peds and vehicles cost replication bandwidth, server CPU, and
//! client frames. This manager keeps each dimension inside its configured
//! budget: spawns are allowed while headroom remains and denied at the cap,
//! with hooks so policy, audit, and gameplay systems observe every decision.
//!
//! Deterministic by design: density is a steady-state fraction cap
//! (`floor(max * density)`), not a dice roll — the same request sequence
//! always yields the same decisions, which is what makes this testable.
//! Stochastic thinning (which individual ambient to cull) happens in the
//! game bridge against live world state, not here.
//!
//! Fail-closed defaults: a dimension with no configured policy denies all
//! spawns (`NoPolicy`). Budgets are per dimension — dimension 5's traffic
//! never eats dimension 0's headroom.
//!
//! What this crate does NOT do: actual spawning, GTA handles, renderer
//! budgets, or client culling. It answers "may this spawn?" and accounts
//! the answer. Enforcement against liars (a bridge spawning without asking)
//! belongs to the bridge's reconciliation pass.

use std::collections::HashMap;

use ald_ecs::DimensionId;
use thiserror::Error;

/// Ambient population kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopulationKind {
    Ped,
    Vehicle,
}

/// Budget for one dimension.
#[derive(Debug, Clone, PartialEq)]
pub struct DensityPolicy {
    pub max_peds: usize,
    pub max_vehicles: usize,
    /// Steady-state fraction of each cap ambients may fill (0.0–1.0).
    /// `1.0` = fill to the cap; `0.0` = no ambients.
    pub ped_density: f32,
    pub vehicle_density: f32,
}

impl DensityPolicy {
    pub fn new(
        max_peds: usize,
        max_vehicles: usize,
        ped_density: f32,
        vehicle_density: f32,
    ) -> Result<Self, PolicyError> {
        for d in [ped_density, vehicle_density] {
            if !(0.0..=1.0).contains(&d) || d.is_nan() {
                return Err(PolicyError::BadDensity(d));
            }
        }
        Ok(DensityPolicy { max_peds, max_vehicles, ped_density, vehicle_density })
    }

    /// No ambients of any kind.
    pub fn closed() -> Self {
        DensityPolicy { max_peds: 0, max_vehicles: 0, ped_density: 0.0, vehicle_density: 0.0 }
    }

    fn cap(&self, kind: PopulationKind) -> usize {
        let (max, density) = match kind {
            PopulationKind::Ped => (self.max_peds, self.ped_density),
            PopulationKind::Vehicle => (self.max_vehicles, self.vehicle_density),
        };
        ((max as f32) * density).floor() as usize
    }
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum PolicyError {
    #[error("density {0} outside 0.0–1.0")]
    BadDensity(f32),
}

/// Why a spawn was denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    /// No policy configured for this dimension.
    NoPolicy,
    /// Dimension at its density-gated cap for this kind.
    AtCap { cap: usize },
}

/// Outcome of [`PopulationManager::request_spawn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnDecision {
    Allow,
    Deny { reason: DenyReason },
}

/// A spawn/deny observation for hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnEvent {
    pub kind: PopulationKind,
    pub dimension: DimensionId,
    pub decision: SpawnDecision,
    /// Live count of this kind in this dimension after the decision.
    pub count_after: usize,
}

/// A spawn-decision observer.
pub type DecisionHook = Box<dyn FnMut(&SpawnEvent)>;

/// Budget enforcement per dimension with creation hooks.
pub struct PopulationManager {
    policies: HashMap<DimensionId, DensityPolicy>,
    counts: HashMap<(DimensionId, PopulationKind), usize>,
    hooks: Vec<DecisionHook>,
}

impl PopulationManager {
    pub fn new() -> Self {
        PopulationManager { policies: HashMap::new(), counts: HashMap::new(), hooks: Vec::new() }
    }

    /// Set (or replace) a dimension's policy. Counts are not retroactively
    /// culled — over-cap dimensions deny new spawns until releases drain
    /// them, which the tests prove.
    pub fn set_policy(&mut self, dimension: DimensionId, policy: DensityPolicy) {
        self.policies.insert(dimension, policy);
    }

    /// Remove a dimension's policy. Outstanding counts stay (released
    /// normally); new spawns deny with `NoPolicy`.
    pub fn clear_policy(&mut self, dimension: DimensionId) -> bool {
        self.policies.remove(&dimension).is_some()
    }

    /// Subscribe to every spawn decision. Hooks fire in subscription order.
    pub fn on_decision(&mut self, hook: DecisionHook) {
        self.hooks.push(hook);
    }

    /// Ask to spawn one ambient. Allowed spawns increment the count.
    pub fn request_spawn(&mut self, kind: PopulationKind, dimension: DimensionId) -> SpawnDecision {
        let decision = match self.policies.get(&dimension) {
            None => SpawnDecision::Deny { reason: DenyReason::NoPolicy },
            Some(policy) => {
                let cap = policy.cap(kind);
                let count = self.count_of(kind, dimension);
                if count < cap {
                    SpawnDecision::Allow
                } else {
                    SpawnDecision::Deny { reason: DenyReason::AtCap { cap } }
                }
            }
        };
        if decision == SpawnDecision::Allow {
            *self.counts.entry((dimension, kind)).or_insert(0) += 1;
        }
        let event = SpawnEvent { kind, dimension, decision, count_after: self.count_of(kind, dimension) };
        for hook in &mut self.hooks {
            hook(&event);
        }
        decision
    }

    /// Release one ambient (despawn/culled/migrated away). Saturates at
    /// zero: double-release reports `false` instead of underflowing.
    pub fn release(&mut self, kind: PopulationKind, dimension: DimensionId) -> bool {
        match self.counts.get_mut(&(dimension, kind)) {
            Some(c) if *c > 0 => {
                *c -= 1;
                true
            }
            _ => false,
        }
    }

    pub fn count_of(&self, kind: PopulationKind, dimension: DimensionId) -> usize {
        self.counts.get(&(dimension, kind)).copied().unwrap_or(0)
    }

    pub fn policy_of(&self, dimension: DimensionId) -> Option<&DensityPolicy> {
        self.policies.get(&dimension)
    }
}

impl Default for PopulationManager {
    fn default() -> Self {
        PopulationManager::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const D0: DimensionId = DimensionId::DEFAULT;
    const D5: DimensionId = DimensionId(5);

    fn policy() -> DensityPolicy {
        DensityPolicy::new(10, 4, 1.0, 0.5).unwrap()
    }

    #[test]
    fn bad_densities_rejected() {
        // NaN never equals itself, so assert rejection by shape, not value.
        for bad in [f32::NAN, -0.1, 1.1, f32::INFINITY] {
            assert!(matches!(DensityPolicy::new(10, 10, bad, 1.0), Err(PolicyError::BadDensity(_))), "{bad}");
            assert!(matches!(DensityPolicy::new(10, 10, 1.0, bad), Err(PolicyError::BadDensity(_))), "{bad}");
        }
        assert!(DensityPolicy::new(10, 10, 0.0, 1.0).is_ok());
    }

    #[test]
    fn unconfigured_dimension_denies() {
        let mut m = PopulationManager::new();
        assert_eq!(m.request_spawn(PopulationKind::Ped, D0), SpawnDecision::Deny { reason: DenyReason::NoPolicy });
        assert_eq!(m.count_of(PopulationKind::Ped, D0), 0);
    }

    #[test]
    fn caps_and_density_fractions_enforced() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, policy());
        // Peds: density 1.0 over 10 -> 10 allowed, 11th denied.
        for _ in 0..10 {
            assert_eq!(m.request_spawn(PopulationKind::Ped, D0), SpawnDecision::Allow);
        }
        assert_eq!(
            m.request_spawn(PopulationKind::Ped, D0),
            SpawnDecision::Deny { reason: DenyReason::AtCap { cap: 10 } }
        );
        // Vehicles: density 0.5 over 4 -> floor(2.0) = 2 allowed.
        for _ in 0..2 {
            assert_eq!(m.request_spawn(PopulationKind::Vehicle, D0), SpawnDecision::Allow);
        }
        assert_eq!(
            m.request_spawn(PopulationKind::Vehicle, D0),
            SpawnDecision::Deny { reason: DenyReason::AtCap { cap: 2 } }
        );
    }

    #[test]
    fn dimensions_isolated() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, policy());
        m.set_policy(D5, DensityPolicy::new(1, 1, 1.0, 1.0).unwrap());
        for _ in 0..10 {
            m.request_spawn(PopulationKind::Ped, D0);
        }
        // D0 full; D5 unaffected.
        assert_eq!(m.request_spawn(PopulationKind::Ped, D5), SpawnDecision::Allow);
        assert_eq!(m.count_of(PopulationKind::Ped, D0), 10);
        assert_eq!(m.count_of(PopulationKind::Ped, D5), 1);
    }

    #[test]
    fn release_drains_and_saturates() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, DensityPolicy::new(1, 0, 1.0, 0.0).unwrap());
        assert_eq!(m.request_spawn(PopulationKind::Ped, D0), SpawnDecision::Allow);
        assert_eq!(
            m.request_spawn(PopulationKind::Ped, D0),
            SpawnDecision::Deny { reason: DenyReason::AtCap { cap: 1 } }
        );
        assert!(m.release(PopulationKind::Ped, D0));
        assert_eq!(m.request_spawn(PopulationKind::Ped, D0), SpawnDecision::Allow);
        // Double release: second reports false, count stays 0... first
        // release after the new spawn consumes it, second saturates.
        assert!(m.release(PopulationKind::Ped, D0));
        assert!(!m.release(PopulationKind::Ped, D0));
        assert_eq!(m.count_of(PopulationKind::Ped, D0), 0);
        // Releasing what was never spawned is false, not a panic.
        assert!(!m.release(PopulationKind::Vehicle, D0));
    }

    #[test]
    fn tightened_policy_denies_until_drain() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, DensityPolicy::new(10, 0, 1.0, 0.0).unwrap());
        for _ in 0..10 {
            m.request_spawn(PopulationKind::Ped, D0);
        }
        // Tighten below the live count: no retroactive cull, new spawns deny.
        m.set_policy(D0, DensityPolicy::new(4, 0, 1.0, 0.0).unwrap());
        assert_eq!(m.count_of(PopulationKind::Ped, D0), 10);
        assert_eq!(
            m.request_spawn(PopulationKind::Ped, D0),
            SpawnDecision::Deny { reason: DenyReason::AtCap { cap: 4 } }
        );
        for _ in 0..7 {
            m.release(PopulationKind::Ped, D0);
        }
        assert_eq!(m.count_of(PopulationKind::Ped, D0), 3);
        assert_eq!(m.request_spawn(PopulationKind::Ped, D0), SpawnDecision::Allow);
    }

    #[test]
    fn hooks_observe_every_decision_in_order() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, DensityPolicy::new(1, 0, 1.0, 0.0).unwrap());
        let seen: Rc<RefCell<Vec<SpawnEvent>>> = Rc::new(RefCell::new(vec![]));
        let probe = Rc::clone(&seen);
        let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(vec![]));
        let order_p = Rc::clone(&order);
        m.on_decision(Box::new(move |e| probe.borrow_mut().push(*e)));
        m.on_decision(Box::new(move |_| order_p.borrow_mut().push("second")));
        m.request_spawn(PopulationKind::Ped, D0);
        m.request_spawn(PopulationKind::Ped, D0);
        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].decision, SpawnDecision::Allow);
        assert_eq!(seen[0].count_after, 1);
        assert_eq!(seen[1].decision, SpawnDecision::Deny { reason: DenyReason::AtCap { cap: 1 } });
        assert_eq!(seen[1].count_after, 1);
        assert_eq!(*order.borrow(), vec!["second", "second"]);
    }

    #[test]
    fn closed_policy_denies_everything() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, DensityPolicy::closed());
        assert_eq!(
            m.request_spawn(PopulationKind::Ped, D0),
            SpawnDecision::Deny { reason: DenyReason::AtCap { cap: 0 } }
        );
    }

    #[test]
    fn clear_policy_releases_to_deny() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, policy());
        m.request_spawn(PopulationKind::Ped, D0);
        assert!(m.clear_policy(D0));
        assert!(!m.clear_policy(D0));
        assert_eq!(m.request_spawn(PopulationKind::Ped, D0), SpawnDecision::Deny { reason: DenyReason::NoPolicy });
        // Outstanding count still releasable.
        assert!(m.release(PopulationKind::Ped, D0));
    }

    #[test]
    fn zero_density_means_zero_cap() {
        let mut m = PopulationManager::new();
        m.set_policy(D0, DensityPolicy::new(100, 100, 0.0, 0.0).unwrap());
        assert_eq!(
            m.request_spawn(PopulationKind::Ped, D0),
            SpawnDecision::Deny { reason: DenyReason::AtCap { cap: 0 } }
        );
    }
}
