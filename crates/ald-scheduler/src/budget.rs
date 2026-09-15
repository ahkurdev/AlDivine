use serde::{Deserialize, Serialize};

/// Per-resource execution budget. 0 = unlimited for that dimension.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Budget {
    pub max_cpu_time_ms: u64,
    pub max_exec_time_ms: u64,
    pub max_memory_bytes: u64,
    pub max_events_per_sec: u64,
    pub max_net_events_per_sec: u64,
    pub max_db_queries_per_sec: u64,
    pub max_tasks: u64,
}

/// Accumulated usage for budget checks. Reset on reload/restart.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BudgetUsage {
    pub cpu_time_ms: u64,
    pub exec_time_ms: u64,
    pub memory_bytes: u64,
    pub events: u64,
    pub net_events: u64,
    pub db_queries: u64,
    pub task_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetViolation {
    CpuTime,
    ExecTime,
    Memory,
    Events,
    NetEvents,
    DbQueries,
    TaskCount,
}

impl std::fmt::Display for BudgetViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BudgetViolation::CpuTime => "cpu_time_exceeded",
            BudgetViolation::ExecTime => "exec_time_exceeded",
            BudgetViolation::Memory => "memory_exceeded",
            BudgetViolation::Events => "events_per_sec_exceeded",
            BudgetViolation::NetEvents => "net_events_per_sec_exceeded",
            BudgetViolation::DbQueries => "db_queries_per_sec_exceeded",
            BudgetViolation::TaskCount => "task_count_exceeded",
        };
        write!(f, "{s}")
    }
}

impl Budget {
    /// Check accumulated usage against limits. Returns the first exceeded dim.
    pub fn check(&self, u: &BudgetUsage) -> Result<(), BudgetViolation> {
        if self.max_cpu_time_ms != 0 && u.cpu_time_ms > self.max_cpu_time_ms {
            return Err(BudgetViolation::CpuTime);
        }
        if self.max_exec_time_ms != 0 && u.exec_time_ms > self.max_exec_time_ms {
            return Err(BudgetViolation::ExecTime);
        }
        if self.max_memory_bytes != 0 && u.memory_bytes > self.max_memory_bytes {
            return Err(BudgetViolation::Memory);
        }
        if self.max_events_per_sec != 0 && u.events > self.max_events_per_sec {
            return Err(BudgetViolation::Events);
        }
        if self.max_net_events_per_sec != 0 && u.net_events > self.max_net_events_per_sec {
            return Err(BudgetViolation::NetEvents);
        }
        if self.max_db_queries_per_sec != 0 && u.db_queries > self.max_db_queries_per_sec {
            return Err(BudgetViolation::DbQueries);
        }
        if self.max_tasks != 0 && u.task_count > self.max_tasks {
            return Err(BudgetViolation::TaskCount);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_means_unlimited() {
        let b = Budget::default();
        let u = BudgetUsage { cpu_time_ms: u64::MAX, ..Default::default() };
        assert!(b.check(&u).is_ok());
    }

    #[test]
    fn memory_violation() {
        let b = Budget { max_memory_bytes: 100, ..Default::default() };
        let u = BudgetUsage { memory_bytes: 200, ..Default::default() };
        assert_eq!(b.check(&u), Err(BudgetViolation::Memory));
    }
}
