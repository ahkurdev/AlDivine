//! Asset mounting: verified files become registered mounts.
//!
//! The mount pipeline walks each asset through an explicit state machine:
//!
//! ```text
//! Queued -> Verifying -> Verified -> DependenciesReady -> MountPending
//!        -> Mounted -> Registered
//! ```
//!
//! with terminal [`MountState::FailedRetryable`], [`MountState::FailedFatal`],
//! and [`MountState::Cancelled`]. Every transition is a named method; jumps
//! outside the diagram are [`MountError::BadTransition`], not silent skips.
//! Terminal states stick — nothing leaves them.
//!
//! Gate order is load-bearing:
//! 1. `queue`: the filename must be a recognized format
//!    ([`ald_asset_registry`]) — unknown formats are refused at the door,
//!    never queued.
//! 2. `verify`: bytes in [`ald_cache`] must hash-match. Mismatch is
//!    retryable (re-fetch fixes it), never fatal.
//! 3. `check_dependencies`: every declared DataFile must resolve in
//!    [`ald_datafiles`]. Missing entries are retryable (packs arrive
//!    later), unknown-everywhere is still retryable — the registry grows,
//!    the mount waits.
//! 4. `begin_mount` / `complete_mount` / `register`: the mount point is
//!    recorded, then the asset is registered for residency handoff.
//!
//! What this crate does NOT do: touch the game. The game-thread commit and
//! residency handshake live in `ald-residency`; this crate ends at
//! `Registered`, ready for handoff.

use std::collections::HashMap;

use ald_asset_registry::{FormatRegistry, Recognition, StreamClass};
use thiserror::Error;

/// Mount lifecycle state. Terminal: both Failed*, Cancelled, Registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountState {
    Queued,
    Verifying,
    Verified,
    DependenciesReady,
    MountPending,
    Mounted,
    Registered,
    FailedRetryable { reason: String },
    FailedFatal { reason: String },
    Cancelled,
}

impl MountState {
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            MountState::Registered
                | MountState::FailedRetryable { .. }
                | MountState::FailedFatal { .. }
                | MountState::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MountError {
    #[error("unknown asset '{0}'")]
    UnknownAsset(String),
    #[error("unrecognized format for '{0}'")]
    UnknownFormat(String),
    #[error("illegal transition: {op} from {from:?}")]
    BadTransition { from: MountState, op: &'static str },
    #[error("terminal state, no further transitions")]
    Terminal,
}

/// One asset's mount record.
#[derive(Debug, Clone)]
pub struct MountRecord {
    pub asset_id: String,
    pub file_name: String,
    pub class: StreamClass,
    pub expected_hash: Option<String>,
    pub dependencies: Vec<String>,
    pub state: MountState,
    pub mount_point: Option<String>,
}

/// Mount pipeline over caller-owned registries and cache handles.
///
/// Cache access goes through two injected closures so this crate stays free
/// of filesystem and cache types: `verify` answers "do these bytes hash to
/// `expected`", groking nothing about storage. Tests inject fakes; the real
/// wiring passes `ald_cache` calls.
pub struct Mounter {
    records: HashMap<String, MountRecord>,
}

impl Mounter {
    pub fn new() -> Self {
        Mounter { records: HashMap::new() }
    }

    /// Queue an asset. Unknown formats are refused here, not queued.
    pub fn queue(
        &mut self,
        asset_id: &str,
        file_name: &str,
        formats: &FormatRegistry,
        expected_hash: Option<String>,
        dependencies: Vec<String>,
    ) -> Result<(), MountError> {
        let class = match formats.recognize(file_name) {
            Recognition::Known(f) => f.class,
            Recognition::Unknown { .. } => return Err(MountError::UnknownFormat(file_name.to_string())),
        };
        if self.records.contains_key(asset_id) {
            return Err(MountError::BadTransition { from: self.records[asset_id].state.clone(), op: "re-queue" });
        }
        self.records.insert(
            asset_id.to_string(),
            MountRecord {
                asset_id: asset_id.to_string(),
                file_name: file_name.to_string(),
                class,
                expected_hash,
                dependencies,
                state: MountState::Queued,
                mount_point: None,
            },
        );
        Ok(())
    }

    fn record_mut(&mut self, asset_id: &str) -> Result<&mut MountRecord, MountError> {
        self.records.get_mut(asset_id).ok_or_else(|| MountError::UnknownAsset(asset_id.to_string()))
    }

    fn check(state: &MountState, want: &MountState, op: &'static str) -> Result<(), MountError> {
        if state.terminal() {
            return Err(MountError::Terminal);
        }
        if std::mem::discriminant(state) != std::mem::discriminant(want) {
            return Err(MountError::BadTransition { from: state.clone(), op });
        }
        Ok(())
    }

    /// Queued -> Verifying.
    pub fn begin_verify(&mut self, asset_id: &str) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        Self::check(&rec.state, &MountState::Queued, "begin_verify")?;
        rec.state = MountState::Verifying;
        Ok(())
    }

    /// Verifying -> Verified (hash matched) or FailedRetryable (mismatch).
    /// `hash_ok` is the caller's cache verdict for `expected_hash`.
    pub fn finish_verify(&mut self, asset_id: &str, hash_ok: bool, detail: &str) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        Self::check(&rec.state, &MountState::Verifying, "finish_verify")?;
        rec.state = if hash_ok {
            MountState::Verified
        } else {
            MountState::FailedRetryable { reason: format!("hash mismatch: {detail}") }
        };
        Ok(())
    }

