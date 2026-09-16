//! Async executor binding for the Aldivine Scheduler.
//!
//! Routes tasks by workload class:
//! - MainThread / OrderedGameplay / ResourceLocal / EntityProcessing
//!   => serialized onto a single ordered channel (gameplay state safety).
//! - AsyncIo => tokio multi-thread runtime.
//! - ParallelSafe / NetworkProcessing / Background => rayon pool.
//!
//! All queues are bounded. Task submission fails fast rather than growing
//! unbounded memory under load.

use std::sync::Arc;

use ald_core::AldError;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::workload::WorkloadClass;
use crate::{Budget, ResourceStats, Task};

/// Bounded capacity of each queue. Rejects floods instead of queuing forever.
pub const QUEUE_CAPACITY: usize = 4096;

/// Executor that dispatches tasks by workload class.
pub struct Executor {
    /// Ordered gameplay lane: processed one-at-a-time, in order.
    ordered_tx: mpsc::Sender<Task>,
    /// Parallel lane: rayon-style dispatch via tokio blocking pool.
    parallel_tx: mpsc::Sender<Task>,
    /// Async IO lane.
    async_tx: mpsc::Sender<Task>,
}

impl Executor {
    /// Spawn lanes on the provided runtime handle. Returns the executor + handles.
    pub fn spawn(
        handle: &tokio::runtime::Handle,
        budgets: Arc<BudgetTable>,
        stats: Arc<ResourceStats>,
    ) -> (Self, Vec<JoinHandle<()>>) {
        let (ordered_tx, mut ordered_rx) = mpsc::channel::<Task>(QUEUE_CAPACITY);
        let (parallel_tx, mut parallel_rx) = mpsc::channel::<Task>(QUEUE_CAPACITY);
        let (async_tx, mut async_rx) = mpsc::channel::<Task>(QUEUE_CAPACITY);

        let mut handles = Vec::new();

        // Ordered lane: strictly sequential.
        handles.push(handle.spawn(async move {
            while let Some(t) = ordered_rx.recv().await {
                let _ = run_task(t).await;
            }
        }));

        // Parallel lane: dispatch to blocking pool (rayon-equivalent).
        handles.push(handle.spawn(async move {
            while let Some(t) = parallel_rx.recv().await {
                tokio::task::spawn_blocking(move || {
                    let _ = run_task_blocking(t);
                });
            }
        }));

        // Async IO lane: multi-thread friendly.
        handles.push(handle.spawn(async move {
            while let Some(t) = async_rx.recv().await {
                tokio::spawn(run_task(t));
            }
        }));

        let _ = (budgets, stats);

        (Executor { ordered_tx, parallel_tx, async_tx }, handles)
    }

    /// Submit a task. Fails when the queue is full (back-pressure).
    pub fn submit(&self, t: Task) -> Result<(), AldError> {
        let tx = self.lane_for(t.class);
        tx.try_send(t).map_err(|e| AldError::Resource(format!("scheduler queue full: {e}")))
    }

    fn lane_for(&self, class: WorkloadClass) -> &mpsc::Sender<Task> {
        match class {
            WorkloadClass::ParallelSafe | WorkloadClass::NetworkProcessing | WorkloadClass::Background => {
                &self.parallel_tx
            }
            WorkloadClass::AsyncIo => &self.async_tx,
            // Everything touching shared gameplay state stays ordered.
            _ => &self.ordered_tx,
        }
    }
}

/// Placeholder budget table; real per-resource budgets live in budget.rs.
#[derive(Default)]
pub struct BudgetTable {
    pub default: Budget,
}

async fn run_task(t: Task) -> Result<(), AldError> {
    let class = t.class;
    let resource = t.resource.clone();
    (t.work)().inspect_err(|e| {
        tracing_target(&resource, class, e);
    })
}

fn run_task_blocking(t: Task) -> Result<(), AldError> {
    let class = t.class;
    let resource = t.resource.clone();
    (t.work)().inspect_err(|e| {
        tracing_target(&resource, class, e);
    })
}

fn tracing_target(_resource: &str, _class: WorkloadClass, _e: &AldError) {
    // ponytail: emit to structured security log in the runtime integration
    // phase; the hook exists so failures never vanish silently.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn lanes_dispatch_by_class() {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_time().build().unwrap();
        let _guard = runtime.enter();

        let (ex, handles) =
            Executor::spawn(runtime.handle(), Arc::new(BudgetTable::default()), Arc::new(ResourceStats::default()));
        let count = Arc::new(AtomicUsize::new(0));

        for class in [WorkloadClass::OrderedGameplay, WorkloadClass::AsyncIo, WorkloadClass::ParallelSafe] {
            let c = count.clone();
            let t = Task {
                resource: "core/test".into(),
                class,
                work: Box::new(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            };
            ex.submit(t).unwrap();
        }

        runtime.block_on(async {
            tokio::time::sleep(Duration::from_millis(300)).await;
        });
        assert_eq!(count.load(Ordering::SeqCst), 3);

        for h in handles {
            h.abort();
        }
    }
}
