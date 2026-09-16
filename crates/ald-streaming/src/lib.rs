//! Streaming orchestration: one driven path across the proven pieces.
//!
//! `download` moves bytes, `cache` keeps verified objects, `mount`
//! registers them, `residency` waits for the game to hold them. This crate
//! owns nothing but coordination: a unified asset view, progress accounting,
//! stall aggregation, and the join gate.
//!
//! Driven entirely on caller ticks and caller bytes — no threads, no hidden
//! timers, no network. The two game touchpoints stay explicit handoffs:
//! `complete_mount` (the game-thread commit happened and reported back) and
//! the [`GameCommit`](ald_residency::GameCommit) bridge passed into
//! residency calls. The orchestrator never pretends those happened.
//!
//! Per-asset flow through this API:
//!
//! ```text
//! add_asset -> receive_chunk* -> finalize_download (cache commit)
//!           -> mount_verify -> mount_check_deps -> mount_begin
//!           -> complete_mount (caller: game commit done)
//!           -> mount_register -> residency_request -> residency_poll*
//!           -> Ready
//! ```
//!
//! [`StreamingEngine::asset_state`] folds the four trackers into one
//! [`AssetState`]; [`StreamingEngine::join_ready`] gates join on a required
//! set; [`StreamingEngine::overall_progress`] reports bytes across assets.

use std::collections::HashMap;

