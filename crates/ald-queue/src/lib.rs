//! Connection admission queue: playing slots, waitlist, reserved slots.
//!
//! Pure logic, no I/O and no clock: the caller passes `now_ms` so behavior is
//! deterministic and tests use fake time. The server passes its real clock.
//!
//! Model:
//! - `max_players` playing slots. `reserved_slots` of them (saturating) may
//!   only be occupied by reservation holders (staff, VIP, priority queue
//!   graduates). Everyone else is capped at `max_players - reserved_slots`.
//! - A player with no free slot joins the FIFO waitlist (bounded by
//!   `max_queue_len`) instead of being rejected, unless the waitlist itself
//!   is full — or zero-length, in which case the server is direct-or-reject.
//! - When a playing slot frees, the waitlist is scanned front-to-back and
//!   the first entry that fits the reservation rule is promoted. FIFO order
//!   holds except where a reservation rule blocks the head.
//! - Waitlist entries go stale after `stale_after_ms` (0 disables) and are
//!   dropped by [`Queue::expire_stale`]. Stale players rejoin at the back.
//!
//! What this crate does NOT do: networking, identity checks, or ban checks.
//! Those are deferral gates ([`ald_deferrals`]) and the login pipeline. This
//! crate answers only "is there room, and where do you wait".

use std::collections::VecDeque;

use thiserror::Error;

/// Admission queue errors (configuration only — joins report outcomes, not errors).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QueueError {
    #[error("reserved_slots ({reserved}) exceeds max_players ({max})")]
    TooManyReserved { max: usize, reserved: usize },
}

/// Queue sizing and expiry policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueConfig {
    /// Total playing slots.
    pub max_players: usize,
    /// Playing slots only reservation holders may occupy. Must be <= max.
    pub reserved_slots: usize,
    /// Waitlist bound. 0 means direct-or-reject (no waiting).
    pub max_queue_len: usize,
    /// Queued entries older than this (ms) are stale. 0 disables expiry.
    pub stale_after_ms: u64,
}

impl QueueConfig {
    pub fn new(
        max_players: usize,
        reserved_slots: usize,
        max_queue_len: usize,
        stale_after_ms: u64,
    ) -> Result<Self, QueueError> {
        if reserved_slots > max_players {
            return Err(QueueError::TooManyReserved { max: max_players, reserved: reserved_slots });
        }
        Ok(QueueConfig { max_players, reserved_slots, max_queue_len, stale_after_ms })
    }

    /// Playing slots a non-holder may occupy.
    pub fn open_slots(&self) -> usize {
        self.max_players.saturating_sub(self.reserved_slots)
    }
}

/// A join attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    /// Stable player identifier (Aldivine player id string).
    pub id: String,
    /// Whether this player holds a reservation (staff / VIP / priority).
    pub holds_reservation: bool,
}

impl JoinRequest {
    pub fn new(id: &str, holds_reservation: bool) -> Self {
        JoinRequest { id: id.to_string(), holds_reservation }
    }
}

/// Why a join was refused outright (not queued).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// No playing slot and waiting is disabled (`max_queue_len == 0`).
    ServerFull,
    /// No playing slot and the waitlist is full.
    QueueFull,
    /// This id is already playing or queued.
    AlreadyPresent,
    /// Empty id.
    InvalidId,
}

