//! Lifecycle supervisor — owns the LifecycleManager as a single writer.
//!
//! Resource state transitions are funneled through one task so that
//! LifecycleManager mutation never crosses an await point under a lock and
//! consoles/Aegis/hot-reload can request transitions concurrently.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::state::ServerState;
use crate::LifecycleCommand;

pub struct LifecycleSupervisor {
    state: Arc<ServerState>,
}

impl LifecycleSupervisor {
    pub fn new(state: Arc<ServerState>) -> Self {
        LifecycleSupervisor { state }
    }

    /// Process lifecycle commands until the channel closes.
    pub async fn run(self, mut rx: mpsc::UnboundedReceiver<LifecycleCommand>) {
        while let Some(cmd) = rx.recv().await {
            if *self.state.shutdown_rx().borrow() {
                tracing::debug!("shutdown active; ignoring lifecycle command");
                continue;
            }
            match cmd {
                LifecycleCommand::Start(name) => self.transition_start(&name).await,
                LifecycleCommand::Stop(name) => self.transition_stop(&name).await,
                LifecycleCommand::Restart(name) => self.transition_restart(&name).await,
                LifecycleCommand::Reload(name) => self.transition_restart(&name).await,
            }
        }
    }

    async fn transition_start(&self, name: &str) {
        let mut l = self.state.lifecycle().lock().await;
        if !l.can_start(name) {
            tracing::warn!(resource = name, "start rejected: not in Stopped/Failed");
            return;
        }
        l.set(name, ald_resource::ResourceState::Starting);
        // ponytail: script execution not yet wired; the runtime will
        // populate real handle counts when script hosts are integrated.
        match l.mark_running(name, 0) {
            Ok(()) => {
                self.state.metrics().incr("resources.started", 1);
                tracing::info!(resource = name, "resource started");
            }
            Err(e) => {
                l.set(name, ald_resource::ResourceState::Failed);
                self.state.metrics().incr("resources.start_failures", 1);
                tracing::error!(resource = name, error = %e, "resource failed to start");
            }
        }
    }

    async fn transition_stop(&self, name: &str) {
        let mut l = self.state.lifecycle().lock().await;
        l.set(name, ald_resource::ResourceState::Stopping);
        match l.stop(name) {
            Ok(()) => {
                self.state.metrics().incr("resources.stopped", 1);
                tracing::info!(resource = name, "resource stopped");
            }
            Err(e) => {
                l.set(name, ald_resource::ResourceState::Failed);
                self.state.metrics().incr("resources.stop_failures", 1);
                tracing::error!(resource = name, error = %e, "resource leaked handles on stop");
            }
        }
    }

    async fn transition_restart(&self, name: &str) {
        self.transition_stop(name).await;
        self.transition_start(name).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_config::Config;

    fn dummy() -> (Arc<ServerState>, mpsc::UnboundedSender<LifecycleCommand>, LifecycleSupervisor) {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let state = Arc::new(ServerState::new(Config::default(), rx));
        let (tx, rx2) = mpsc::unbounded_channel();
        let supervisor = LifecycleSupervisor::new(Arc::clone(&state));
        // Drop rx2 to keep the channel quiescent; supervisor gets its own.
        drop(rx2);
        (state, tx, supervisor)
    }

    #[tokio::test]
    async fn start_stop_roundtrip() {
        let (state, _tx, supervisor) = dummy();
        let (tx2, rx2) = mpsc::unbounded_channel();
        let handle = tokio::spawn(async move { supervisor.run(rx2).await });
        tx2.send(LifecycleCommand::Start("test-res".into())).unwrap();
        // Allow the supervisor task to process.
        for _ in 0..20 {
            if state.running_count().await == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(state.running_count().await, 1);

        tx2.send(LifecycleCommand::Stop("test-res".into())).unwrap();
        for _ in 0..20 {
            if state.running_count().await == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(state.running_count().await, 0);
        drop(tx2);
        handle.await.unwrap();
    }
}