use ald_asset_registry::FormatRegistry;
use ald_cache::ContentCache;
use ald_datafiles::DataFileRegistry;
use ald_download::{Download, DownloadError, Lane, LaneScheduler, Watchdog};
use ald_mount::{MountError, MountState, Mounter};
use ald_residency::{GameCommit, ResidencyError, ResidencyState, ResidencyTracker};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StreamError {
    #[error("unknown asset '{0}'")]
    UnknownAsset(String),
    #[error("asset '{0}' already tracked")]
    Duplicate(String),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Mount(#[from] MountError),
    #[error(transparent)]
    Residency(#[from] ResidencyError),
    #[error(transparent)]
    Cache(#[from] ald_cache::CacheError),
}

/// One asset's unified state across all four trackers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetState {
    Downloading { received: u64, total: u64 },
    Stalled { received: u64, total: u64 },
    Caching,
    Mounting { stage: String },
    Residency { stage: String },
    Ready,
    Failed { reason: String },
}

struct AssetMeta {
    expected_hash: String,
    lane: Lane,
    download: Download,
    cached: bool,
}

/// The orchestrator. Owns registries, scheduler, and all per-asset state.
pub struct StreamingEngine {
    formats: FormatRegistry,
    datafiles: DataFileRegistry,
    lanes: LaneScheduler,
    downloads_planned: HashMap<String, AssetMeta>,
    mounter: Mounter,
    residency: ResidencyTracker,
    watchdog: Watchdog,
    job_seq: u64,
}

impl StreamingEngine {
    pub fn new(watchdog: Watchdog) -> Self {
        StreamingEngine {
            formats: FormatRegistry::with_spec_seed(),
            datafiles: DataFileRegistry::with_spec_seed(),
            lanes: LaneScheduler::new(),
            downloads_planned: HashMap::new(),
            mounter: Mounter::new(),
            residency: ResidencyTracker::new(),
            watchdog,
            job_seq: 0,
        }
    }

    pub fn formats(&self) -> &FormatRegistry {
        &self.formats
    }

    pub fn datafiles(&self) -> &DataFileRegistry {
        &self.datafiles
    }

    pub fn datafiles_mut(&mut self) -> &mut DataFileRegistry {
        &mut self.datafiles
    }

    /// Track a new asset: mount queue (refuses unknown formats here),
    /// download plan, and one lane job. `file_len`/`chunk_len` follow
    /// [`ald_download`] rules.
    #[allow(clippy::too_many_arguments)]
    pub fn add_asset(
        &mut self,
        asset_id: &str,
        file_name: &str,
        file_len: u64,
        chunk_len: u64,
        expected_hash: String,
        dependencies: Vec<String>,
        lane: Lane,
        now_tick: u64,
    ) -> Result<(), StreamError> {
        if self.downloads_planned.contains_key(asset_id) {
            return Err(StreamError::Duplicate(asset_id.to_string()));
        }
        let plan = ald_download::ChunkPlan::new(file_len, chunk_len)?;
        self.mounter.queue(asset_id, file_name, &self.formats, Some(expected_hash.clone()), dependencies)?;
        self.job_seq += 1;
        let job = self.job_seq;
        self.lanes.enqueue(lane, job);
        self.downloads_planned.insert(
            asset_id.to_string(),
            AssetMeta { expected_hash, lane, download: Download::new(plan, now_tick), cached: false },
        );
        Ok(())
    }

    /// Feed received bytes into an asset's download.
    pub fn receive_chunk(
        &mut self,
        asset_id: &str,
        offset: u64,
        data: &[u8],
        now_tick: u64,
    ) -> Result<(), StreamError> {
        let meta =
            self.downloads_planned.get_mut(asset_id).ok_or_else(|| StreamError::UnknownAsset(asset_id.to_string()))?;
        meta.download.receive(offset, data, now_tick)?;
        Ok(())
    }

    /// Stall report over incomplete downloads: `(asset_id, stalled)`.
    pub fn poll_stalls(&self, now_tick: u64) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = self
            .downloads_planned
            .iter()
            .filter(|(_, m)| !m.download.is_complete())
            .map(|(id, m)| (id.clone(), self.watchdog.is_stalled(m.download.last_progress_tick(), now_tick)))
            .collect();
        out.sort();
        out
    }

    /// Commit a completed download into the cache. Fails when incomplete or
    /// when bytes hash-mismatch (the cache removes the temp and errors).
    pub fn finalize_download(&mut self, asset_id: &str, cache: &mut ContentCache) -> Result<(), StreamError> {
        let meta =
            self.downloads_planned.get_mut(asset_id).ok_or_else(|| StreamError::UnknownAsset(asset_id.to_string()))?;
        let bytes = meta.download.assembled().ok_or(DownloadError::OutOfBounds {
            offset: 0,
            len: 0,
            file_len: meta.download.plan().file_len,
        })?;
        let staged = cache.stage(bytes)?;
        cache.commit(staged, &meta.expected_hash)?;
        meta.cached = true;
        Ok(())
    }

    /// Run the mount verify gate using the cache's verdict.
    pub fn mount_verify(&mut self, asset_id: &str, cache: &mut ContentCache) -> Result<(), StreamError> {
        let expected = self
            .downloads_planned
            .get(asset_id)
            .ok_or_else(|| StreamError::UnknownAsset(asset_id.to_string()))?
            .expected_hash
            .clone();
        self.mounter.begin_verify(asset_id)?;
        let ok = cache.verify(&expected).unwrap_or(false);
        self.mounter.finish_verify(asset_id, ok, &format!("cache verdict for {expected}"))?;
        Ok(())
    }

    /// Run the dependency gate against the engine's datafile registry.
    pub fn mount_check_deps(&mut self, asset_id: &str) -> Result<(), StreamError> {
        self.mounter.check_dependencies(asset_id, |d| self.datafiles.lookup(d).is_some())?;
        Ok(())
    }

    pub fn mount_begin(&mut self, asset_id: &str, mount_point: &str) -> Result<(), StreamError> {
        self.mounter.begin_mount(asset_id, mount_point)?;
        Ok(())
    }

    /// Handoff: the caller confirms the game-thread commit happened.
    pub fn complete_mount(&mut self, asset_id: &str) -> Result<(), StreamError> {
        self.mounter.complete_mount(asset_id)?;
        Ok(())
    }

    pub fn mount_register(&mut self, asset_id: &str) -> Result<(), StreamError> {
        self.mounter.register(asset_id)?;
        Ok(())
    }

    pub fn residency_request(
        &mut self,
        asset_id: &str,
        now_tick: u64,
        timeout_ticks: u64,
        max_retries: u32,
        commit: &mut impl GameCommit,
    ) -> Result<ResidencyState, StreamError> {
        Ok(self.residency.request(asset_id, now_tick, timeout_ticks, max_retries, commit)?)
    }

    pub fn residency_poll(
        &mut self,
        asset_id: &str,
        now_tick: u64,
        commit: &mut impl GameCommit,
    ) -> Result<ResidencyState, StreamError> {
        Ok(self.residency.poll(asset_id, now_tick, commit)?)
    }

    /// Unified state across trackers. Mount/residency failures surface with
    /// their reasons; a stalled download reports `Stalled`, not `Downloading`.
    pub fn asset_state(&self, asset_id: &str, now_tick: u64) -> Result<AssetState, StreamError> {
        let meta =
            self.downloads_planned.get(asset_id).ok_or_else(|| StreamError::UnknownAsset(asset_id.to_string()))?;
        if let Some(res) = self.residency.state_of(asset_id) {
            return Ok(match res {
                ResidencyState::Ready => AssetState::Ready,
                ResidencyState::Failed { reason } => AssetState::Failed { reason: reason.clone() },
                ResidencyState::Expired => AssetState::Failed { reason: "residency expired".into() },
                ResidencyState::Cancelled => AssetState::Failed { reason: "residency cancelled".into() },
                ResidencyState::Requested | ResidencyState::Waiting { .. } | ResidencyState::Resident { .. } => {
                    AssetState::Residency { stage: format!("{res:?}") }
                }
            });
        }
        if let Some(mount) = self.mounter.state_of(asset_id) {
            match mount {
                MountState::Registered => {}
                MountState::FailedRetryable { reason } | MountState::FailedFatal { reason } => {
                    return Ok(AssetState::Failed { reason: reason.clone() });
                }
                MountState::Cancelled => return Ok(AssetState::Failed { reason: "mount cancelled".into() }),
                // Non-terminal mount stages report below — but only once the
                // download is done; while bytes are still flowing the asset
                // is Downloading, not "mount queued".
                _ => {}
            }
        }
        if !meta.download.is_complete() {
            let total = meta.download.plan().file_len;
            let received = meta.download.received_bytes();
            if self.watchdog.is_stalled(meta.download.last_progress_tick(), now_tick) {
                return Ok(AssetState::Stalled { received, total });
            }
            return Ok(AssetState::Downloading { received, total });
        }
        if !meta.cached {
            return Ok(AssetState::Caching);
        }
        if let Some(mount) = self.mounter.state_of(asset_id) {
            return Ok(AssetState::Mounting { stage: format!("{mount:?}") });
        }
        Ok(AssetState::Caching)
    }

    /// Overall byte progress across assets: `(received, total)`.
    pub fn overall_progress(&self) -> (u64, u64) {
        self.downloads_planned
            .values()
            .fold((0, 0), |(r, t), m| (r + m.download.received_bytes(), t + m.download.plan().file_len))
    }

    /// Join gate: every required asset `Ready`.
    pub fn join_ready(&self, required: &[&str], now_tick: u64) -> bool {
        required.iter().all(|id| matches!(self.asset_state(id, now_tick), Ok(AssetState::Ready)))
    }

    /// Next scheduled lane job (drives worker assignment).
    pub fn next_lane_job(&mut self) -> Option<(Lane, u64)> {
        self.lanes.pop_next()
    }

    pub fn lane_of(&self, asset_id: &str) -> Option<Lane> {
        self.downloads_planned.get(asset_id).map(|m| m.lane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_cache::sha256_hex;
    use std::collections::VecDeque;

    struct FakeCommit {
        requests: VecDeque<ald_residency::CommitOutcome>,
        confirm: VecDeque<bool>,
    }

    impl GameCommit for FakeCommit {
        fn request_residency(&mut self, _asset_id: &str) -> ald_residency::CommitOutcome {
            self.requests.pop_front().unwrap_or(ald_residency::CommitOutcome::Accepted)
        }

        fn confirm_resident(&mut self, _asset_id: &str) -> bool {
            self.confirm.pop_front().unwrap_or(false)
        }
    }

    fn engine() -> StreamingEngine {
        StreamingEngine::new(Watchdog::new(10))
    }

    fn test_cache_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ald-streaming-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn add_car(e: &mut StreamingEngine, payload: &[u8]) -> String {
        let hash = sha256_hex(payload);
        e.add_asset(
            "car1",
            "sportscar.yft",
            payload.len() as u64,
            4,
            hash.clone(),
            vec!["vehicles.meta".into()],
            Lane::JoinCritical,
            0,
        )
        .unwrap();
        hash
    }

    fn drive_to_ready(e: &mut StreamingEngine, cache: &mut ContentCache, payload: &[u8], bridge: &mut FakeCommit) {
        // Feed all bytes, finalize, mount chain, residency.
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 3).min(payload.len());
            e.receive_chunk("car1", off as u64, &payload[off..end], 1).unwrap();
            off = end;
        }
        e.finalize_download("car1", cache).unwrap();
        e.mount_verify("car1", cache).unwrap();
        e.mount_check_deps("car1").unwrap();
        e.mount_begin("car1", "vehicles:/sportscar").unwrap();
        e.complete_mount("car1").unwrap();
        e.mount_register("car1").unwrap();
        e.residency_request("car1", 2, 10, 2, bridge).unwrap();
        assert_eq!(e.residency_poll("car1", 3, bridge).unwrap(), ResidencyState::Ready);
    }

    #[test]
    fn full_path_to_ready_and_join_gate() {
        let mut e = engine();
        let dir = test_cache_dir("full");
        let mut cache = ContentCache::open(&dir, 1 << 30).unwrap();
        let payload = b"fake-yft-bytes-0123456789";
        add_car(&mut e, payload);
        assert!(!e.join_ready(&["car1"], 1));
        assert!(matches!(e.asset_state("car1", 1), Ok(AssetState::Downloading { received: 0, total: _ })));
        let mut bridge = FakeCommit {
            requests: VecDeque::from([ald_residency::CommitOutcome::Accepted]),
            confirm: VecDeque::from([true]),
        };
        drive_to_ready(&mut e, &mut cache, payload, &mut bridge);
        assert_eq!(e.asset_state("car1", 4), Ok(AssetState::Ready));
        assert!(e.join_ready(&["car1"], 4));
        assert_eq!(e.overall_progress(), (payload.len() as u64, payload.len() as u64));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_format_refused_at_add() {
        let mut e = engine();
        assert!(matches!(
            e.add_asset("x", "notes.txt", 10, 4, "abc".into(), vec![], Lane::Background, 0),
            Err(StreamError::Mount(_))
        ));
        assert!(e.add_asset("x", "notes.txt", 10, 4, "abc".into(), vec![], Lane::Background, 0).is_err());
        assert_eq!(
            e.add_asset("ok", "a.yft", 0, 4, "abc".into(), vec![], Lane::Background, 0),
            Err(StreamError::Download(DownloadError::BadPlan(0, 4)))
        );
    }

    #[test]
    fn duplicate_asset_refused() {
        let mut e = engine();
        add_car(&mut e, b"0123456789");
        assert_eq!(
            e.add_asset("car1", "other.yft", 10, 4, "x".into(), vec![], Lane::Background, 0),
            Err(StreamError::Duplicate("car1".into()))
        );
    }

    #[test]
    fn corrupt_bytes_fail_finalize() {
        let mut e = engine();
        let dir = test_cache_dir("corrupt");
        let mut cache = ContentCache::open(&dir, 1 << 30).unwrap();
        // Declare hash of X but feed Y: cache commit rejects.
        e.add_asset("car1", "sportscar.yft", 8, 4, sha256_hex(b"XXXXXXXX"), vec![], Lane::JoinCritical, 0).unwrap();
        e.receive_chunk("car1", 0, b"YYYYYYYY", 1).unwrap();
        assert!(matches!(e.finalize_download("car1", &mut cache), Err(StreamError::Cache(_))));
        // Download is complete but nothing verified: awaiting finalize retry.
        assert!(matches!(e.asset_state("car1", 1), Ok(AssetState::Caching)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stall_surfaces_in_state() {
        let mut e = engine();
        add_car(&mut e, b"0123456789");
        e.receive_chunk("car1", 0, b"01", 0).unwrap();
        assert!(matches!(e.asset_state("car1", 5), Ok(AssetState::Downloading { .. })));
        assert!(matches!(e.asset_state("car1", 11), Ok(AssetState::Stalled { received: 2, total: 10 })));
        let stalls = e.poll_stalls(11);
        assert_eq!(stalls, vec![("car1".to_string(), true)]);
    }

    #[test]
    fn mount_failure_surfaces_with_reason() {
        let mut e = engine();
        let dir = test_cache_dir("mountfail");
        let mut cache = ContentCache::open(&dir, 1 << 30).unwrap();
        let payload = b"0123456789";
        // Depend on a datafile nobody registered: retryable at the dep gate.
        let hash = sha256_hex(payload);
        e.add_asset("car1", "sportscar.yft", 10, 4, hash, vec!["future.meta".into()], Lane::JoinCritical, 0).unwrap();
        e.receive_chunk("car1", 0, payload, 1).unwrap();
        e.finalize_download("car1", &mut cache).unwrap();
        e.mount_verify("car1", &mut cache).unwrap();
        e.mount_check_deps("car1").unwrap();
        match e.asset_state("car1", 2).unwrap() {
            AssetState::Failed { reason } => assert!(reason.contains("future.meta")),
            other => panic!("expected Failed, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lane_assignment_and_scheduling() {
        let mut e = engine();
        add_car(&mut e, b"0123456789");
        assert_eq!(e.lane_of("car1"), Some(Lane::JoinCritical));
        assert_eq!(e.lane_of("ghost"), None);
        // One job was enqueued at add.
        assert!(e.next_lane_job().is_some());
        assert_eq!(e.next_lane_job(), None);
    }

    #[test]
    fn join_gate_needs_every_asset() {
        let mut e = engine();
        let dir = test_cache_dir("gate");
        let mut cache = ContentCache::open(&dir, 1 << 30).unwrap();
        let payload = b"0123456789";
        add_car(&mut e, payload);
        let hash2 = sha256_hex(payload);
        e.add_asset("car2", "coupe.yft", 10, 4, hash2, vec![], Lane::Foreground, 0).unwrap();
        let mut bridge = FakeCommit {
            requests: VecDeque::from([ald_residency::CommitOutcome::Accepted]),
            confirm: VecDeque::from([true]),
        };
        drive_to_ready(&mut e, &mut cache, payload, &mut bridge);
        assert!(!e.join_ready(&["car1", "car2"], 4)); // car2 still downloading
        assert!(e.join_ready(&["car1"], 4));
        assert!(!e.join_ready(&["ghost"], 4)); // unknown: not ready, not panic
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn residency_failure_surfaces() {
        let mut e = engine();
        let dir = test_cache_dir("resfail");
        let mut cache = ContentCache::open(&dir, 1 << 30).unwrap();
        let payload = b"0123456789";
        add_car(&mut e, payload);
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 3).min(payload.len());
            e.receive_chunk("car1", off as u64, &payload[off..end], 1).unwrap();
            off = end;
        }
        e.finalize_download("car1", &mut cache).unwrap();
        e.mount_verify("car1", &mut cache).unwrap();
        e.mount_check_deps("car1").unwrap();
        e.mount_begin("car1", "v:/c").unwrap();
        e.complete_mount("car1").unwrap();
        e.mount_register("car1").unwrap();
        let mut bridge =
            FakeCommit { requests: VecDeque::from([ald_residency::CommitOutcome::Fatal]), confirm: VecDeque::new() };
        e.residency_request("car1", 2, 10, 2, &mut bridge).unwrap();
        assert!(matches!(e.asset_state("car1", 3).unwrap(), AssetState::Failed { .. }));
        assert!(!e.join_ready(&["car1"], 3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_asset_everywhere() {
        let mut e = engine();
        assert_eq!(e.receive_chunk("ghost", 0, b"x", 0), Err(StreamError::UnknownAsset("ghost".into())));
        assert_eq!(e.asset_state("ghost", 0), Err(StreamError::UnknownAsset("ghost".into())));
    }
}
