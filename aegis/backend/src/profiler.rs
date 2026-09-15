//! Aegis performance profiler.
//!
//! Tracks per-resource execution cost so an operator can see which resource
//! is eating the tick budget before the whole server degrades. Records:
//!   - wall-clock time per handler invocation
//!   - allocation counts (where the runtime reports them)
//!   - pending async operations
//!   - slow handlers, sorted worst-first
//!
//! The windowed ring buffer keeps a bounded history: profiling data is high
//! volume, and an unbounded profiler is itself a memory leak.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// How many recent samples to keep per handler before evicting the oldest.
const SAMPLE_WINDOW: usize = 256;

/// Percentile distribution of a handler's recent execution times.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionStats {
    pub samples: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub mean_us: u64,
    pub last_us: u64,
}

/// One resource's profiler state.
#[derive(Debug)]
pub struct ResourceProfile {
    pub resource: String,
    samples: VecDeque<u64>,
    pub total_invocations: u64,
    pub total_time_us: u64,
    pub slow_invocations: u64,
    slow_threshold_us: u64,
    pub errors: u64,
    pub warnings: u64,
    pub events_per_sec: f64,
    pub pending_async: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
}

impl ResourceProfile {
    pub fn new(resource: String, slow_threshold_us: u64) -> Self {
        ResourceProfile {
            resource,
            samples: VecDeque::with_capacity(SAMPLE_WINDOW.min(64)),
            total_invocations: 0,
            total_time_us: 0,
            slow_invocations: 0,
            slow_threshold_us,
            errors: 0,
            warnings: 0,
            events_per_sec: 0.0,
            pending_async: 0,
            bytes_sent: 0,
            bytes_recv: 0,
        }
    }

    /// Record one handler execution. `micros` is wall-clock microseconds.
    pub fn record(&mut self, micros: u64) {
        self.total_invocations += 1;
        self.total_time_us += micros;
        if micros > self.slow_threshold_us {
            self.slow_invocations += 1;
        }
        if self.samples.len() >= SAMPLE_WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(micros);
    }

    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    pub fn record_warning(&mut self) {
        self.warnings += 1;
    }

    pub fn record_network(&mut self, sent: u64, recv: u64) {
        self.bytes_sent += sent;
        self.bytes_recv += recv;
    }

    /// Compute percentiles over the sample window.
    ///
    /// Nearest-rank: the P95 of 100 sorted samples is the 95th value. For
    /// windows under 100 samples this is coarse but honest — it does not
    /// interpolate a value that was never measured.
    pub fn stats(&self) -> ExecutionStats {
        let n = self.samples.len();
        if n == 0 {
            return ExecutionStats::default();
        }
        let mut sorted: Vec<u64> = self.samples.iter().copied().collect();
        sorted.sort_unstable();

        let rank = |pct: f64| -> usize {
            let r = (pct / 100.0 * n as f64).ceil() as usize;
            r.saturating_sub(1).min(n - 1)
        };

        let p50 = sorted[rank(50.0)];
        let p95 = sorted[rank(95.0)];
        let p99 = sorted[rank(99.0)];
        let max = sorted[n - 1];
        let mean = self.total_time_us / self.total_invocations.max(1);

        ExecutionStats {
            samples: self.total_invocations,
            p50_us: p50,
            p95_us: p95,
            p99_us: p99,
            max_us: max,
            mean_us: mean,
            last_us: *self.samples.back().unwrap_or(&0),
        }
    }

    /// Total CPU time spent in this resource, in whole milliseconds.
    pub fn total_cpu_ms(&self) -> u64 {
        self.total_time_us / 1000
    }
}

/// The profiler: one profile per resource.
pub struct Profiler {
    profiles: std::collections::HashMap<String, ResourceProfile>,
    slow_threshold_us: u64,
}

impl Profiler {
    pub fn new(slow_threshold_us: u64) -> Self {
        Profiler { profiles: std::collections::HashMap::new(), slow_threshold_us }
    }

    /// Observe a handler execution, creating the profile on first use.
    pub fn observe(&mut self, resource: &str, micros: u64) {
        let threshold = self.slow_threshold_us;
        let profile = self
            .profiles
            .entry(resource.to_string())
            .or_insert_with(|| ResourceProfile::new(resource.to_string(), threshold));
        profile.record(micros);
    }

    pub fn record_error(&mut self, resource: &str) {
        if let Some(p) = self.profiles.get_mut(resource) {
            p.record_error();
        }
    }

    pub fn record_network(&mut self, resource: &str, sent: u64, recv: u64) {
        if let Some(p) = self.profiles.get_mut(resource) {
            p.record_network(sent, recv);
        }
    }

    pub fn get(&self, resource: &str) -> Option<&ResourceProfile> {
        self.profiles.get(resource)
    }