/// Outcome of [`Queue::join`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinOutcome {
    /// Occupies a playing slot immediately.
    Admitted,
    /// On the waitlist. `position` is 1-based.
    Queued { position: usize },
    /// Refused outright.
    Rejected { reason: RejectReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueuedEntry {
    id: String,
    holds_reservation: bool,
    enqueued_at_ms: u64,
}

/// The admission queue itself. Single-threaded logic; the owner guards sharing.
#[derive(Debug)]
pub struct Queue {
    config: QueueConfig,
    playing: Vec<QueuedEntry>,
    waitlist: VecDeque<QueuedEntry>,
}

impl Queue {
    pub fn new(config: QueueConfig) -> Self {
        Queue { config, playing: Vec::new(), waitlist: VecDeque::new() }
    }

    pub fn config(&self) -> &QueueConfig {
        &self.config
    }

    pub fn playing_count(&self) -> usize {
        self.playing.len()
    }

    pub fn queue_len(&self) -> usize {
        self.waitlist.len()
    }

    pub fn is_playing(&self, id: &str) -> bool {
        self.playing.iter().any(|e| e.id == id)
    }

    pub fn is_queued(&self, id: &str) -> bool {
        self.waitlist.iter().any(|e| e.id == id)
    }

    /// 1-based waitlist position, or `None` when not queued.
    pub fn position(&self, id: &str) -> Option<usize> {
        self.waitlist.iter().position(|e| e.id == id).map(|i| i + 1)
    }

    fn slot_free_for(&self, holds_reservation: bool) -> bool {
        if holds_reservation {
            self.playing.len() < self.config.max_players
        } else {
            self.playing.len() < self.config.open_slots()
        }
    }

    /// Attempt to join. Never fails: the outcome says what happened.
    pub fn join(&mut self, req: JoinRequest, now_ms: u64) -> JoinOutcome {
        if req.id.trim().is_empty() {
            return JoinOutcome::Rejected { reason: RejectReason::InvalidId };
        }
        if self.is_playing(&req.id) || self.is_queued(&req.id) {
            return JoinOutcome::Rejected { reason: RejectReason::AlreadyPresent };
        }
        if self.slot_free_for(req.holds_reservation) {
            self.playing.push(QueuedEntry {
                id: req.id,
                holds_reservation: req.holds_reservation,
                enqueued_at_ms: now_ms,
            });
            return JoinOutcome::Admitted;
        }
        if self.waitlist.len() < self.config.max_queue_len {
            self.waitlist.push_back(QueuedEntry {
                id: req.id,
                holds_reservation: req.holds_reservation,
                enqueued_at_ms: now_ms,
            });
            return JoinOutcome::Queued { position: self.waitlist.len() };
        }
        if self.config.max_queue_len == 0 {
            JoinOutcome::Rejected { reason: RejectReason::ServerFull }
        } else {
            JoinOutcome::Rejected { reason: RejectReason::QueueFull }
        }
    }

    /// Leave the playing set. Returns the waitlisted id promoted into the
    /// freed slot, if any. Unknown ids report `None` (no panic, no lie).
    pub fn playing_leave(&mut self, id: &str) -> Option<String> {
        let at = self.playing.iter().position(|e| e.id == id)?;
        self.playing.remove(at);
        self.promote_front_fit()
    }

    /// Leave the waitlist. Returns whether the id was queued.
    pub fn queue_leave(&mut self, id: &str) -> bool {
        match self.waitlist.iter().position(|e| e.id == id) {
            Some(at) => {
                self.waitlist.remove(at);
                true
            }
            None => false,
        }
    }

    /// Scan front-to-back; admit the first entry the reservation rule allows.
    fn promote_front_fit(&mut self) -> Option<String> {
        let at = self.waitlist.iter().position(|e| self.slot_free_for(e.holds_reservation))?;
        let entry = self.waitlist.remove(at).expect("position from iter is valid");
        self.playing.push(entry.clone());
        Some(entry.id)
    }

    /// Drop waitlist entries older than `stale_after_ms`. Returns ids removed,
    /// oldest first. Entries dated in the future are kept (clock skew grace).
    pub fn expire_stale(&mut self, now_ms: u64) -> Vec<String> {
        if self.config.stale_after_ms == 0 {
            return Vec::new();
        }
        let ttl = self.config.stale_after_ms;
        let mut removed = Vec::new();
        self.waitlist.retain(|e| {
            let age = now_ms.saturating_sub(e.enqueued_at_ms);
            // `saturating_sub` also covers future-dated entries (age 0).
            let stale = age > ttl;
            if stale {
                removed.push(e.id.clone());
            }
            !stale
        });
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(max: usize, reserved: usize, qlen: usize) -> QueueConfig {
        QueueConfig::new(max, reserved, qlen, 60_000).unwrap()
    }

    #[test]
    fn bad_config_rejected() {
        assert_eq!(QueueConfig::new(10, 11, 5, 0), Err(QueueError::TooManyReserved { max: 10, reserved: 11 }));
        // Boundary holds: all slots reserved is legal.
        assert!(QueueConfig::new(10, 10, 5, 0).is_ok());
    }

    #[test]
    fn direct_admit_until_full() {
        let mut q = Queue::new(cfg(2, 0, 4));
        assert_eq!(q.join(JoinRequest::new("a", false), 0), JoinOutcome::Admitted);
        assert_eq!(q.join(JoinRequest::new("b", false), 0), JoinOutcome::Admitted);
        assert_eq!(q.join(JoinRequest::new("c", false), 0), JoinOutcome::Queued { position: 1 });
        assert_eq!(q.playing_count(), 2);
        assert!(q.is_playing("a"));
        assert!(q.is_queued("c"));
    }

    #[test]
    fn reserved_slots_held_for_holders() {
        let mut q = Queue::new(cfg(4, 1, 4));
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            assert_eq!(q.join(JoinRequest::new(id, false), i as u64), JoinOutcome::Admitted);
        }
        // Only the reserved slot is free: normal player must queue.
        assert_eq!(q.join(JoinRequest::new("d", false), 0), JoinOutcome::Queued { position: 1 });
        // A holder takes it directly.
        assert_eq!(q.join(JoinRequest::new("vip", true), 0), JoinOutcome::Admitted);
        assert_eq!(q.playing_count(), 4);
    }

    #[test]
    fn duplicate_join_rejected() {
        let mut q = Queue::new(cfg(1, 0, 4));
        assert_eq!(q.join(JoinRequest::new("a", false), 0), JoinOutcome::Admitted);
        assert_eq!(
            q.join(JoinRequest::new("a", false), 0),
            JoinOutcome::Rejected { reason: RejectReason::AlreadyPresent }
        );
        let mut q2 = Queue::new(cfg(1, 0, 4));
        q2.join(JoinRequest::new("x", false), 0);
        q2.join(JoinRequest::new("y", false), 0);
        assert_eq!(
            q2.join(JoinRequest::new("y", false), 0),
            JoinOutcome::Rejected { reason: RejectReason::AlreadyPresent }
        );
    }

    #[test]
    fn empty_id_rejected() {
        let mut q = Queue::new(cfg(4, 0, 4));
        assert_eq!(
            q.join(JoinRequest::new("   ", false), 0),
            JoinOutcome::Rejected { reason: RejectReason::InvalidId }
        );
    }

    #[test]
    fn full_queue_rejects() {
        let mut q = Queue::new(cfg(1, 0, 1));
        q.join(JoinRequest::new("a", false), 0);
        assert_eq!(q.join(JoinRequest::new("b", false), 0), JoinOutcome::Queued { position: 1 });
        assert_eq!(q.join(JoinRequest::new("c", false), 0), JoinOutcome::Rejected { reason: RejectReason::QueueFull });
    }

    #[test]
    fn zero_queue_is_direct_or_reject() {
        let mut q = Queue::new(cfg(1, 0, 0));
        q.join(JoinRequest::new("a", false), 0);
        assert_eq!(q.join(JoinRequest::new("b", false), 0), JoinOutcome::Rejected { reason: RejectReason::ServerFull });
    }

    #[test]
    fn leave_promotes_head() {
        let mut q = Queue::new(cfg(2, 0, 4));
        q.join(JoinRequest::new("a", false), 0);
        q.join(JoinRequest::new("b", false), 0);
        q.join(JoinRequest::new("c", false), 0);
        q.join(JoinRequest::new("d", false), 0);
        assert_eq!(q.playing_leave("a"), Some("c".to_string()));
        assert!(q.is_playing("c"));
        assert_eq!(q.position("d"), Some(1));
        // Unknown id: no promotion, no panic.
        assert_eq!(q.playing_leave("ghost"), None);
    }

    #[test]
    fn promotion_skips_blocked_head() {
        // 2 slots, 1 reserved. Playing: one normal + one holder.
        let mut q = Queue::new(cfg(2, 1, 4));
        q.join(JoinRequest::new("n1", false), 0);
        q.join(JoinRequest::new("h1", true), 0);
        // Head of the queue is normal (blocked: open slots full), then holder.
        q.join(JoinRequest::new("n2", false), 0);
        q.join(JoinRequest::new("h2", true), 0);
        // Holder leaves: freed slot still fits a holder first from the front
        // scan — h2 is behind n2, but n2 does not fit, so h2 promotes.
        assert_eq!(q.playing_leave("h1"), Some("h2".to_string()));
        assert_eq!(q.position("n2"), Some(1));
    }

    #[test]
    fn queue_leave_removes_and_reindexes() {
        let mut q = Queue::new(cfg(1, 0, 4));
        q.join(JoinRequest::new("a", false), 0);
        q.join(JoinRequest::new("b", false), 0);
        q.join(JoinRequest::new("c", false), 0);
        assert!(q.queue_leave("b"));
        assert_eq!(q.position("c"), Some(1));
        assert!(!q.queue_leave("b"));
        assert!(!q.queue_leave("a")); // playing, not queued
    }

    #[test]
    fn stale_entries_expire_oldest_first() {
        let mut q = Queue::new(QueueConfig::new(1, 0, 8, 1_000).unwrap());
        q.join(JoinRequest::new("a", false), 0);
        q.join(JoinRequest::new("old1", false), 0);
        q.join(JoinRequest::new("old2", false), 500);
        q.join(JoinRequest::new("fresh", false), 1_500);
        // now = 2000: old1 (age 2000) and old2 (age 1500) are stale.
        assert_eq!(q.expire_stale(2_000), vec!["old1".to_string(), "old2".to_string()]);
        assert_eq!(q.position("fresh"), Some(1));
        // Boundary: age exactly ttl is kept (strictly-greater expiry).
        let mut q2 = Queue::new(QueueConfig::new(1, 0, 8, 1_000).unwrap());
        q2.join(JoinRequest::new("a", false), 0);
        q2.join(JoinRequest::new("edge", false), 1_000);
        assert!(q2.expire_stale(2_000).is_empty());
    }

    #[test]
    fn expiry_disabled_when_ttl_zero() {
        let mut q = Queue::new(QueueConfig::new(1, 0, 8, 0).unwrap());
        q.join(JoinRequest::new("a", false), 0);
        q.join(JoinRequest::new("b", false), 0);
        assert!(q.expire_stale(u64::MAX).is_empty());
        assert_eq!(q.queue_len(), 1);
    }
}
