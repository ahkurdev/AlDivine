//! Download engine control plane: chunking, resume, watchdog, fair lanes.
//!
//! This is the *scheduling and correctness* half of the streaming pipeline:
//! how a file is split, what is still missing, whether a transfer stalled,
//! and whose turn it is on the wire. It is deliberately transport-free —
//! the caller moves bytes (from CDN, cache, or peer) and reports progress
//! here. That split is what makes every rule below testable without a fake
//! network.
//!
//! - [`ChunkPlan`]: file length + chunk length → count and byte ranges. The
//!   last chunk may be short; zero-length files and zero chunk lengths are
//!   rejected, not silently accepted.
//! - [`Download`]: receive-side reassembly over a caller-supplied virtual
//!   tick clock, tracked on a per-byte coverage bitmap. `receive()` is
//!   idempotent on overlap (retransmits are normal) and rejects
//!   out-of-bounds writes; only newly covered bytes advance progress.
//!   `missing_ranges()` drives resume: request exactly what is absent.
//! - [`Watchdog`]: stall detection from progress ticks. No progress for
//!   `stall_after` ticks reads stalled; any fresh byte resets it. The
//!   watchdog never acts — it reports, and the caller cancels/retries — so
//!   "never wait forever" is policy, not a timer hidden in here.
//! - [`LaneScheduler`]: deficit round robin across the five spec lanes with
//!   weights (join-critical highest). One giant package cannot monopolize
//!   workers: DRR serves lanes proportionally, and empty lanes are skipped
//!   without consuming deficit.
//!
//! Memory note: [`Download`] holds a coverage bitmap plus assembled bytes,
//! which bounds this exact struct to control-plane use and test scale; the
//! disk-backed sink (staging files, background writer) arrives with the
//! cache phase through the same `receive()` / `missing_ranges()` API.

use std::collections::{HashMap, VecDeque};

use thiserror::Error;

/// Streaming lanes, highest priority first. Weights are scheduling shares,
/// not strict priority — every non-empty lane progresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lane {
    JoinCritical,
    SpawnCritical,
    Foreground,
    Background,
    Optional,
}

impl Lane {
    /// Deficit-round-robin quantum per lane per round.
    pub fn weight(self) -> u64 {
        match self {
            Lane::JoinCritical => 8,
            Lane::SpawnCritical => 4,
            Lane::Foreground => 2,
            Lane::Background => 1,
            Lane::Optional => 1,
        }
    }

    pub fn all() -> [Lane; 5] {
        [Lane::JoinCritical, Lane::SpawnCritical, Lane::Foreground, Lane::Background, Lane::Optional]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DownloadError {
    #[error("invalid plan: file_len {0}, chunk_len {1}")]
    BadPlan(u64, u64),
    #[error("write out of bounds: offset {offset}, len {len}, file {file_len}")]
    OutOfBounds { offset: u64, len: usize, file_len: u64 },
}

/// Split of a file into chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPlan {
    pub file_len: u64,
    pub chunk_len: u64,
    pub chunk_count: u64,
}

impl ChunkPlan {
    pub fn new(file_len: u64, chunk_len: u64) -> Result<Self, DownloadError> {
        if file_len == 0 || chunk_len == 0 {
            return Err(DownloadError::BadPlan(file_len, chunk_len));
        }
        let chunk_count = file_len.div_ceil(chunk_len);
        Ok(ChunkPlan { file_len, chunk_len, chunk_count })
    }

    /// Byte range `[start, end)` of chunk `i`. `None` when out of range.
    pub fn chunk_range(&self, i: u64) -> Option<(u64, u64)> {
        if i >= self.chunk_count {
            return None;
        }
        let start = i * self.chunk_len;
        let end = (start + self.chunk_len).min(self.file_len);
        Some((start, end))
    }
}

/// One in-progress download: per-byte coverage, reassembly, progress clock.
#[derive(Debug)]
pub struct Download {
    plan: ChunkPlan,
    bytes: Vec<u8>,
    covered: Vec<bool>,
    received_bytes: u64,
    last_progress_tick: u64,
}

impl Download {
    pub fn new(plan: ChunkPlan, now_tick: u64) -> Self {
        Download {
            bytes: vec![0; plan.file_len as usize],
            covered: vec![false; plan.file_len as usize],
            plan,
            received_bytes: 0,
            last_progress_tick: now_tick,
        }
    }

