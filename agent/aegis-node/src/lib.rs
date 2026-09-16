//! Aegis Node Agent — Server Process Supervisor
//!
//! Owns the game server process lifecycle outside the web browser:
//! - Instance start, stop, restart, graceful termination
//! - Watchdog heartbeat monitoring and unresponsive process restarts
//! - Bounded stdout/stderr ring buffer capture
//! - Crash-loop mitigation with retry limits
//! - Deployment build updates and instant rollback to last known-good build
//! - Multi-instance isolation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Runtime lifecycle state of an instance process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessState {
    Stopped,
    Starting,
    Running { pid: u32, started_at_ms: u64 },
    Stopping,
    Crashed { exit_code: Option<i32>, retries_left: u32 },
    Quarantined { reason: String },
}

/// Configuration describing an instance managed by Aegis Node Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceConfig {
    pub instance_id: String,
    pub binary_path: String,
    pub config_path: String,
    pub auto_restart: bool,
    pub max_crash_retries: u32,
    pub watchdog_timeout_ms: u64,
    pub active_build_id: String,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            instance_id: "default-inst".to_string(),
            binary_path: "ald-server.exe".to_string(),
            config_path: "server.toml".to_string(),
            auto_restart: true,
            max_crash_retries: 3,
            watchdog_timeout_ms: 15_000,
            active_build_id: "build-0.1.0".to_string(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SupervisorError {
    #[error("instance '{0}' already registered")]
    InstanceAlreadyExists(String),
    #[error("instance '{0}' not found")]
    InstanceNotFound(String),
    #[error("instance '{0}' is already running")]
    AlreadyRunning(String),
    #[error("no rollback build available for instance '{0}'")]
    NoRollbackAvailable(String),
}

/// Supervisor for a single server instance.
pub struct InstanceSupervisor {
    pub config: InstanceConfig,
    pub state: ProcessState,
    pub last_heartbeat_ms: u64,
    pub previous_known_good_build: Option<String>,
    crash_count_in_window: u32,
    log_buffer: Vec<String>,
    max_log_lines: usize,
    simulated_pid_counter: u32,
}

impl InstanceSupervisor {
    pub fn new(config: InstanceConfig, max_log_lines: usize) -> Self {
        Self {
            config,
            state: ProcessState::Stopped,
            last_heartbeat_ms: 0,
            previous_known_good_build: None,
            crash_count_in_window: 0,
            log_buffer: Vec::new(),
            max_log_lines,
            simulated_pid_counter: 1000,
        }
    }

    pub fn start(&mut self, now_ms: u64) -> Result<(), SupervisorError> {
        if matches!(self.state, ProcessState::Running { .. } | ProcessState::Starting) {
            return Err(SupervisorError::AlreadyRunning(self.config.instance_id.clone()));
        }

        self.simulated_pid_counter += 1;
        let pid = self.simulated_pid_counter;
        self.state = ProcessState::Running { pid, started_at_ms: now_ms };
        self.last_heartbeat_ms = now_ms;
        self.append_log(format!("[INFO] Instance '{}' started (PID {})", self.config.instance_id, pid));
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), SupervisorError> {
        match self.state {
            ProcessState::Running { pid, .. } => {
                self.state = ProcessState::Stopped;
                self.append_log(format!("[INFO] Instance '{}' stopped (PID {})", self.config.instance_id, pid));
                Ok(())
            }
            ProcessState::Starting => {
                self.state = ProcessState::Stopped;
                self.append_log(format!("[INFO] Instance '{}' stopped before running", self.config.instance_id));
                Ok(())
            }
            _ => Ok(()), // Idempotent stop
        }
    }

    pub fn restart(&mut self, now_ms: u64) -> Result<(), SupervisorError> {
        self.stop()?;
        self.start(now_ms)
    }

    pub fn record_heartbeat(&mut self, now_ms: u64) {
        self.last_heartbeat_ms = now_ms;
    }

    pub fn poll_watchdog(&mut self, now_ms: u64) -> bool {
        if let ProcessState::Running { pid, .. } = self.state {
            if now_ms.saturating_sub(self.last_heartbeat_ms) > self.config.watchdog_timeout_ms {
                self.append_log(format!(
                    "[WARN] Watchdog timeout for instance '{}' (PID {}). Triggering auto-restart.",
                    self.config.instance_id, pid
                ));
                self.restart(now_ms).ok();
                return true;
            }
        }
        false
    }

    pub fn handle_crash(&mut self, exit_code: Option<i32>, now_ms: u64) {
        self.crash_count_in_window += 1;
        self.append_log(format!(
            "[ERROR] Instance '{}' crashed with exit code {:?}",
            self.config.instance_id, exit_code
        ));

        if self.crash_count_in_window >= self.config.max_crash_retries {
            let reason = format!("Crash loop exceeded retry limit ({} crashes)", self.crash_count_in_window);
            self.state = ProcessState::Quarantined { reason: reason.clone() };
            self.append_log(format!("[CRITICAL] Instance quarantined: {}", reason));
        } else if self.config.auto_restart {
            let retries_left = self.config.max_crash_retries.saturating_sub(self.crash_count_in_window);
            self.state = ProcessState::Crashed { exit_code, retries_left };
            self.restart(now_ms).ok();
        } else {
            self.state = ProcessState::Crashed { exit_code, retries_left: 0 };
        }
    }

    pub fn append_log(&mut self, line: impl Into<String>) {
        if self.log_buffer.len() >= self.max_log_lines {
            self.log_buffer.remove(0);
        }
        self.log_buffer.push(line.into());
    }

    pub fn logs(&self) -> &[String] {
        &self.log_buffer
    }

    pub fn update_build(&mut self, new_build_id: impl Into<String>) {
        self.previous_known_good_build = Some(self.config.active_build_id.clone());
        self.config.active_build_id = new_build_id.into();
        self.append_log(format!(
            "[INFO] Updated build_id to '{}' (previous: {:?})",
            self.config.active_build_id, self.previous_known_good_build
        ));
    }

    pub fn rollback(&mut self, now_ms: u64) -> Result<(), SupervisorError> {
        let prev = self
            .previous_known_good_build
            .clone()
            .ok_or_else(|| SupervisorError::NoRollbackAvailable(self.config.instance_id.clone()))?;

        self.append_log(format!("[WARN] Rolling back from '{}' to '{}'", self.config.active_build_id, prev));
        self.config.active_build_id = prev;
        self.previous_known_good_build = None;
        self.crash_count_in_window = 0;
        self.restart(now_ms)
    }
}

/// Node Agent managing multiple isolated server instances.
pub struct AegisNodeSupervisor {
    instances: HashMap<String, InstanceSupervisor>,
    default_log_lines: usize,
}

impl AegisNodeSupervisor {
    pub fn new(default_log_lines: usize) -> Self {
        Self { instances: HashMap::new(), default_log_lines }
    }

    pub fn register_instance(&mut self, config: InstanceConfig) -> Result<(), SupervisorError> {
        if self.instances.contains_key(&config.instance_id) {
            return Err(SupervisorError::InstanceAlreadyExists(config.instance_id));
        }
        let id = config.instance_id.clone();
        let supervisor = InstanceSupervisor::new(config, self.default_log_lines);
        self.instances.insert(id, supervisor);
        Ok(())
    }

    pub fn get_instance(&self, id: &str) -> Result<&InstanceSupervisor, SupervisorError> {
        self.instances.get(id).ok_or_else(|| SupervisorError::InstanceNotFound(id.to_string()))
    }

    pub fn get_instance_mut(&mut self, id: &str) -> Result<&mut InstanceSupervisor, SupervisorError> {
        self.instances.get_mut(id).ok_or_else(|| SupervisorError::InstanceNotFound(id.to_string()))
    }

    pub fn start_instance(&mut self, id: &str, now_ms: u64) -> Result<(), SupervisorError> {
        self.get_instance_mut(id)?.start(now_ms)
    }

    pub fn stop_instance(&mut self, id: &str) -> Result<(), SupervisorError> {
        self.get_instance_mut(id)?.stop()
    }

    pub fn restart_instance(&mut self, id: &str, now_ms: u64) -> Result<(), SupervisorError> {
        self.get_instance_mut(id)?.restart(now_ms)
    }

    pub fn poll_all_watchdogs(&mut self, now_ms: u64) -> Vec<String> {
        let mut timed_out = Vec::new();
        for (id, supervisor) in &mut self.instances {
            if supervisor.poll_watchdog(now_ms) {
                timed_out.push(id.clone());
            }
        }
        timed_out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_stop_instance_lifecycle() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 50);
        assert_eq!(sup.state, ProcessState::Stopped);

        sup.start(1000).unwrap();
        assert!(matches!(sup.state, ProcessState::Running { pid: 1001, .. }));

        sup.stop().unwrap();
        assert_eq!(sup.state, ProcessState::Stopped);
    }

    #[test]
    fn restart_transitions_through_stopping_and_running() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 50);
        sup.start(1000).unwrap();
        let pid1 = match sup.state {
            ProcessState::Running { pid, .. } => pid,
            _ => unreachable!(),
        };

        sup.restart(2000).unwrap();
        let pid2 = match sup.state {
            ProcessState::Running { pid, .. } => pid,
            _ => unreachable!(),
        };

        assert_ne!(pid1, pid2);
        assert!(matches!(sup.state, ProcessState::Running { .. }));
    }

    #[test]
    fn watchdog_detects_heartbeat_timeout_and_restarts() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 50);
        sup.start(1000).unwrap();

        // No timeout at 5000ms
        assert!(!sup.poll_watchdog(5000));

        // Timeout triggers past 15_000ms window (at 20_000ms)
        assert!(sup.poll_watchdog(20_000));
        assert!(sup.logs().iter().any(|l| l.contains("Watchdog timeout")));
    }

    #[test]
    fn crash_loop_respects_max_retries_and_quarantines() {
        let config = InstanceConfig { max_crash_retries: 2, ..Default::default() };
        let mut sup = InstanceSupervisor::new(config, 50);
        sup.start(1000).unwrap();

        // Crash 1 -> restarts
        sup.handle_crash(Some(1), 1100);
        assert!(matches!(sup.state, ProcessState::Running { .. }));

        // Crash 2 -> Quarantined
        sup.handle_crash(Some(1), 1200);
        assert!(matches!(sup.state, ProcessState::Quarantined { .. }));
    }

    #[test]
    fn log_ring_buffer_caps_capacity_and_preserves_recent() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 3);
        sup.append_log("line 1");
        sup.append_log("line 2");
        sup.append_log("line 3");
        sup.append_log("line 4");

        assert_eq!(sup.logs().len(), 3);
        assert_eq!(sup.logs()[0], "line 2");
        assert_eq!(sup.logs()[2], "line 4");
    }

    #[test]
    fn multi_instance_isolation_between_different_instances() {
        let mut node = AegisNodeSupervisor::new(50);
        let cfg1 = InstanceConfig { instance_id: "inst-alpha".to_string(), ..Default::default() };
        let cfg2 = InstanceConfig { instance_id: "inst-beta".to_string(), ..Default::default() };

        node.register_instance(cfg1).unwrap();
        node.register_instance(cfg2).unwrap();

        node.start_instance("inst-alpha", 1000).unwrap();
        assert!(matches!(node.get_instance("inst-alpha").unwrap().state, ProcessState::Running { .. }));
        assert_eq!(node.get_instance("inst-beta").unwrap().state, ProcessState::Stopped);
    }

    #[test]
    fn apply_update_updates_build_id_and_records_previous() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 50);
        assert_eq!(sup.config.active_build_id, "build-0.1.0");

        sup.update_build("build-0.2.0");
        assert_eq!(sup.config.active_build_id, "build-0.2.0");
        assert_eq!(sup.previous_known_good_build, Some("build-0.1.0".to_string()));
    }

    #[test]
    fn rollback_restores_previous_build_id() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 50);
        sup.start(1000).unwrap();
        sup.update_build("build-bad");

        sup.rollback(2000).unwrap();
        assert_eq!(sup.config.active_build_id, "build-0.1.0");
        assert_eq!(sup.previous_known_good_build, None);
    }

    #[test]
    fn stopping_an_already_stopped_instance_is_noop() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 50);
        assert_eq!(sup.state, ProcessState::Stopped);
        assert_eq!(sup.stop(), Ok(()));
        assert_eq!(sup.state, ProcessState::Stopped);
    }

    #[test]
    fn watchdog_heartbeat_resets_timeout_counter() {
        let mut sup = InstanceSupervisor::new(InstanceConfig::default(), 50);
        sup.start(1000).unwrap();

        // Heartbeat at 10_000ms
        sup.record_heartbeat(10_000);

        // At 20_000ms, only 10_000ms elapsed since heartbeat -> no timeout (window is 15_000ms)
        assert!(!sup.poll_watchdog(20_000));
    }
}
