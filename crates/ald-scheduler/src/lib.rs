//! Aldivine Scheduler — classifies workloads, enforces per-resource budgets,
//! and detects runaway resources (infinite loops, event floods, memory growth).
//!
//! This synchronous core implements the policy/budget engine and is tested
//! without Tokio. The async executor (Tokio/rayon binding) is added in a later
//! phase; the Scheduler API is executor-agnostic.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub mod budget;
pub mod detector;
pub mod executor;
pub mod workload;

pub use budget::{Budget, BudgetUsage, BudgetViolation};
pub use detector::{RunawayDetector, RunawayKind};
pub use executor::{BudgetTable, Executor, QUEUE_CAPACITY};
pub use workload::WorkloadClass;

/// Default tick rate for ordered gameplay work.
pub const DEFAULT_TICK_MS: u64 = 16;

/// A unit of scheduled work. `work` runs under the chosen executor.
pub struct Task {
    pub resource: String,
    pub class: WorkloadClass,
    pub work: Box<dyn FnOnce() -> ald_core::Result<()> + Send>,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task").field("resource", &self.resource).field("class", &self.class).finish()
    }
}

/// Per-resource accounting state used by budget + detector logic.
#[derive(Debug, Clone, Default)]
pub struct ResourceStats {
    pub cpu_time_ms: u64,
    pub exec_time_ms: u64,
    pub memory_bytes: u64,
    pub events_sec: u64,
    pub net_events_sec: u64,
    pub db_queries_sec: u64,
    pub tasks_spawned: u64,
}

/// Central scheduler state. Holds budgets and live stats per resource.
#[derive(Debug, Default)]
pub struct Scheduler {
    budgets: HashMap<String, Budget>,
    usage: HashMap<String, BudgetUsage>,
    stats: HashMap<String, ResourceStats>,
    detector: RunawayDetector,
    started: Option<Instant>,
}

impl Scheduler {
    pub fn new() -> Self {
        Scheduler::default()
    }

    pub fn start(&mut self) {
        self.started = Some(Instant::now());
    }

    /// Register a resource with its budget. Defaults applied for missing fields.
    pub fn register_resource(&mut self, name: &str, budget: Budget) {
        self.budgets.insert(name.to_string(), budget);
        self.usage.insert(name.to_string(), BudgetUsage::default());
        self.stats.insert(name.to_string(), ResourceStats::default());
    }

    pub fn budget(&self, name: &str) -> Option<&Budget> {
        self.budgets.get(name)
    }

    pub fn stats(&self, name: &str) -> Option<&ResourceStats> {
        self.stats.get(name)
    }

    /// Record a completed task's measured cost and validate against budget.
    /// Returns Err(BudgetViolation) if the resource exceeded its limits.
    pub fn account_task(
        &mut self,
        name: &str,
        cpu_ms: u64,
        mem_delta: i64,
        events: u64,
    ) -> Result<(), BudgetViolation> {
        let usage = self.usage.entry(name.to_string()).or_default();
        usage.cpu_time_ms += cpu_ms;
        usage.task_count += 1;
        usage.events += events;
        if mem_delta >= 0 {
            usage.memory_bytes += mem_delta as u64;
        } else {
            usage.memory_bytes = usage.memory_bytes.saturating_sub((-mem_delta) as u64);
        }
        let stats = self.stats.entry(name.to_string()).or_default();
        stats.cpu_time_ms += cpu_ms;
        if mem_delta >= 0 {
            stats.memory_bytes += mem_delta as u64;
        } else {
            stats.memory_bytes = stats.memory_bytes.saturating_sub((-mem_delta) as u64);
        }
        stats.events_sec += events;
        stats.tasks_spawned += 1;

        if let Some(budget) = self.budgets.get(name) {
            return budget.check(usage);
        }
        Ok(())
    }

    /// Run the runaway detector over current stats. Returns detected kinds.
    pub fn check_runaway(&mut self, name: &str) -> Vec<RunawayKind> {
        let stats = self.stats.get(name).cloned().unwrap_or_default();
        let budget = self.budgets.get(name).cloned().unwrap_or_default();
        self.detector.inspect(name, &stats, &budget)
    }

    /// Elapsed scheduler uptime.
    pub fn uptime(&self) -> Option<Duration> {
        self.started.map(|s| s.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_enforced() {
        let mut s = Scheduler::new();
        s.register_resource(
            "r",
            Budget {
                max_cpu_time_ms: 100,
                max_exec_time_ms: 0,
                max_memory_bytes: 1_000_000,
                max_events_per_sec: 0,
                max_net_events_per_sec: 0,
                max_db_queries_per_sec: 0,
                max_tasks: 0,
            },
        );
        assert!(s.account_task("r", 50, 1000, 0).is_ok());
        // Third task pushes cpu over 100ms.
        assert!(s.account_task("r", 60, 1000, 0).is_err());
    }

    #[test]
    fn runaway_event_flood_detected() {
        let mut s = Scheduler::new();
        s.register_resource(
            "flooder",
            Budget {
                max_cpu_time_ms: 0,
                max_exec_time_ms: 0,
                max_memory_bytes: 0,
                max_events_per_sec: 100,
                max_net_events_per_sec: 0,
                max_db_queries_per_sec: 0,
                max_tasks: 0,
            },
        );
        // Simulate 500 events in the window.
        let mut _st = ResourceStats::default();
        s.stats.insert("flooder".to_string(), ResourceStats { events_sec: 500, ..Default::default() });
        let kinds = s.check_runaway("flooder");
        assert!(kinds.contains(&RunawayKind::EventFlood));
    }

    #[test]
    fn scheduler_uptime_ticks() {
        let mut s = Scheduler::new();
        s.start();
        std::thread::sleep(Duration::from_millis(2));
        assert!(s.uptime().unwrap() >= Duration::from_millis(2));
    }
}
