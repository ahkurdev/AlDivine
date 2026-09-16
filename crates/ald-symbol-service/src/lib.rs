//! Aldivine Symbol Service, Crash Fingerprinting & Safe Mode
//!
//! Provides minidump/core-dump parsing metadata, address symbolication,
//! deterministic crash fingerprinting, crash-loop detection, and
//! automated Safe Mode isolation with operator recovery steps.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;

/// Operating system / platform source of crash dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DumpPlatform {
    WindowsMinidump,
    LinuxCoreDump,
    GenericPanic,
}

/// Raw crash dump metadata received from client or server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawCrashDump {
    pub report_id: String,
    pub platform: DumpPlatform,
    pub build_id: String,
    pub artifact_id: String,
    pub game_edition: String,
    pub game_build: u32,
    pub resource_context: Option<String>,
    pub fault_address: u64,
    pub raw_frames: Vec<u64>,
    pub panic_message: Option<String>,
}

impl RawCrashDump {
    /// Compute a deterministic crash fingerprint from top frames and context.
    pub fn compute_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.build_id.as_bytes());
        if let Some(ref res) = self.resource_context {
            hasher.update(res.as_bytes());
        }
        for frame in self.raw_frames.iter().take(5) {
            hasher.update(frame.to_le_bytes());
        }
        format!("{:x}", hasher.finalize())[..16].to_string()
    }
}

/// Resolved debug symbol information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub symbol_name: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// A symbolicated stack frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolicatedFrame {
    pub address: u64,
    pub symbol: Option<SymbolInfo>,
}

/// Complete symbolicated crash report ready for diagnostics or Aegis view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolicatedCrashReport {
    pub report_id: String,
    pub fingerprint: String,
    pub platform: DumpPlatform,
    pub resource_context: Option<String>,
    pub fault_address: u64,
    pub frames: Vec<SymbolicatedFrame>,
    pub panic_message: Option<String>,
}

/// Symbol table for a specific build artifact.
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    symbols: BTreeMap<u64, SymbolInfo>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, address: u64, info: SymbolInfo) {
        self.symbols.insert(address, info);
    }

    pub fn resolve(&self, address: u64) -> Option<&SymbolInfo> {
        self.symbols.get(&address)
    }
}

#[derive(Debug, Error)]
pub enum SymbolError {
    #[error("build symbols not found for build_id: {0}")]
    BuildNotFound(String),
}

/// Service managing symbol tables and symbolication.
#[derive(Default)]
pub struct SymbolService {
    tables: HashMap<String, SymbolTable>,
}

impl SymbolService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_table(&mut self, build_id: impl Into<String>, table: SymbolTable) {
        self.tables.insert(build_id.into(), table);
    }

    pub fn symbolicate(&self, raw: &RawCrashDump) -> Result<SymbolicatedCrashReport, SymbolError> {
        let table = self.tables.get(&raw.build_id).ok_or_else(|| SymbolError::BuildNotFound(raw.build_id.clone()))?;

        let mut frames = Vec::with_capacity(raw.raw_frames.len());
        for &addr in &raw.raw_frames {
            let symbol = table.resolve(addr).cloned();
            frames.push(SymbolicatedFrame { address: addr, symbol });
        }

        Ok(SymbolicatedCrashReport {
            report_id: raw.report_id.clone(),
            fingerprint: raw.compute_fingerprint(),
            platform: raw.platform,
            resource_context: raw.resource_context.clone(),
            fault_address: raw.fault_address,
            frames,
            panic_message: raw.panic_message.clone(),
        })
    }
}

/// Safe mode recovery status and recommendations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeModeState {
    pub is_active: bool,
    pub reason: String,
    pub culprit_resources: Vec<String>,
    pub recovery_steps: Vec<String>,
}

/// Supervisor monitoring crash loops to trigger Safe Mode.
pub struct SafeModeDetector {
    crash_history: Vec<(u64, RawCrashDump)>,
    crash_threshold: usize,
    window_ms: u64,
}

impl SafeModeDetector {
    pub fn new(crash_threshold: usize, window_ms: u64) -> Self {
        Self { crash_history: Vec::new(), crash_threshold, window_ms }
    }

