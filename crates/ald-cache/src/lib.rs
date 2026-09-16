//! Content-addressed cache: atomic staging, integrity, repair, quota.
//!
//! Layout inside one directory:
//!
//! ```text
//! objects/ab/cdef…   one file per sha256, sharded by first byte
//! tmp/               staging area (never served)
//! ```
//!
//! Rules:
//! - Objects are named by their sha256. The same bytes stored twice land
//!   once (dedup is structural, not a feature flag).
//! - Writes stage to `tmp/` first; `commit()` re-hashes the staged bytes
//!   and only then atomically renames into `objects/`. A hash mismatch or
//!   a crash mid-stage leaves no partial object behind — partial files are
//!   never considered valid, and the temp is removed on failure.
//! - `verify()` re-hashes on demand; `verify_all()` reports per-object
//!   health; `repair()` deletes corrupt objects and reports what it
//!   removed. Repair never fabricates bytes — a corrupt object is gone, and
//!   the caller re-fetches it through the download engine.
//! - Quota: `evict_to_fit()` deletes least-recently-used objects until the
//!   store fits, except `protected` hashes (mounted content the game holds).
//!   Recency is tracked in memory by access; on `open()`, order rebuilds
//!   from file mtimes so restarts do not evict blindly.
//!
//! What this crate does NOT do: downloading (the caller fetches bytes via
//! `ald-download` and stages them here) or mounting (later phase consumes
//! `object_path()`).

use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CacheError {
    #[error("io: {0}")]
    Io(String),
    #[error("staged bytes do not match expected hash {expected} (got {actual})")]
    HashMismatch { expected: String, actual: String },
    #[error("no such staged file")]
    NoSuchStaged,
    #[error("no such object '{0}'")]
    NoSuchObject(String),
    #[error("bad hash '{0}' (expected 64 lowercase hex)")]
    BadHash(String),
}

impl From<std::io::Error> for CacheError {
    fn from(e: std::io::Error) -> Self {
        CacheError::Io(e.to_string())
    }
}

/// sha256 hex of bytes.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(64);
    for b in Sha256::digest(data) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// A staged (unverified, unserved) payload.
#[derive(Debug)]
pub struct Staged {
    path: PathBuf,
}

/// Health report over stored objects.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthReport {
    pub ok: usize,
    pub corrupt: Vec<String>,
}

/// Content-addressed object store in `dir`.
#[derive(Debug)]
pub struct ContentCache {
    quota_bytes: u64,
    objects_dir: PathBuf,
    tmp_dir: PathBuf,
    sizes: HashMap<String, u64>,
    /// Most-recently-used last. Rebuilt from mtimes on open, updated by touch.
    lru: VecDeque<String>,
    staged_seq: u64,
}

impl ContentCache {
    /// Open (creating layout), scanning existing objects for sizes and mtime
    /// recency order. Unknown files under `objects/` (wrong name shape or
    /// bad shard) are ignored, never served — `repair` does not apply to
    /// files the cache itself could not have written.
    pub fn open(dir: &Path, quota_bytes: u64) -> Result<Self, CacheError> {
        let objects_dir = dir.join("objects");
        let tmp_dir = dir.join("tmp");
        std::fs::create_dir_all(&objects_dir)?;
        std::fs::create_dir_all(&tmp_dir)?;
        let mut c = ContentCache {
            quota_bytes,
            objects_dir,
            tmp_dir,
            sizes: HashMap::new(),
            lru: VecDeque::new(),
            staged_seq: 0,
        };
        c.rescan()?;
        Ok(c)
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        self.objects_dir.join(&hash[..2]).join(&hash[2..])
    }

