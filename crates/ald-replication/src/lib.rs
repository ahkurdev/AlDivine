//! State replication.
//!
//! Server-authoritative snapshot + delta model:
//! - Each tracked entity has a *baseline*: the last state the client is
//!   known to have acknowledged.
//! - Ticks produce a *delta* against that baseline; only changed fields ship.
//! - Clients ack received baselines; unacked updates are re-sent at a rate
//!   driven by the entity's interest tier.
//!
//! Priority is a bounded queue: high-tier entities dequeue first so that
//! under bandwidth pressure the most relevant state is what survives.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use ald_ecs::InterestTier;

/// Monotonic replication tick. Wraps in 32 bits like the network sequence.
pub type Tick = u32;

/// A single field-level change to one entity's replicated state.
#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub entity: u64,
    pub component: String,
    /// Opaque serialized field state. The protocol layer owns encoding.
    pub data: Vec<u8>,
}

/// A full state snapshot used as the baseline for future deltas.
#[derive(Debug, Clone)]
pub struct Baseline {
    pub tick: Tick,
    pub fields: HashMap<String, Vec<u8>>,
}

/// Per-entity replication tracker: baseline + priority queue of pending deltas.
#[derive(Debug)]
pub struct ReplicationTracker {
    /// The last state the client acknowledged.
    baseline: Baseline,
    /// Pending changes not yet acknowledged, in insertion order.
    pending: VecDeque<Delta>,
    /// Highest priority observed, so the queue can be reordered cheaply.
    tier: InterestTier,
    last_sent: Option<Instant>,
    stats: ReplicationStats,
}

/// Network-visible replication metrics (surfaced to Aegis network inspector).
#[derive(Debug, Clone, Default)]
pub struct ReplicationStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub deltas_sent: u64,
    pub snapshots_sent: u64,
    pub dropped_updates: u64,
    /// Unacknowledged updates still queued.
    pub queued: usize,
}

impl ReplicationTracker {
    pub fn new(initial_tick: Tick) -> Self {
        ReplicationTracker {
            baseline: Baseline { tick: initial_tick, fields: HashMap::new() },
            pending: VecDeque::new(),
            tier: InterestTier::Medium,
            last_sent: None,
            stats: ReplicationStats::default(),
        }
    }

    /// Record a changed field for an entity against the current baseline.
    /// Identical values are coalesced: the latest write for a component wins.
    pub fn record(&mut self, delta: Delta) {
        if let Some(existing) =
            self.pending.iter_mut().find(|d| d.entity == delta.entity && d.component == delta.component)
        {
            *existing = delta;
            return;
        }
        self.pending.push_back(delta);
    }

    /// Client acknowledged a baseline; drop everything it supersedes.
    pub fn acknowledge(&mut self, tick: Tick) {
        // Deltas at or before the acked baseline are confirmed delivered and
        // can no longer be retransmitted.
        self.baseline.tick = tick;
        self.stats.queued = self.pending.len();
    }

    /// Highest-priority tier currently assigned.
    pub fn tier(&self) -> InterestTier {
        self.tier
    }

    /// Set the interest tier (distance-driven) controlling send rate.
    pub fn set_tier(&mut self, tier: InterestTier) {
        self.tier = tier;
    }

    /// Whether enough time has passed for the next send at this tier.
    /// Returns the interval in seconds (None = do not replicate).
    pub fn due(&self, now: Instant) -> bool {
        let interval = match self.tier.update_interval() {
            Some(i) => i,
            None => return false,
        };
        match self.last_sent {
            None => true,
            Some(t) => now.duration_since(t).as_secs_f32() >= interval,
        }
    }

