use serde::{Deserialize, Serialize};

/// Workload classification. Do NOT execute every script on a random thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkloadClass {
    /// Game loop / ordered state mutation. Single-threaded ordering required.
    MainThread,
    /// Async IO: network, file, db. Runs on async executor.
    AsyncIo,
    /// CPU-bound, no shared mutable game state. Parallel-safe.
    ParallelSafe,
    /// Ordered gameplay logic that must not race.
    OrderedGameplay,
    /// Scoped to a single resource's local state.
    ResourceLocal,
    /// Network packet processing.
    NetworkProcessing,
    /// Entity simulation/update.
    EntityProcessing,
    /// Low-priority housekeeping.
    Background,
}

impl WorkloadClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkloadClass::MainThread => "MAIN_THREAD",
            WorkloadClass::AsyncIo => "ASYNC_IO",
            WorkloadClass::ParallelSafe => "PARALLEL_SAFE",
            WorkloadClass::OrderedGameplay => "ORDERED_GAMEPLAY",
            WorkloadClass::ResourceLocal => "RESOURCE_LOCAL",
            WorkloadClass::NetworkProcessing => "NETWORK_PROCESSING",
            WorkloadClass::EntityProcessing => "ENTITY_PROCESSING",
            WorkloadClass::Background => "BACKGROUND",
        }
    }

    /// Whether this class may be dispatched across worker threads.
    pub fn parallel_eligible(&self) -> bool {
        matches!(self, WorkloadClass::ParallelSafe | WorkloadClass::Background | WorkloadClass::NetworkProcessing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_eligibility() {
        assert!(WorkloadClass::ParallelSafe.parallel_eligible());
        assert!(!WorkloadClass::OrderedGameplay.parallel_eligible());
        assert!(WorkloadClass::NetworkProcessing.parallel_eligible());
    }
}