    /// All resources sorted by total CPU time, worst first.
    pub fn by_cpu(&self) -> Vec<(&String, u64)> {
        let mut all: Vec<(&String, u64)> = self.profiles.iter().map(|(name, p)| (name, p.total_cpu_ms())).collect();
        all.sort_by(|a, b| b.1.cmp(&a.1));
        all
    }

    /// The slowest handlers by P95.
    pub fn slowest_by_p95(&self) -> Vec<(&String, ExecutionStats)> {
        let mut all: Vec<(&String, ExecutionStats)> = self.profiles.iter().map(|(name, p)| (name, p.stats())).collect();
        all.sort_by(|a, b| b.1.p95_us.cmp(&a.1.p95_us));
        all
    }

    /// Snapshot for the Aegis dashboard.
    pub fn snapshot(&self) -> Vec<ResourceSnapshot> {
        self.profiles
            .iter()
            .map(|(name, p)| ResourceSnapshot {
                resource: name.clone(),
                invocations: p.total_invocations,
                cpu_ms: p.total_cpu_ms(),
                slow: p.slow_invocations,
                errors: p.errors,
                warnings: p.warnings,
                stats: p.stats(),
                bytes_sent: p.bytes_sent,
                bytes_recv: p.bytes_recv,
            })
            .collect()
    }
}

/// A flat, serializable view of one resource's cost for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub resource: String,
    pub invocations: u64,
    pub cpu_ms: u64,
    pub slow: u64,
    pub errors: u64,
    pub warnings: u64,
    pub stats: ExecutionStats,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_profile_has_no_stats() {
        let p = ResourceProfile::new("r".into(), 1000);
        let s = p.stats();
        assert_eq!(s.samples, 0);
        assert_eq!(s.p50_us, 0);
    }

    #[test]
    fn percentiles_of_known_samples() {
        let mut p = ResourceProfile::new("r".into(), 10_000);
        // 100 samples, 1..=100 microseconds.
        for i in 1..=100u64 {
            p.record(i);
        }
        let s = p.stats();
        assert_eq!(s.p50_us, 50);
        assert_eq!(s.p95_us, 95);
        assert_eq!(s.p99_us, 99);
        assert_eq!(s.max_us, 100);
        assert_eq!(s.last_us, 100);
    }

    #[test]
    fn mean_is_total_over_count() {
        let mut p = ResourceProfile::new("r".into(), 10_000);
        p.record(100);
        p.record(300);
        let s = p.stats();
        assert_eq!(s.mean_us, 200);
    }

    #[test]
    fn slow_invocations_counted() {
        let mut p = ResourceProfile::new("r".into(), 50);
        p.record(10);
        p.record(100);
        p.record(200);
        assert_eq!(p.slow_invocations, 2);
    }

    #[test]
    fn sample_window_evicts_oldest() {
        let mut p = ResourceProfile::new("r".into(), 1_000_000);
        for i in 0..(SAMPLE_WINDOW + 100) as u64 {
            p.record(i);
        }
        assert_eq!(p.samples.len(), SAMPLE_WINDOW);
        // The window holds the most recent samples, so the minimum is no
        // longer 0 — it was evicted.
        assert!(*p.samples.front().unwrap() > 0);
    }

    #[test]
    fn profiler_creates_and_aggregates() {
        let mut prof = Profiler::new(1000);
        prof.observe("a", 100);
        prof.observe("a", 200);
        prof.observe("b", 5000);
        let by_cpu = prof.by_cpu();
        assert_eq!(by_cpu[0].0, "b");
        assert_eq!(by_cpu[0].1, 5);
    }

    #[test]
    fn slowest_by_p95_orders_worst_first() {
        let mut prof = Profiler::new(1_000_000);
        for _ in 0..100 {
            prof.observe("fast", 10);
            prof.observe("slow", 5000);
        }
        let slowest = prof.slowest_by_p95();
        assert_eq!(slowest[0].0, "slow");
        assert_eq!(slowest[0].1.p95_us, 5000);
    }

    #[test]
    fn errors_and_warnings_tracked() {
        let mut p = ResourceProfile::new("r".into(), 1000);
        p.record_error();
        p.record_error();
        p.record_warning();
        assert_eq!(p.errors, 2);
        assert_eq!(p.warnings, 1);
    }

    #[test]
    fn network_counters_accumulate() {
        let mut p = ResourceProfile::new("r".into(), 1000);
        p.record_network(100, 50);
        p.record_network(10, 5);
        assert_eq!(p.bytes_sent, 110);
        assert_eq!(p.bytes_recv, 55);
    }

    #[test]
    fn snapshot_is_serializable() {
        let mut prof = Profiler::new(1000);
        prof.observe("r", 42);
        let snap = prof.snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("p50_us"));
        assert!(json.contains("\"r\""));
    }

    #[test]
    fn single_sample_percentile() {
        let mut p = ResourceProfile::new("r".into(), 1000);
        p.record(777);
        let s = p.stats();
        assert_eq!(s.p50_us, 777);
        assert_eq!(s.p99_us, 777);
    }
}