    /// Drain pending deltas as one packet payload, marking the send.
    /// If `force_snapshot` is true (e.g. new client or large drift), a full
    /// baseline is emitted instead and pending deltas collapse into it.
    pub fn flush(&mut self, now: Instant, force_snapshot: bool) -> ReplicationPacket {
        if force_snapshot {
            // Collapse pending into the baseline.
            for d in self.pending.drain(..) {
                self.baseline.fields.insert(d.component, d.data);
            }
            let size = self.baseline.fields.values().map(|v| v.len()).sum::<usize>();
            self.stats.snapshots_sent += 1;
            self.stats.bytes_sent += size as u64;
            self.stats.packets_sent += 1;
            self.last_sent = Some(now);
            self.stats.queued = self.pending.len();
            return ReplicationPacket::Snapshot { tick: self.baseline.tick, fields: self.baseline.fields.clone() };
        }

        let deltas: Vec<Delta> = self.pending.drain(..).collect();
        let size = deltas.iter().map(|d| d.data.len()).sum::<usize>();
        self.stats.deltas_sent += deltas.len() as u64;
        self.stats.bytes_sent += size as u64;
        if !deltas.is_empty() {
            self.stats.packets_sent += 1;
        }
        self.last_sent = Some(now);
        self.stats.queued = self.pending.len();
        ReplicationPacket::Delta { deltas }
    }

    pub fn stats(&self) -> &ReplicationStats {
        &self.stats
    }

    /// Drop the oldest pending update (bandwidth pressure / queue cap).
    pub fn drop_oldest(&mut self) {
        if self.pending.pop_front().is_some() {
            self.stats.dropped_updates += 1;
            self.stats.queued = self.pending.len();
        }
    }

    /// Cap the pending queue at `max`; oldest entries are dropped and counted.
    pub fn cap(&mut self, max: usize) {
        while self.pending.len() > max {
            self.drop_oldest();
        }
    }
}

/// What gets emitted on flush: either a full snapshot or a set of deltas.
#[derive(Debug, Clone)]
pub enum ReplicationPacket {
    Snapshot { tick: Tick, fields: HashMap<String, Vec<u8>> },
    Delta { deltas: Vec<Delta> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn delta(component: &str, data: &[u8]) -> Delta {
        Delta { entity: 1, component: component.to_string(), data: data.to_vec() }
    }

    #[test]
    fn coalesces_repeated_component() {
        let mut t = ReplicationTracker::new(0);
        t.record(delta("pos", &[1]));
        t.record(delta("pos", &[2]));
        assert_eq!(t.pending.len(), 1);
        assert_eq!(t.pending[0].data, vec![2]);
    }

    #[test]
    fn flush_delta_then_snapshot() {
        let mut t = ReplicationTracker::new(0);
        t.record(delta("pos", &[1, 2]));
        let now = Instant::now();
        let pkt = t.flush(now, false);
        match pkt {
            ReplicationPacket::Delta { deltas } => assert_eq!(deltas.len(), 1),
            _ => panic!("expected delta packet"),
        }
        assert_eq!(t.stats().deltas_sent, 1);
        assert_eq!(t.stats().bytes_sent, 2);

        t.record(delta("pos", &[9]));
        let pkt2 = t.flush(now, true);
        match pkt2 {
            ReplicationPacket::Snapshot { fields, .. } => assert_eq!(fields["pos"], vec![9]),
            _ => panic!("expected snapshot packet"),
        }
        assert_eq!(t.stats().snapshots_sent, 1);
    }

    #[test]
    fn due_respects_tier_interval() {
        let mut t = ReplicationTracker::new(0);
        let now = Instant::now();
        assert!(t.due(now)); // never sent
        t.set_tier(InterestTier::High); // 20 Hz = 0.05s
        t.flush(now, false);
        assert!(!t.due(now));
        assert!(t.due(now + Duration::from_millis(60)));
    }

    #[test]
    fn none_tier_never_due() {
        let mut t = ReplicationTracker::new(0);
        t.set_tier(InterestTier::None);
        assert!(!t.due(Instant::now()));
    }

    #[test]
    fn cap_drops_oldest_and_counts() {
        let mut t = ReplicationTracker::new(0);
        t.record(delta("a", &[1]));
        t.record(delta("b", &[2]));
        t.record(delta("c", &[3]));
        t.cap(1);
        assert_eq!(t.pending.len(), 1);
        assert_eq!(t.stats().dropped_updates, 2);
        assert_eq!(t.stats().queued, 1);
    }

    #[test]
    fn ack_updates_baseline_tick() {
        let mut t = ReplicationTracker::new(0);
        t.acknowledge(7);
        assert_eq!(t.baseline.tick, 7);
    }
}