    pub fn plan(&self) -> &ChunkPlan {
        &self.plan
    }

    /// Record received bytes. Overlap is idempotent (only fresh bytes count
    /// toward progress); anything outside `[0, file_len)` is rejected;
    /// writes after completion are rejected.
    pub fn receive(&mut self, offset: u64, data: &[u8], now_tick: u64) -> Result<(), DownloadError> {
        if self.is_complete() {
            return Err(DownloadError::OutOfBounds { offset, len: data.len(), file_len: self.plan.file_len });
        }
        let end = offset.checked_add(data.len() as u64).ok_or(DownloadError::OutOfBounds {
            offset,
            len: data.len(),
            file_len: self.plan.file_len,
        })?;
        if end > self.plan.file_len {
            return Err(DownloadError::OutOfBounds { offset, len: data.len(), file_len: self.plan.file_len });
        }
        if data.is_empty() {
            return Ok(()); // keep-alive with no bytes: no progress, no error
        }
        let mut fresh = 0u64;
        for (i, byte) in data.iter().enumerate() {
            let at = offset as usize + i;
            self.bytes[at] = *byte;
            if !self.covered[at] {
                self.covered[at] = true;
                fresh += 1;
            }
        }
        if fresh > 0 {
            self.received_bytes += fresh;
            self.last_progress_tick = now_tick;
        }
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.received_bytes == self.plan.file_len
    }

    /// Fully covered chunk count (a chunk counts only when every byte landed).
    pub fn complete_chunks(&self) -> u64 {
        (0..self.plan.chunk_count)
            .filter(|i| {
                let (s, e) = self.plan.chunk_range(*i).expect("range in plan");
                (s..e).all(|b| self.covered[b as usize])
            })
            .count() as u64
    }

    /// Absent byte runs as `(offset, len)` in ascending order — the exact
    /// resume shopping list. Empty when complete.
    pub fn missing_ranges(&self) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut run: Option<(u64, u64)> = None;
        for (i, covered) in self.covered.iter().enumerate() {
            if *covered {
                if let Some((s, l)) = run.take() {
                    out.push((s, l));
                }
            } else {
                let i = i as u64;
                run = Some(match run {
                    Some((s, l)) => (s, l + 1),
                    None => (i, 1),
                });
            }
        }
        if let Some((s, l)) = run {
            out.push((s, l));
        }
        out
    }

    /// Assembled file. `None` until complete — partial bytes are never valid.
    pub fn assembled(&self) -> Option<&[u8]> {
        self.is_complete().then_some(&self.bytes)
    }

    pub fn received_bytes(&self) -> u64 {
        self.received_bytes
    }

    pub fn last_progress_tick(&self) -> u64 {
        self.last_progress_tick
    }
}

/// Stall detection over virtual ticks. Reports only; the caller retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watchdog {
    /// Ticks without progress after which a transfer reads stalled.
    pub stall_after_ticks: u64,
}

impl Watchdog {
    pub fn new(stall_after_ticks: u64) -> Self {
        Watchdog { stall_after_ticks: stall_after_ticks.max(1) }
    }

    pub fn is_stalled(&self, last_progress_tick: u64, now_tick: u64) -> bool {
        now_tick.saturating_sub(last_progress_tick) > self.stall_after_ticks
    }
}

/// Deficit round robin across lanes. The rotation visits lanes in priority
/// order; each visit grants the lane a turn of up to `weight` consecutive
/// jobs, then moves on — shares converge to the weight ratios while every
/// non-empty lane progresses. Empty lanes are skipped and bank nothing.
#[derive(Debug, Default)]
pub struct LaneScheduler {
    queues: HashMap<Lane, VecDeque<u64>>,
    /// Rotation pointer into priority order.
    pos: usize,
    /// Remaining jobs in the current lane's turn.
    remaining: HashMap<Lane, u64>,
    served: HashMap<Lane, u64>,
}

impl LaneScheduler {
    pub fn new() -> Self {
        LaneScheduler::default()
    }