    /// Verified -> DependenciesReady, or FailedRetryable listing the missing
    /// entries. `resolve` answers datafile membership (the caller's registry).
    pub fn check_dependencies(&mut self, asset_id: &str, resolve: impl Fn(&str) -> bool) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        Self::check(&rec.state, &MountState::Verified, "check_dependencies")?;
        let missing: Vec<&str> = rec.dependencies.iter().map(String::as_str).filter(|d| !resolve(d)).collect();
        rec.state = if missing.is_empty() {
            MountState::DependenciesReady
        } else {
            MountState::FailedRetryable { reason: format!("missing datafiles: {}", missing.join(", ")) }
        };
        Ok(())
    }

    /// DependenciesReady -> MountPending, recording where it will mount.
    pub fn begin_mount(&mut self, asset_id: &str, mount_point: &str) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        Self::check(&rec.state, &MountState::DependenciesReady, "begin_mount")?;
        rec.mount_point = Some(mount_point.to_string());
        rec.state = MountState::MountPending;
        Ok(())
    }

    /// MountPending -> Mounted (the game-thread commit happened elsewhere
    /// and reported success through the caller).
    pub fn complete_mount(&mut self, asset_id: &str) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        Self::check(&rec.state, &MountState::MountPending, "complete_mount")?;
        rec.state = MountState::Mounted;
        Ok(())
    }

    /// Mounted -> Registered: handoff to the residency tracker.
    pub fn register(&mut self, asset_id: &str) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        Self::check(&rec.state, &MountState::Mounted, "register")?;
        rec.state = MountState::Registered;
        Ok(())
    }

    /// Cancel from any non-terminal state.
    pub fn cancel(&mut self, asset_id: &str) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        if rec.state.terminal() {
            return Err(MountError::Terminal);
        }
        rec.state = MountState::Cancelled;
        Ok(())
    }

    /// Fail a non-terminal record with an operator reason.
    pub fn fail(&mut self, asset_id: &str, reason: &str, retryable: bool) -> Result<(), MountError> {
        let rec = self.record_mut(asset_id)?;
        if rec.state.terminal() {
            return Err(MountError::Terminal);
        }
        rec.state = if retryable {
            MountState::FailedRetryable { reason: reason.to_string() }
        } else {
            MountState::FailedFatal { reason: reason.to_string() }
        };
        Ok(())
    }

    pub fn state_of(&self, asset_id: &str) -> Option<&MountState> {
        self.records.get(asset_id).map(|r| &r.state)
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

impl Default for Mounter {
    fn default() -> Self {
        Mounter::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_datafiles::DataFileRegistry;

    fn formats() -> FormatRegistry {
        FormatRegistry::with_spec_seed()
    }

    fn queue_car(m: &mut Mounter) {
        m.queue("car1", "sportscar.yft", &formats(), Some("abc".into()), vec!["vehicles.meta".into()]).unwrap();
    }

    #[test]
    fn happy_path_to_registered() {
        let mut m = Mounter::new();
        queue_car(&mut m);
        let datafiles = DataFileRegistry::with_spec_seed();
        m.begin_verify("car1").unwrap();
        m.finish_verify("car1", true, "").unwrap();
        m.check_dependencies("car1", |d| datafiles.lookup(d).is_some()).unwrap();
        m.begin_mount("car1", "vehicles:/sportscar").unwrap();
        assert_eq!(m.records["car1"].mount_point.as_deref(), Some("vehicles:/sportscar"));
        m.complete_mount("car1").unwrap();
        m.register("car1").unwrap();
        assert_eq!(m.state_of("car1"), Some(&MountState::Registered));
    }

    #[test]
    fn unknown_format_refused_at_queue() {
        let mut m = Mounter::new();
        assert_eq!(
            m.queue("x", "notes.txt", &formats(), None, vec![]),
            Err(MountError::UnknownFormat("notes.txt".into()))
        );
        assert_eq!(m.record_count(), 0);
    }

    #[test]
    fn hash_mismatch_is_retryable() {
        let mut m = Mounter::new();
        queue_car(&mut m);
        m.begin_verify("car1").unwrap();
        m.finish_verify("car1", false, "expected abc, got def").unwrap();
        assert!(matches!(m.state_of("car1"), Some(MountState::FailedRetryable { .. })));
    }

    #[test]
    fn missing_datafile_is_retryable_with_names() {
        let mut m = Mounter::new();
        m.queue("mlo1", "office.ymap", &formats(), None, vec!["nonexistent.meta".into()]).unwrap();
        let datafiles = DataFileRegistry::with_spec_seed();
        m.begin_verify("mlo1").unwrap();
        m.finish_verify("mlo1", true, "").unwrap();
        m.check_dependencies("mlo1", |d| datafiles.lookup(d).is_some()).unwrap();
        match m.state_of("mlo1") {
            Some(MountState::FailedRetryable { reason }) => assert!(reason.contains("nonexistent.meta")),
            other => panic!("expected retryable, got {other:?}"),
        }
    }

    #[test]
    fn illegal_jumps_rejected() {
        let mut m = Mounter::new();
        queue_car(&mut m);
        // Skip straight to mount: rejected.
        assert!(matches!(m.begin_mount("car1", "x"), Err(MountError::BadTransition { op: "begin_mount", .. })));
        assert!(matches!(m.register("car1"), Err(MountError::BadTransition { .. })));
        assert!(matches!(m.complete_mount("car1"), Err(MountError::BadTransition { .. })));
        // Unknown asset everywhere.
        assert_eq!(m.begin_verify("ghost"), Err(MountError::UnknownAsset("ghost".into())));
        assert_eq!(m.state_of("ghost"), None);
    }

    #[test]
    fn double_queue_rejected() {
        let mut m = Mounter::new();
        queue_car(&mut m);
        assert!(matches!(
            m.queue("car1", "other.yft", &formats(), None, vec![]),
            Err(MountError::BadTransition { .. })
        ));
    }

    #[test]
    fn cancel_and_fail_paths() {
        let mut m = Mounter::new();
        queue_car(&mut m);
        m.cancel("car1").unwrap();
        assert_eq!(m.state_of("car1"), Some(&MountState::Cancelled));
        // Terminal: nothing further.
        assert_eq!(m.cancel("car1"), Err(MountError::Terminal));
        assert_eq!(m.begin_verify("car1"), Err(MountError::Terminal));

        let mut m = Mounter::new();
        queue_car(&mut m);
        m.fail("car1", "disk gone", false).unwrap();
        assert!(matches!(m.state_of("car1"), Some(MountState::FailedFatal { .. })));
        assert_eq!(m.fail("car1", "again", true), Err(MountError::Terminal));
    }

    #[test]
    fn class_recorded_from_registry() {
        let mut m = Mounter::new();
        m.queue("tex", "body.ytd", &formats(), None, vec![]).unwrap();
        assert_eq!(m.records["tex"].class, StreamClass::Texture);
    }
}
