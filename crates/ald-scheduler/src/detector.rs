use serde::{Deserialize, Serialize};

use crate::budget::Budget;
use crate::ResourceStats;

/// Categories of runaway behavior the scheduler must catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunawayKind {
    InfiniteLoop,
    EventFlood,
    MemoryGrowth,
    SlowHandler,
    UnboundedTasks,
}

/// Heuristic runaway detector. Operates on a snapshot of resource stats.
/// Thresholds are intentionally simple; tuning happens via benchmarks later.
#[derive(Debug, Default)]
pub struct RunawayDetector {
    event_flood_threshold: u64,
    memory_growth_threshold: u64,
    slow_handler_ms: u64,
}

impl RunawayDetector {
    pub fn new() -> Self {
        RunawayDetector {
            event_flood_threshold: 1000,
            memory_growth_threshold: 512 * 1024 * 1024,
            slow_handler_ms: 250,
        }
    }

    pub fn inspect(&self, _name: &str, stats: &ResourceStats, budget: &Budget) -> Vec<RunawayKind> {
        let mut out = Vec::new();
        if stats.events_sec > self.event_flood_threshold {
            out.push(RunawayKind::EventFlood);
        }
        if stats.memory_bytes > self.memory_growth_threshold
            && budget.max_memory_bytes != 0
            && stats.memory_bytes > budget.max_memory_bytes
        {
            out.push(RunawayKind::MemoryGrowth);
        }
        if stats.exec_time_ms as u64 > self.slow_handler_ms && budget.max_exec_time_ms == 0 {
            out.push(RunawayKind::SlowHandler);
        }
        if budget.max_tasks != 0 && stats.tasks_spawned > budget.max_tasks * 4 {
            out.push(RunawayKind::UnboundedTasks);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceStats;

    fn stats(events: u64, mem: u64, exec: u64, tasks: u64) -> ResourceStats {
        ResourceStats {
            cpu_time_ms: 0,
            exec_time_ms: exec,
            memory_bytes: mem,
            events_sec: events,
            net_events_sec: 0,
            db_queries_sec: 0,
            tasks_spawned: tasks,
        }
    }

    #[test]
    fn detects_flood_and_slow() {
        let d = RunawayDetector::new();
        let s = stats(5000, 100, 400, 10);
        let kinds = d.inspect("r", &s, &Budget::default());
        assert!(kinds.contains(&RunawayKind::EventFlood));
        assert!(kinds.contains(&RunawayKind::SlowHandler));
    }
}
