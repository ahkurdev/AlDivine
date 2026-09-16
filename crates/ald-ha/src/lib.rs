//! Aldivine Central Service High Availability & Fault Tolerance
//!
//! Provides circuit breakers, automatic endpoint failover, offline grace mode,
//! and split-brain safe leader election for central services and edge nodes.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// State of a circuit breaker protecting an external service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitState {
    Closed,
    Open { tripped_at_ms: u64 },
    HalfOpen,
}

/// Circuit breaker mitigating cascading network and service failures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreaker {
    pub state: CircuitState,
    pub failure_threshold: u32,
    pub cooldown_ms: u64,
    pub consecutive_failures: u32,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, cooldown_ms: u64) -> Self {
        Self { state: CircuitState::Closed, failure_threshold, cooldown_ms, consecutive_failures: 0 }
    }

    pub fn can_execute(&mut self, now_ms: u64) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open { tripped_at_ms } => {
                if now_ms.saturating_sub(tripped_at_ms) >= self.cooldown_ms {
                    self.state = CircuitState::HalfOpen;
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.state = CircuitState::Closed;
    }

    pub fn record_failure(&mut self, now_ms: u64) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.failure_threshold {
            self.state = CircuitState::Open { tripped_at_ms: now_ms };
        }
    }
}

/// Replicated service endpoint with priority failover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub url: String,
    pub priority: u32,
    pub healthy: bool,
}

/// Endpoint failover pool selecting the best available replica.
#[derive(Debug, Clone, Default)]
pub struct FailoverPool {
    endpoints: Vec<ServiceEndpoint>,
}

impl FailoverPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_endpoint(&mut self, url: impl Into<String>, priority: u32) {
        self.endpoints.push(ServiceEndpoint { url: url.into(), priority, healthy: true });
        self.endpoints.sort_by_key(|e| e.priority);
    }

    pub fn mark_healthy(&mut self, url: &str, healthy: bool) {
        if let Some(e) = self.endpoints.iter_mut().find(|e| e.url == url) {
            e.healthy = healthy;
        }
    }

    pub fn get_active_endpoint(&self) -> Option<&ServiceEndpoint> {
        self.endpoints.iter().find(|e| e.healthy)
    }
}

/// Offline tolerance policy allowing gameplay during central outages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineGracePolicy {
    pub grace_started_at_ms: u64,
    pub grace_duration_ms: u64,
}

impl OfflineGracePolicy {
    pub fn new(now_ms: u64, duration_ms: u64) -> Self {
        Self { grace_started_at_ms: now_ms, grace_duration_ms: duration_ms }
    }

