//! Aldivine Backup, Disaster Recovery (DR) & Lockfile Management
//!
//! Provides deterministic backup creation, SHA-256 integrity verification,
//! database migration version safety checks during restore, and `aldivine.lock`
//! lockfile serialization and validation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;

/// Type of backup archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackupKind {
    Full,
    Incremental { parent_backup_id: String },
}

/// Metadata describing a single file entry in a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupEntry {
    pub relative_path: String,
    pub sha256_hex: String,
    pub size_bytes: u64,
}

/// Signed / checksummed manifest embedded at the root of a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub backup_id: String,
    pub created_at_utc: String,
    pub kind: BackupKind,
    pub server_version: String,
    pub db_migration_version: i64,
    pub entries: Vec<BackupEntry>,
    pub total_bytes: u64,
}

impl BackupManifest {
    pub fn compute_content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.backup_id.as_bytes());
        hasher.update(self.server_version.as_bytes());
        hasher.update(self.db_migration_version.to_le_bytes());
        for entry in &self.entries {
            hasher.update(entry.relative_path.as_bytes());
            hasher.update(entry.sha256_hex.as_bytes());
            hasher.update(entry.size_bytes.to_le_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}

/// Represents a package locked in `aldivine.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub publisher_id: String,
    pub sha256_hex: String,
}

/// The project lockfile (`aldivine.lock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AldivineLock {
    pub lock_version: u32,
    pub packages: Vec<LockedPackage>,
}

impl AldivineLock {
    pub fn new() -> Self {
        Self { lock_version: 1, packages: Vec::new() }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    pub fn verify_package(&self, name: &str, expected_hash: &str) -> Result<(), BackupError> {
        let pkg = self
            .packages
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| BackupError::PackageNotFoundInLock(name.to_string()))?;

        if pkg.sha256_hex != expected_hash {
            return Err(BackupError::LockHashMismatch {
                package: name.to_string(),
                expected: pkg.sha256_hex.clone(),
                actual: expected_hash.to_string(),
            });
        }
        Ok(())
    }
}

impl Default for AldivineLock {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BackupError {
    #[error("empty backup entries list")]
    EmptyBackup,
    #[error("entry not found: {0}")]
    EntryNotFound(String),
    #[error("file hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch { path: String, expected: String, actual: String },
    #[error("database migration incompatible: backup version {backup_version}, target db version {target_version}")]
    IncompatibleDbMigration { backup_version: i64, target_version: i64 },
    #[error("package '{0}' not found in aldivine.lock")]
    PackageNotFoundInLock(String),
    #[error("lock hash mismatch for package '{package}': expected {expected}, got {actual}")]
    LockHashMismatch { package: String, expected: String, actual: String },
}

/// In-memory staged backup archive.
#[derive(Debug, Clone)]
pub struct BackupArchive {
    pub manifest: BackupManifest,
    pub files: HashMap<String, Vec<u8>>,
}

/// Prepared restore execution plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    pub backup_id: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub requires_migration_sync: bool,
}

impl BackupArchive {
    /// Create a new backup archive from named file buffers.
    pub fn create(
        backup_id: impl Into<String>,
        kind: BackupKind,
        server_version: impl Into<String>,
        db_migration_version: i64,
        files: HashMap<String, Vec<u8>>,
    ) -> Result<Self, BackupError> {
        if files.is_empty() {
            return Err(BackupError::EmptyBackup);
        }

        let mut entries = Vec::with_capacity(files.len());
        let mut total_bytes = 0u64;

        for (path, content) in &files {
            let mut hasher = Sha256::new();
            hasher.update(content);
            let sha256_hex = format!("{:x}", hasher.finalize());
            let size_bytes = content.len() as u64;
            total_bytes += size_bytes;

            entries.push(BackupEntry { relative_path: path.clone(), sha256_hex, size_bytes });
        }

        // Deterministic sort by path
        entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let manifest = BackupManifest {
            backup_id: backup_id.into(),
            created_at_utc: "2026-09-16T00:00:00Z".to_string(),
            kind,
            server_version: server_version.into(),
            db_migration_version,
            entries,
            total_bytes,
        };

        Ok(Self { manifest, files })
    }

    /// Verify cryptographic integrity of all files against the manifest.
    pub fn verify_integrity(&self) -> Result<(), BackupError> {
        for entry in &self.manifest.entries {
            let content = self
                .files
                .get(&entry.relative_path)
                .ok_or_else(|| BackupError::EntryNotFound(entry.relative_path.clone()))?;

            let mut hasher = Sha256::new();
            hasher.update(content);
            let actual_hex = format!("{:x}", hasher.finalize());

            if actual_hex != entry.sha256_hex {
                return Err(BackupError::HashMismatch {
                    path: entry.relative_path.clone(),
                    expected: entry.sha256_hex.clone(),
                    actual: actual_hex,
                });
            }
        }
        Ok(())
    }