    pub fn enqueue(&mut self, lane: Lane, job: u64) {
        self.queues.entry(lane).or_default().push_back(job);
    }

    pub fn pending(&self, lane: Lane) -> usize {
        self.queues.get(&lane).map(VecDeque::len).unwrap_or(0)
    }

    pub fn total_pending(&self) -> usize {
        self.queues.values().map(VecDeque::len).sum()
    }

    pub fn served(&self, lane: Lane) -> u64 {
        self.served.get(&lane).copied().unwrap_or(0)
    }

    /// Next job to serve, or `None` when every lane is empty.
    pub fn pop_next(&mut self) -> Option<(Lane, u64)> {
        if self.total_pending() == 0 {
            self.pos = 0;
            self.remaining.clear();
            return None;
        }
        let order = Lane::all();
        for _ in 0..order.len() {
            let lane = order[self.pos % order.len()];
            if self.pending(lane) == 0 {
                // Skipped lanes bank nothing for later.
                self.remaining.remove(&lane);
                self.pos = (self.pos + 1) % order.len();
                continue;
            }
            // Quantum for a fresh turn, or the remainder of an ongoing one.
            // Entries are removed when a turn ends, so this is never zero.
            // Copied out (not borrowed) so the queue borrow below is legal.
            let mut rem = self.remaining.get(&lane).copied().unwrap_or_else(|| lane.weight());
            rem -= 1;
            let job = self.queues.get_mut(&lane).expect("non-empty checked").pop_front().expect("non-empty checked");
            if self.pending(lane) == 0 || rem == 0 {
                // Turn over: queue drained or quantum spent.
                self.remaining.remove(&lane);
                self.pos = (self.pos + 1) % order.len();
            } else {
                self.remaining.insert(lane, rem);
            }
            *self.served.entry(lane).or_insert(0) += 1;
            return Some((lane, job));
        }
        // Unreachable: a non-empty lane always yields within one rotation.
        // Return None instead of looping forever if logic ever drifts.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_math_exact_and_partial() {
        let p = ChunkPlan::new(1024, 256).unwrap();
        assert_eq!(p.chunk_count, 4);
        assert_eq!(p.chunk_range(3), Some((768, 1024)));
        assert_eq!(p.chunk_range(4), None);
        let p = ChunkPlan::new(1000, 256).unwrap();
        assert_eq!(p.chunk_count, 4);
        assert_eq!(p.chunk_range(3), Some((768, 1000)));
        assert_eq!(ChunkPlan::new(0, 256), Err(DownloadError::BadPlan(0, 256)));
        assert_eq!(ChunkPlan::new(100, 0), Err(DownloadError::BadPlan(100, 0)));
    }

    #[test]
    fn receive_assembles_in_any_order() {
        let plan = ChunkPlan::new(10, 4).unwrap();
        let mut d = Download::new(plan, 0);
        d.receive(6, b"ghij", 1).unwrap();
        d.receive(0, b"abcd", 2).unwrap();
        assert!(!d.is_complete());
        assert_eq!(d.received_bytes(), 8);
        assert_eq!(d.complete_chunks(), 2); // chunks [0,4) [4,8): second partial
        assert_eq!(d.missing_ranges(), vec![(4, 2)]);
        assert_eq!(d.assembled(), None); // partial is never valid
        d.receive(4, b"ef", 3).unwrap();
        assert!(d.is_complete());
        assert_eq!(d.assembled(), Some(b"abcdefghij".as_slice()));
        assert!(d.missing_ranges().is_empty());
    }

    #[test]
    fn overlap_is_idempotent() {
        let plan = ChunkPlan::new(8, 4).unwrap();
        let mut d = Download::new(plan, 0);
        d.receive(0, b"abcd", 1).unwrap();
        assert_eq!(d.last_progress_tick(), 1);
        d.receive(2, b"cdef", 5).unwrap(); // 2 fresh bytes
        assert_eq!(d.received_bytes(), 6);
        assert_eq!(d.last_progress_tick(), 5);
        d.receive(0, b"abcdef", 9).unwrap(); // pure retransmit: no progress
        assert_eq!(d.received_bytes(), 6);
        assert_eq!(d.last_progress_tick(), 5);
    }