    /// Record a crash event at virtual or real timestamp `timestamp_ms`.
    pub fn record_crash(&mut self, crash: RawCrashDump, timestamp_ms: u64) {
        self.crash_history.push((timestamp_ms, crash));
        // Evict expired entries outside the sliding window
        let cutoff = timestamp_ms.saturating_sub(self.window_ms);
        self.crash_history.retain(|(t, _)| *t >= cutoff);
    }

    /// Evaluate whether system is in a crash loop and needs Safe Mode.
    pub fn evaluate(&self) -> Option<SafeModeState> {
        if self.crash_history.len() < self.crash_threshold {
            return None;
        }

        let mut resource_counts: HashMap<String, usize> = HashMap::new();
        for (_, crash) in &self.crash_history {
            if let Some(ref res) = crash.resource_context {
                *resource_counts.entry(res.clone()).or_default() += 1;
            }
        }

        let mut culprit_resources = Vec::new();
        for (res, count) in resource_counts {
            if count > self.crash_threshold / 2 {
                culprit_resources.push(res);
            }
        }
        culprit_resources.sort();

        let recovery_steps = if !culprit_resources.is_empty() {
            vec![
                format!("Quarantine or disable culprit resource(s): {}", culprit_resources.join(", ")),
                "Inspect resource script logs and memory allocations".to_string(),
                "Restart ald-server in SAFE_MODE with isolated resources".to_string(),
            ]
        } else {
            vec![
                "Revert to last known-good runtime artifact build".to_string(),
                "Check system dependencies and database connection limits".to_string(),
                "Restart server daemon in SAFE_MODE".to_string(),
            ]
        };

        Some(SafeModeState {
            is_active: true,
            reason: format!("Crash loop detected: {} crashes within window", self.crash_history.len()),
            culprit_resources,
            recovery_steps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample_dump(id: &str, res: Option<&str>, frames: Vec<u64>) -> RawCrashDump {
        RawCrashDump {
            report_id: id.to_string(),
            platform: DumpPlatform::WindowsMinidump,
            build_id: "build-101".to_string(),
            artifact_id: "art-x86_64".to_string(),
            game_edition: "Enhanced".to_string(),
            game_build: 3095,
            resource_context: res.map(|s| s.to_string()),
            fault_address: 0x7fff_0001_0000,
            raw_frames: frames,
            panic_message: None,
        }
    }

    #[test]
    fn symbolication_resolves_known_addresses() {
        let mut service = SymbolService::new();
        let mut table = SymbolTable::new();
        table.insert(
            0x1000,
            SymbolInfo {
                symbol_name: "ald_resource::dispatch".to_string(),
                file: Some("src/dispatch.rs".to_string()),
                line: Some(42),
            },
        );
        service.register_table("build-101", table);

        let dump = make_sample_dump("dump-1", Some("chat"), vec![0x1000]);
        let rep = service.symbolicate(&dump).unwrap();

        assert_eq!(rep.frames.len(), 1);
        let sym = rep.frames[0].symbol.as_ref().unwrap();
        assert_eq!(sym.symbol_name, "ald_resource::dispatch");
        assert_eq!(sym.line, Some(42));
    }

    #[test]
    fn symbolication_handles_unknown_addresses_gracefully() {
        let mut service = SymbolService::new();
        service.register_table("build-101", SymbolTable::new());

        let dump = make_sample_dump("dump-2", None, vec![0x9999]);
        let rep = service.symbolicate(&dump).unwrap();

        assert_eq!(rep.frames.len(), 1);
        assert_eq!(rep.frames[0].symbol, None);
    }

    #[test]
    fn fingerprint_is_deterministic_and_deduplicates_crashes() {
        let d1 = make_sample_dump("dump-a", Some("spawn"), vec![0x10, 0x20, 0x30]);
        let d2 = make_sample_dump("dump-b", Some("spawn"), vec![0x10, 0x20, 0x30]);

        assert_eq!(d1.compute_fingerprint(), d2.compute_fingerprint());
    }

    #[test]
    fn different_crash_stacks_produce_different_fingerprints() {
        let d1 = make_sample_dump("dump-a", Some("spawn"), vec![0x10, 0x20]);
        let d2 = make_sample_dump("dump-b", Some("spawn"), vec![0x90, 0x99]);

        assert_ne!(d1.compute_fingerprint(), d2.compute_fingerprint());
    }

    #[test]
    fn safe_mode_activates_on_repeated_crashes_in_window() {
        let mut detector = SafeModeDetector::new(3, 60_000);
        let d1 = make_sample_dump("d1", None, vec![0x10]);
        let d2 = make_sample_dump("d2", None, vec![0x10]);
        let d3 = make_sample_dump("d3", None, vec![0x10]);

        detector.record_crash(d1, 1000);
        assert!(detector.evaluate().is_none());

        detector.record_crash(d2, 2000);
        assert!(detector.evaluate().is_none());

        detector.record_crash(d3, 3000);
        let state = detector.evaluate().unwrap();
        assert!(state.is_active);
    }

    #[test]
    fn safe_mode_does_not_activate_for_isolated_crashes() {
        let mut detector = SafeModeDetector::new(3, 10_000);
        let d1 = make_sample_dump("d1", None, vec![0x10]);
        let d2 = make_sample_dump("d2", None, vec![0x10]);

        detector.record_crash(d1, 1000);
        // Crash 2 happens way after the window
        detector.record_crash(d2, 50_000);

        assert!(detector.evaluate().is_none());
    }

    #[test]
    fn safe_mode_isolates_culprit_resource() {
        let mut detector = SafeModeDetector::new(3, 60_000);
        let d1 = make_sample_dump("d1", Some("bad-script"), vec![0x10]);
        let d2 = make_sample_dump("d2", Some("bad-script"), vec![0x10]);
        let d3 = make_sample_dump("d3", Some("bad-script"), vec![0x10]);

        detector.record_crash(d1, 1000);
        detector.record_crash(d2, 2000);
        detector.record_crash(d3, 3000);

        let state = detector.evaluate().unwrap();
        assert_eq!(state.culprit_resources, vec!["bad-script"]);
        assert!(state.recovery_steps[0].contains("bad-script"));
    }

    #[test]
    fn safe_mode_provides_actionable_recovery_steps() {
        let mut detector = SafeModeDetector::new(2, 60_000);
        let d1 = make_sample_dump("d1", None, vec![0x10]);
        let d2 = make_sample_dump("d2", None, vec![0x10]);

        detector.record_crash(d1, 1000);
        detector.record_crash(d2, 2000);

        let state = detector.evaluate().unwrap();
        assert!(!state.recovery_steps.is_empty());
        assert!(state.recovery_steps.iter().any(|s| s.contains("last known-good")));
    }

    #[test]
    fn safe_mode_resets_after_grace_period() {
        let mut detector = SafeModeDetector::new(2, 5_000);
        let d1 = make_sample_dump("d1", None, vec![0x10]);
        let d2 = make_sample_dump("d2", None, vec![0x10]);

        detector.record_crash(d1, 1000);
        detector.record_crash(d2, 2000);
        assert!(detector.evaluate().is_some());

        // New crash comes much later, old ones pruned
        let d3 = make_sample_dump("d3", None, vec![0x10]);
        detector.record_crash(d3, 20_000);

        assert!(detector.evaluate().is_none());
    }

    #[test]
    fn multi_platform_dump_ingestion() {
        let d_win = RawCrashDump {
            report_id: "w-1".to_string(),
            platform: DumpPlatform::WindowsMinidump,
            build_id: "b-1".to_string(),
            artifact_id: "art-win".to_string(),
            game_edition: "Enhanced".to_string(),
            game_build: 3095,
            resource_context: None,
            fault_address: 0x1234,
            raw_frames: vec![0x1234],
            panic_message: None,
        };

        let d_linux = RawCrashDump {
            report_id: "l-1".to_string(),
            platform: DumpPlatform::LinuxCoreDump,
            build_id: "b-1".to_string(),
            artifact_id: "art-linux".to_string(),
            game_edition: "Legacy".to_string(),
            game_build: 2699,
            resource_context: None,
            fault_address: 0x5678,
            raw_frames: vec![0x5678],
            panic_message: None,
        };

        assert_eq!(d_win.platform, DumpPlatform::WindowsMinidump);
        assert_eq!(d_linux.platform, DumpPlatform::LinuxCoreDump);
    }
}
