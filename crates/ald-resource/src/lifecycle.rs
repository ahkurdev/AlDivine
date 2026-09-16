use std::collections::HashMap;

use ald_core::AldError;

/// Full resource lifecycle. A resource is serving players only in HEALTHY
/// (or DEGRADED with a recorded reason) — scripts must actually load and the
/// health gate must pass. There is no shortcut from STARTING to serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceState {
    Stopped,
    Discovered,
    Validating,
    DependencyResolution,
    Starting,
    ScriptHostStarting,
    MigrationsReady,
    HealthChecking,
    Healthy,
    Degraded,
    Quarantined,
    Stopping,
    Failed,
}

impl ResourceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceState::Stopped => "STOPPED",
            ResourceState::Discovered => "DISCOVERED",
            ResourceState::Validating => "VALIDATING",
            ResourceState::DependencyResolution => "DEPENDENCY_RESOLUTION",
            ResourceState::Starting => "STARTING",
            ResourceState::ScriptHostStarting => "SCRIPT_HOST_STARTING",
            ResourceState::MigrationsReady => "MIGRATIONS_READY",
            ResourceState::HealthChecking => "HEALTH_CHECKING",
            ResourceState::Healthy => "HEALTHY",
            ResourceState::Degraded => "DEGRADED",
            ResourceState::Quarantined => "QUARANTINED",
            ResourceState::Stopping => "STOPPING",
            ResourceState::Failed => "FAILED",
        }
    }

    /// Serving traffic (fully or with a recorded degradation).
    pub fn is_serving(&self) -> bool {
        matches!(self, ResourceState::Healthy | ResourceState::Degraded)
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

    /// Transition to Healthy after the health gate passes; records handle count.
    pub fn mark_healthy(&mut self, name: &str, handle_count: usize) -> Result<(), AldError> {
        if self.state(name) != ResourceState::HealthChecking {
            return Err(AldError::Resource(format!(
                "resource '{name}' not in HealthChecking state before mark_healthy"
            )));
        }
        self.set(name, ResourceState::Healthy);
        self.handles.insert(name.to_string(), handle_count);
        Ok(())
    }

    /// Transition to Degraded: serving with a recorded reason. Records handle
    /// count like the healthy path.
    pub fn mark_degraded(&mut self, name: &str, handle_count: usize) -> Result<(), AldError> {
        if self.state(name) != ResourceState::HealthChecking {
            return Err(AldError::Resource(format!(
                "resource '{name}' not in HealthChecking state before mark_degraded"
            )));
        }
        self.set(name, ResourceState::Degraded);
        self.handles.insert(name.to_string(), handle_count);
        Ok(())
    }

    /// Move a resource out of service for inspection. Only a serving or failed
    /// resource can be quarantined; it returns via start after review.
    pub fn quarantine(&mut self, name: &str) -> Result<(), AldError> {
        match self.state(name) {
            ResourceState::Healthy | ResourceState::Degraded | ResourceState::Failed => {
                self.set(name, ResourceState::Quarantined);
                Ok(())
            }
            other => Err(AldError::Resource(format!("resource '{name}' in {} cannot be quarantined", other.as_str()))),
        }
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

    /// Number of resources currently serving (Healthy or Degraded).
    pub fn running_count(&self) -> usize {
        self.states.values().filter(|s| s.is_serving()).count()
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
        lm.set("r", ResourceState::HealthChecking);
        lm.mark_healthy("r", 3).unwrap();
        assert_eq!(lm.state("r"), ResourceState::Healthy);
        assert!(lm.state("r").is_serving());
        lm.release_handle("r");
        lm.release_handle("r");
        lm.release_handle("r");
        lm.stop("r").unwrap();
        assert_eq!(lm.state("r"), ResourceState::Stopped);
    }

    #[test]
    fn healthy_requires_health_checking() {
        let mut lm = LifecycleManager::new();
        lm.set("r", ResourceState::Starting);
        assert!(lm.mark_healthy("r", 0).is_err());
        assert!(lm.mark_degraded("r", 0).is_err());
    }

    #[test]
    fn degraded_serves() {
        let mut lm = LifecycleManager::new();
        lm.set("r", ResourceState::HealthChecking);
        lm.mark_degraded("r", 1).unwrap();
        assert!(lm.state("r").is_serving());
        assert_eq!(lm.running_count(), 1);
    }

    #[test]
    fn quarantine_roundtrip() {
        let mut lm = LifecycleManager::new();
        assert!(lm.quarantine("fresh").is_err());
        lm.set("r", ResourceState::HealthChecking);
        lm.mark_healthy("r", 0).unwrap();
        lm.quarantine("r").unwrap();
        assert_eq!(lm.state("r"), ResourceState::Quarantined);
        assert!(!lm.state("r").is_serving());
        assert_eq!(lm.running_count(), 0);
    }

    #[test]
    fn leaked_handles_block_stop() {
        let mut lm = LifecycleManager::new();
        lm.set("r", ResourceState::HealthChecking);
        lm.mark_healthy("r", 2).unwrap();
        lm.release_handle("r");
        assert!(lm.stop("r").is_err());
    }
}