    fn rescan(&mut self) -> Result<(), CacheError> {
        let mut found: Vec<(String, u64, std::time::SystemTime)> = Vec::new();
        let Ok(shards) = std::fs::read_dir(&self.objects_dir) else { return Ok(()) };
        for shard in shards.flatten() {
            let Ok(files) = std::fs::read_dir(shard.path()) else { continue };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                let shard_name = shard.file_name().to_string_lossy().into_owned();
                if shard_name.len() != 2 || name.len() != 62 {
                    continue; // not ours: ignore
                }
                let hash = format!("{shard_name}{name}");
                if !valid_hash(&hash) {
                    continue;
                }
                let meta = match f.metadata() {
                    Ok(m) if m.is_file() => m,
                    _ => continue,
                };
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                found.push((hash, meta.len(), mtime));
            }
        }
        found.sort_by_key(|(_, _, t)| *t);
        for (hash, size, _) in found {
            self.sizes.insert(hash.clone(), size);
            self.lru.push_back(hash);
        }
        Ok(())
    }

    /// Stage raw bytes for a later verified commit. Returns a handle, not a
    /// servable object.
    pub fn stage(&mut self, payload: &[u8]) -> Result<Staged, CacheError> {
        self.staged_seq += 1;
        let path = self.tmp_dir.join(format!("stage-{}-{}", std::process::id(), self.staged_seq));
        let mut f = File::create(&path)?;
        f.write_all(payload)?;
        f.sync_all()?;
        Ok(Staged { path })
    }

    /// Verify staged bytes against `expected` and atomically promote into
    /// objects. Idempotent: committing bytes already stored re-verifies and
    /// reports the same path. Mismatch removes the temp and errors.
    pub fn commit(&mut self, staged: Staged, expected: &str) -> Result<PathBuf, CacheError> {
        if !valid_hash(expected) {
            let _ = std::fs::remove_file(&staged.path);
            return Err(CacheError::BadHash(expected.to_string()));
        }
        let bytes = std::fs::read(&staged.path).map_err(|_| CacheError::NoSuchStaged)?;
        let actual = sha256_hex(&bytes);
        if actual != expected {
            let _ = std::fs::remove_file(&staged.path);
            return Err(CacheError::HashMismatch { expected: expected.to_string(), actual });
        }
        let dest = self.object_path(expected);
        if !dest.exists() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&staged.path, &dest)?;
        } else {
            let _ = std::fs::remove_file(&staged.path);
        }
        // Re-verify what landed (rename onto existing is impossible here —
        // we checked — but disks lie; trust the hash, not the path).
        let landed = std::fs::read(&dest)?;
        if sha256_hex(&landed) != expected {
            let _ = std::fs::remove_file(&dest);
            return Err(CacheError::HashMismatch { expected: expected.to_string(), actual: sha256_hex(&landed) });
        }
        self.sizes.insert(expected.to_string(), landed.len() as u64);
        self.touch(expected);
        Ok(dest)
    }

    /// Serve an object path, if stored. Does not re-hash (use `verify` for
    /// that); the name IS the verified claim from `commit`.
    pub fn get(&mut self, hash: &str) -> Option<PathBuf> {
        if !valid_hash(hash) {
            return None;
        }
        let p = self.object_path(hash);
        if p.is_file() {
            self.touch(hash);
            Some(p)
        } else {
            None
        }
    }

    /// Re-hash one object. Missing objects are `false`, not an error.
    pub fn verify(&self, hash: &str) -> Result<bool, CacheError> {
        if !valid_hash(hash) {
            return Err(CacheError::BadHash(hash.to_string()));
        }
        let p = self.object_path(hash);
        let bytes = match std::fs::read(&p) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        Ok(sha256_hex(&bytes) == hash)
    }

    /// Health over every tracked object.
    pub fn verify_all(&self) -> Result<HealthReport, CacheError> {
        let mut report = HealthReport::default();
        for hash in self.sizes.keys() {
            if self.verify(hash)? {
                report.ok += 1;
            } else {
                report.corrupt.push(hash.clone());
            }
        }
        report.corrupt.sort();
        Ok(report)
    }

    /// Delete corrupt objects. Returns hashes removed. Never recreates data.
    pub fn repair(&mut self) -> Result<Vec<String>, CacheError> {
        let report = self.verify_all()?;
        for hash in &report.corrupt {
            let _ = std::fs::remove_file(self.object_path(hash));
            self.sizes.remove(hash);
            self.lru.retain(|h| h != hash);
        }
        // Drop empty shards left behind (hygiene, best effort).
        if let Ok(shards) = std::fs::read_dir(&self.objects_dir) {
            for shard in shards.flatten() {
                let _ = std::fs::remove_dir(shard.path());
            }
        }
        Ok(report.corrupt)
    }

    /// Total stored bytes.
    pub fn size_bytes(&self) -> u64 {
        self.sizes.values().sum()
    }

    pub fn object_count(&self) -> usize {
        self.sizes.len()
    }

    /// Evict least-recently-used objects until within quota. `protected`
    /// hashes are never evicted (mounted content). Returns evicted hashes,
    /// oldest first. A protected set larger than quota still evicts
    /// everything unprotected — protection exempts, it does not reserve.
    pub fn evict_to_fit(&mut self, protected: &[String]) -> Result<Vec<String>, CacheError> {
        let mut evicted = Vec::new();
        while self.size_bytes() > self.quota_bytes {
            let victim = self.lru.iter().find(|h| !protected.contains(h)).cloned();
            match victim {
                Some(hash) => {
                    let _ = std::fs::remove_file(self.object_path(&hash));
                    self.sizes.remove(&hash);
                    self.lru.retain(|h| h != &hash);
                    evicted.push(hash);
                }
                None => break, // everything left is protected: stop, report
            }
        }
        Ok(evicted)
    }

    fn touch(&mut self, hash: &str) {
        self.lru.retain(|h| h != hash);
        self.lru.push_back(hash.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_IDS: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let id = TEST_IDS.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ald-cache-test-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn put(cache: &mut ContentCache, data: &[u8]) -> String {
        let hash = sha256_hex(data);
        let staged = cache.stage(data).unwrap();
        cache.commit(staged, &hash).unwrap();
        hash
    }

    #[test]
    fn stage_commit_serve_roundtrip() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 1 << 30).unwrap();
        let hash = put(&mut c, b"hello-cache");
        let path = c.get(&hash).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello-cache");
        assert_eq!(c.object_count(), 1);
        assert!(c.verify(&hash).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedup_structural() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 1 << 30).unwrap();
        let h1 = put(&mut c, b"same");
        let h2 = put(&mut c, b"same");
        assert_eq!(h1, h2);
        assert_eq!(c.object_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_mismatch_never_lands() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 1 << 30).unwrap();
        let staged = c.stage(b"real-bytes").unwrap();
        let wrong = sha256_hex(b"other-bytes");
        assert_eq!(
            c.commit(staged, &wrong),
            Err(CacheError::HashMismatch { expected: wrong.clone(), actual: sha256_hex(b"real-bytes") })
        );
        assert_eq!(c.get(&wrong), None);
        assert_eq!(c.object_count(), 0);
        // Temp cleaned up: tmp/ empty.
        assert!(std::fs::read_dir(dir.join("tmp")).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_hash_rejected() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 1 << 30).unwrap();
        let staged = c.stage(b"x").unwrap();
        assert!(matches!(c.commit(staged, "notahash"), Err(CacheError::BadHash(_))));
        assert!(c.get("notahash").is_none());
        assert!(c.verify("notahash").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_object_is_none_and_false() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 1 << 30).unwrap();
        let absent = sha256_hex(b"never-stored");
        assert_eq!(c.get(&absent), None);
        assert!(!c.verify(&absent).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corruption_detected_and_repaired() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 1 << 30).unwrap();
        let h1 = put(&mut c, b"good-1");
        let h2 = put(&mut c, b"good-2");
        // Flip a byte in h1's object on disk.
        let p1 = c.get(&h1).unwrap();
        let mut bytes = std::fs::read(&p1).unwrap();
        bytes[0] ^= 0xFF;
        std::fs::write(&p1, &bytes).unwrap();
        let report = c.verify_all().unwrap();
        assert_eq!(report.ok, 1);
        assert_eq!(report.corrupt, vec![h1.clone()]);
        let removed = c.repair().unwrap();
        assert_eq!(removed, vec![h1.clone()]);
        assert_eq!(c.get(&h1), None);
        assert!(c.verify(&h2).unwrap());
        assert_eq!(c.object_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quota_evicts_lru_first() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 30).unwrap(); // 30-byte quota
        let a = put(&mut c, b"aaaaaaaaaa"); // 10
        let b = put(&mut c, b"bbbbbbbbbb"); // 10
        let d = put(&mut c, b"dddddddddd"); // 10 -> exactly 30, fits
        assert_eq!(c.size_bytes(), 30);
        let _ = put(&mut c, b"eeeeeeeeee"); // 40 total: over quota
                                            // Access order: a oldest untouched... touch b and d to protect by recency.
        let _ = c.get(&b);
        let _ = c.get(&d);
        let evicted = c.evict_to_fit(&[]).unwrap();
        assert_eq!(evicted, vec![a.clone()]);
        assert_eq!(c.get(&a), None);
        assert_eq!(c.size_bytes(), 30);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn protected_survives_eviction() {
        let dir = test_dir();
        let mut c = ContentCache::open(&dir, 10).unwrap();
        let mounted = put(&mut c, b"mounted-01"); // 10
        let other = put(&mut c, b"other-data"); // 20 total
        let evicted = c.evict_to_fit(std::slice::from_ref(&mounted)).unwrap();
        assert_eq!(evicted, vec![other.clone()]);
        assert!(c.get(&mounted).is_some());
        // Everything protected: nothing evicted even over quota.
        let evicted = c.evict_to_fit(std::slice::from_ref(&mounted)).unwrap();
        assert!(evicted.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopen_rebuilds_index() {
        let dir = test_dir();
        let hash = {
            let mut c = ContentCache::open(&dir, 1 << 30).unwrap();
            put(&mut c, b"persist-me")
        };
        let c2 = ContentCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(c2.object_count(), 1);
        assert_eq!(c2.size_bytes(), 10);
        assert!(c2.verify(&hash).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stray_files_ignored_never_served() {
        let dir = test_dir();
        let c = ContentCache::open(&dir, 1 << 30).unwrap();
        // Drop junk the cache never wrote.
        std::fs::create_dir_all(dir.join("objects").join("zz")).unwrap();
        std::fs::write(dir.join("objects").join("zz").join("junk"), b"junk").unwrap();
        std::fs::write(dir.join("objects").join("loose"), b"loose").unwrap();
        let c2 = ContentCache::open(&dir, 1 << 30).unwrap();
        assert_eq!(c2.object_count(), 0);
        assert_eq!(c2.verify_all().unwrap(), HealthReport::default());
        let _ = c;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