    pub fn is_within_grace(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.grace_started_at_ms) <= self.grace_duration_ms
    }

    pub fn remaining_grace_ms(&self, now_ms: u64) -> u64 {
        let elapsed = now_ms.saturating_sub(self.grace_started_at_ms);
        self.grace_duration_ms.saturating_sub(elapsed)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ElectionError {
    #[error("stale election epoch: current {current}, provided {provided}")]
    StaleEpoch { current: u64, provided: u64 },
    #[error("heartbeat timeout not reached; leader '{leader}' is still active")]
    LeaderStillActive { leader: String },
}

/// Split-brain safe leader election coordinator.
#[derive(Debug, Clone)]
pub struct LeaderCoordinator {
    pub current_epoch: u64,
    pub current_leader: Option<String>,
    pub last_heartbeat_ms: u64,
    pub heartbeat_timeout_ms: u64,
}

impl LeaderCoordinator {
    pub fn new(heartbeat_timeout_ms: u64) -> Self {
        Self { current_epoch: 0, current_leader: None, last_heartbeat_ms: 0, heartbeat_timeout_ms }
    }

    pub fn record_heartbeat(&mut self, leader_id: &str, epoch: u64, now_ms: u64) -> Result<(), ElectionError> {
        if epoch < self.current_epoch {
            return Err(ElectionError::StaleEpoch { current: self.current_epoch, provided: epoch });
        }
        self.current_epoch = epoch;
        self.current_leader = Some(leader_id.to_string());
        self.last_heartbeat_ms = now_ms;
        Ok(())
    }

    pub fn attempt_claim_leadership(&mut self, candidate_id: &str, now_ms: u64) -> Result<u64, ElectionError> {
        let is_timed_out = now_ms.saturating_sub(self.last_heartbeat_ms) > self.heartbeat_timeout_ms;
        if let Some(ref leader) = self.current_leader {
            if !is_timed_out {
                return Err(ElectionError::LeaderStillActive { leader: leader.clone() });
            }
        }

        self.current_epoch += 1;
        self.current_leader = Some(candidate_id.to_string());
        self.last_heartbeat_ms = now_ms;
        Ok(self.current_epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_breaker_trips_on_consecutive_failures() {
        let mut cb = CircuitBreaker::new(3, 5000);
        assert!(cb.can_execute(1000));

        cb.record_failure(1100);
        cb.record_failure(1200);
        assert!(cb.can_execute(1250));

        // 3rd failure trips
        cb.record_failure(1300);
        assert!(!cb.can_execute(1350));
        assert_eq!(cb.state, CircuitState::Open { tripped_at_ms: 1300 });
    }

    #[test]
    fn circuit_breaker_recovers_to_half_open_after_cooldown() {
        let mut cb = CircuitBreaker::new(2, 5000);
        cb.record_failure(1000);
        cb.record_failure(1100);
        assert!(!cb.can_execute(2000));

        // After cooldown (1100 + 5000 = 6100), can execute and enters HalfOpen
        assert!(cb.can_execute(6200));
        assert_eq!(cb.state, CircuitState::HalfOpen);
    }

    #[test]
    fn circuit_breaker_resets_to_closed_on_success() {
        let mut cb = CircuitBreaker::new(2, 5000);
        cb.record_failure(1000);
        cb.record_failure(1100);
        cb.can_execute(7000); // Transitions to HalfOpen

        cb.record_success();
        assert_eq!(cb.state, CircuitState::Closed);
        assert_eq!(cb.consecutive_failures, 0);
    }

    #[test]
    fn endpoint_failover_rotates_to_healthy_replica() {
        let mut pool = FailoverPool::new();
        pool.add_endpoint("https://primary.aldivine.com", 1);
        pool.add_endpoint("https://secondary.aldivine.com", 2);

        assert_eq!(pool.get_active_endpoint().unwrap().url, "https://primary.aldivine.com");

        // Fail primary
        pool.mark_healthy("https://primary.aldivine.com", false);
        assert_eq!(pool.get_active_endpoint().unwrap().url, "https://secondary.aldivine.com");

        // Restore primary
        pool.mark_healthy("https://primary.aldivine.com", true);
        assert_eq!(pool.get_active_endpoint().unwrap().url, "https://primary.aldivine.com");
    }

    #[test]
    fn offline_grace_mode_permits_cached_entitlements() {
        // 24 hour grace
        let policy = OfflineGracePolicy::new(1_000_000, 86_400_000);
        assert!(policy.is_within_grace(1_000_000 + 3_600_000)); // 1 hour later
        assert!(policy.remaining_grace_ms(1_000_000 + 3_600_000) > 0);
    }

    #[test]
    fn offline_grace_mode_expires_after_ttl() {
        let policy = OfflineGracePolicy::new(1000, 5000);
        assert!(policy.is_within_grace(5000));
        assert!(!policy.is_within_grace(7000));
        assert_eq!(policy.remaining_grace_ms(7000), 0);
    }

    #[test]
    fn active_standby_heartbeat_detects_leader_failure() {
        let mut coord = LeaderCoordinator::new(3000);
        coord.record_heartbeat("node-primary", 1, 1000).unwrap();

        // Secondary fails to claim while primary is healthy
        let res = coord.attempt_claim_leadership("node-secondary", 2000);
        assert!(matches!(res, Err(ElectionError::LeaderStillActive { .. })));

        // Heartbeat timeout occurs past 4000ms
        let new_epoch = coord.attempt_claim_leadership("node-secondary", 5000).unwrap();
        assert_eq!(new_epoch, 2);
        assert_eq!(coord.current_leader, Some("node-secondary".to_string()));
    }

    #[test]
    fn epoch_increment_prevents_stale_leader_split_brain() {
        let mut coord = LeaderCoordinator::new(3000);
        coord.record_heartbeat("node-primary", 2, 1000).unwrap();

        // Old primary attempts heartbeat with stale epoch 1
        let res = coord.record_heartbeat("node-primary", 1, 1500);
        assert!(matches!(res, Err(ElectionError::StaleEpoch { .. })));
    }

    #[test]
    fn failover_pool_empty_handles_gracefully() {
        let pool = FailoverPool::new();
        assert_eq!(pool.get_active_endpoint(), None);
    }

    #[test]
    fn failover_pool_all_unhealthy_returns_none() {
        let mut pool = FailoverPool::new();
        pool.add_endpoint("https://node-1.aldivine.com", 1);
        pool.mark_healthy("https://node-1.aldivine.com", false);
        assert_eq!(pool.get_active_endpoint(), None);
    }
}