    /// Plan a restore onto a target environment with specified current DB version.
    pub fn plan_restore(&self, target_db_version: i64, allow_migration_gap: bool) -> Result<RestorePlan, BackupError> {
        self.verify_integrity()?;

        let requires_migration_sync = self.manifest.db_migration_version != target_db_version;
        if requires_migration_sync && !allow_migration_gap {
            return Err(BackupError::IncompatibleDbMigration {
                backup_version: self.manifest.db_migration_version,
                target_version: target_db_version,
            });
        }

        Ok(RestorePlan {
            backup_id: self.manifest.backup_id.clone(),
            file_count: self.manifest.entries.len(),
            total_bytes: self.manifest.total_bytes,
            requires_migration_sync,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample_files() -> HashMap<String, Vec<u8>> {
        let mut f = HashMap::new();
        f.insert("server.cfg".to_string(), b"endpoint_add_tcp 0.0.0.0:30120\n".to_vec());
        f.insert("resources/spawn/init.lua".to_string(), b"print('spawn')".to_vec());
        f
    }

    #[test]
    fn full_backup_creation_and_hash_calculation() {
        let files = make_sample_files();
        let archive = BackupArchive::create("bk-100", BackupKind::Full, "0.1.0", 12, files).unwrap();

        assert_eq!(archive.manifest.backup_id, "bk-100");
        assert_eq!(archive.manifest.kind, BackupKind::Full);
        assert_eq!(archive.manifest.entries.len(), 2);
        assert!(archive.manifest.total_bytes > 0);

        let h = archive.manifest.compute_content_hash();
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn incremental_backup_records_parent() {
        let files = make_sample_files();
        let kind = BackupKind::Incremental { parent_backup_id: "bk-100".to_string() };
        let archive = BackupArchive::create("bk-101", kind.clone(), "0.1.0", 12, files).unwrap();

        assert_eq!(archive.manifest.kind, kind);
    }

    #[test]
    fn verify_integrity_succeeds_on_clean_backup() {
        let files = make_sample_files();
        let archive = BackupArchive::create("bk-102", BackupKind::Full, "0.1.0", 12, files).unwrap();

        assert_eq!(archive.verify_integrity(), Ok(()));
    }

    #[test]
    fn verify_integrity_detects_corrupted_file() {
        let files = make_sample_files();
        let mut archive = BackupArchive::create("bk-103", BackupKind::Full, "0.1.0", 12, files).unwrap();

        // Tamper with one file
        archive.files.insert("server.cfg".to_string(), b"tampered content".to_vec());

        let res = archive.verify_integrity();
        assert!(matches!(res, Err(BackupError::HashMismatch { .. })));
    }

    #[test]
    fn restore_plan_validates_db_version_match() {
        let files = make_sample_files();
        let archive = BackupArchive::create("bk-104", BackupKind::Full, "0.1.0", 5, files).unwrap();

        let plan = archive.plan_restore(5, false).unwrap();
        assert_eq!(plan.backup_id, "bk-104");
        assert!(!plan.requires_migration_sync);
        assert_eq!(plan.file_count, 2);
    }

    #[test]
    fn restore_plan_rejects_incompatible_db_migration() {
        let files = make_sample_files();
        let archive = BackupArchive::create("bk-105", BackupKind::Full, "0.1.0", 5, files).unwrap();

        let err = archive.plan_restore(8, false).unwrap_err();
        assert_eq!(err, BackupError::IncompatibleDbMigration { backup_version: 5, target_version: 8 });
    }

    #[test]
    fn restore_plan_allows_migration_gap_if_flagged() {
        let files = make_sample_files();
        let archive = BackupArchive::create("bk-106", BackupKind::Full, "0.1.0", 5, files).unwrap();

        let plan = archive.plan_restore(8, true).unwrap();
        assert!(plan.requires_migration_sync);
    }

    #[test]
    fn aldivine_lock_roundtrip_serialization() {
        let mut lock = AldivineLock::new();
        lock.packages.push(LockedPackage {
            name: "core-lib".to_string(),
            version: "1.2.0".to_string(),
            publisher_id: "ald-team".to_string(),
            sha256_hex: "abcd".repeat(16),
        });

        let json = lock.to_json().unwrap();
        let parsed = AldivineLock::from_json(&json).unwrap();
        assert_eq!(lock, parsed);
    }

    #[test]
    fn aldivine_lock_detects_hash_tampering() {
        let mut lock = AldivineLock::new();
        lock.packages.push(LockedPackage {
            name: "weapon-pack".to_string(),
            version: "1.0.0".to_string(),
            publisher_id: "ald-team".to_string(),
            sha256_hex: "1111".repeat(16),
        });

        assert_eq!(lock.verify_package("weapon-pack", &"1111".repeat(16)), Ok(()));
        assert!(matches!(
            lock.verify_package("weapon-pack", &"2222".repeat(16)),
            Err(BackupError::LockHashMismatch { .. })
        ));
    }

    #[test]
    fn empty_backup_rejected() {
        let res = BackupArchive::create("bk-empty", BackupKind::Full, "0.1.0", 1, HashMap::new());
        assert_eq!(res.unwrap_err(), BackupError::EmptyBackup);
    }
}