    #[test]
    fn out_of_bounds_rejected() {
        let plan = ChunkPlan::new(8, 4).unwrap();
        let mut d = Download::new(plan, 0);
        assert!(matches!(d.receive(7, b"xy", 1), Err(DownloadError::OutOfBounds { .. })));
        assert!(matches!(d.receive(8, b"x", 1), Err(DownloadError::OutOfBounds { .. })));
        assert!(matches!(d.receive(u64::MAX, b"x", 1), Err(DownloadError::OutOfBounds { .. })));
        assert_eq!(d.received_bytes(), 0);
        d.receive(0, b"12345678", 1).unwrap();
        assert!(d.receive(0, b"1", 2).is_err()); // writes after completion
    }

    #[test]
    fn empty_receive_is_keepalive() {
        let plan = ChunkPlan::new(4, 4).unwrap();
        let mut d = Download::new(plan, 0);
        d.receive(0, b"", 7).unwrap();
        assert_eq!(d.received_bytes(), 0);
        assert_eq!(d.last_progress_tick(), 0);
    }

    #[test]
    fn watchdog_stall_boundary() {
        let w = Watchdog::new(10);
        assert!(!w.is_stalled(100, 110)); // exactly at budget: not stalled
        assert!(w.is_stalled(100, 111));
        assert!(!w.is_stalled(100, 50)); // clock moved backward: no false stall
        assert_eq!(Watchdog::new(0).stall_after_ticks, 1); // clamped
    }

    #[test]
    fn watchdog_tracks_download_progress() {
        let plan = ChunkPlan::new(100, 10).unwrap();
        let mut d = Download::new(plan, 0);
        let w = Watchdog::new(5);
        d.receive(0, b"0123456789", 3).unwrap();
        assert!(!w.is_stalled(d.last_progress_tick(), 8));
        assert!(w.is_stalled(d.last_progress_tick(), 9));
        d.receive(10, b"0123456789", 20).unwrap(); // progress resets the clock
        assert!(!w.is_stalled(d.last_progress_tick(), 25));
    }

    #[test]
    fn drr_serves_proportionally() {
        let mut s = LaneScheduler::new();
        for lane in Lane::all() {
            for job in 0..160 {
                s.enqueue(lane, job);
            }
        }
        for _ in 0..160 {
            assert!(s.pop_next().is_some());
        }
        // Cycle = 8+4+2+1+1 = 16 pops; 160 pops = 10 exact cycles.
        assert_eq!(s.served(Lane::JoinCritical), 80);
        assert_eq!(s.served(Lane::SpawnCritical), 40);
        assert_eq!(s.served(Lane::Foreground), 20);
        assert_eq!(s.served(Lane::Background), 10);
        assert_eq!(s.served(Lane::Optional), 10);
    }

    #[test]
    fn empty_lanes_skipped_without_hoarding() {
        let mut s = LaneScheduler::new();
        s.enqueue(Lane::Background, 1);
        s.enqueue(Lane::Background, 2);
        // Background alone: served immediately, no waiting for others.
        assert_eq!(s.pop_next(), Some((Lane::Background, 1)));
        assert_eq!(s.pop_next(), Some((Lane::Background, 2)));
        assert_eq!(s.pop_next(), None);
        assert_eq!(s.total_pending(), 0);
        // A lane that arrives later starts at zero deficit, not banked credit.
        s.enqueue(Lane::JoinCritical, 9);
        assert_eq!(s.pop_next(), Some((Lane::JoinCritical, 9)));
    }

    #[test]
    fn small_job_finishes_while_bulk_continues() {
        let mut s = LaneScheduler::new();
        for job in 0..100 {
            s.enqueue(Lane::Background, job);
        }
        s.enqueue(Lane::JoinCritical, 999);
        // Join-critical's weight gets it served within the first round.
        let mut seen = false;
        for _ in 0..16 {
            if s.pop_next() == Some((Lane::JoinCritical, 999)) {
                seen = true;
                break;
            }
        }
        assert!(seen);
        assert!(s.total_pending() > 0); // bulk background still queued
    }
}
