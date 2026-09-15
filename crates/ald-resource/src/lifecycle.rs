use std::collections::HashMap;

use ald_core::AldError;

/// Finite resource states with cleanup on transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
    Restarting,
}

impl ResourceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceState::Stopped => "STOPPED",
            ResourceState::Starting => "STARTING",
            ResourceState::Running => "RUNNING",
            ResourceState::Stopping => "STOPPING",
            ResourceState::Failed => "FAILED",
            ResourceState::Restarting => "RESTARTING",
        }
    }
}

/// Tracks per-resource state and detects invalid transitions / leaked handles.
#[derive(Default)]
pub struct LifecycleManager {
    states: HashMap<String, ResourceState>,
    /// handle counts tracked for leak detection
    handles: HashMap<String, usize>,
}

impl LifecycleManager {
    pub fn new() -> Self {
        LifecycleManager::default()
    }

    pub fn state(&self, name: &str) -> ResourceState {
        *self.states.get(name).unwrap_or(&ResourceState::Stopped)
    }

    pub fn set(&mut self, name: &str, state: ResourceState) {
        self.states.insert(name.to_string(), state);
    }

    pub fn can_start(&self, name: &str) -> bool {
        matches!(self.state(name), ResourceState::Stopped | ResourceState::Failed)
    }

    /// Transition to Running after successful start; records handle count.
    pub fn mark_running(&mut self, name: &str, handle_count: usize) -> Result<(), AldError> {
        if self.state(name) != ResourceState::Starting {
            return Err(AldError::Resource(format!("resource '{name}' not in Starting state before mark_running")));
        }
        self.set(name, ResourceState::Running);
        self.handles.insert(name.to_string(), handle_count);
        Ok(())
    }

    /// Stop a resource, verifying all handles were released.
    pub fn stop(&mut self, name: &str) -> Result<(), AldError> {
        let leaked = self.handles.get(name).copied().unwrap_or(0);
        if leaked != 0 {
            return Err(AldError::Resource(format!("resource '{name}' leaked {leaked} handles on stop")));
        }
        self.set(name, ResourceState::Stopped);
        Ok(())
    }

    pub fn release_handle(&mut self, name: &str) {
        if let Some(c) = self.handles.get_mut(name) {
            if *c > 0 {
                *c -= 1;
            }
        }
    }

    /// Number of resources currently in the Running state.
    pub fn running_count(&self) -> usize {
        self.states.values().filter(|s| **s == ResourceState::Running).count()
    }

    /// Names of all known resources, sorted for deterministic output.
    pub fn resource_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.states.keys().cloned().collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_happy_path() {
        let mut lm = LifecycleManager::new();
        lm.set("r", ResourceState::Starting);
        lm.mark_running("r", 3).unwrap();
        assert_eq!(lm.state("r"), ResourceState::Running);
        lm.release_handle("r");
        lm.release_handle("r");
        lm.release_handle("r");
        lm.stop("r").unwrap();
        assert_eq!(lm.state("r"), ResourceState::Stopped);
    }

    #[test]
    fn leaked_handles_block_stop() {
        let mut lm = LifecycleManager::new();
        lm.set("r", ResourceState::Starting);
        lm.mark_running("r", 2).unwrap();
        lm.release_handle("r");
        assert!(lm.stop("r").is_err());
    }
}
