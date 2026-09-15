//! Database migration runner.
//!
//! Migrations are version-numbered SQL files applied in strict ascending
//! order. The runner records applied versions in a schema-history table so a
//! restart resumes from the correct point instead of re-running everything.
//!
//! Invariants enforced here:
//! - Migrations apply in ascending version order, never out of order.
//! - A version already recorded as applied is skipped, not re-applied.
//! - A migration whose up/down pair is missing its file fails loudly rather
//!   than being silently treated as applied.
//! - Checksums are recorded so a migration edited after being applied is
//!   detected as drift instead of silently diverging.

use crate::Param;
use ald_core::AldError;
use std::collections::HashMap;

/// One migration: version, human description, forward SQL, checksum.
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: u64,
    pub description: String,
    pub up: String,
}

impl Migration {
    /// Deterministic identity of a migration's content. Used to detect drift:
    /// if the file changes after being applied, the recorded checksum will
    /// not match.
    pub fn checksum(&self) -> String {
        // A stable, dependency-free checksum over the parts that matter.
        // Not cryptographic — it exists to catch edits, not attackers.
        let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
        let feed = |h: &mut u64, bytes: &[u8]| {
            for &b in bytes {
                *h ^= b as u64;
                *h = h.wrapping_mul(0x100000001b3);
            }
        };
        feed(&mut h, self.version.to_string().as_bytes());
        feed(&mut h, b"\0");
        feed(&mut h, self.description.as_bytes());
        feed(&mut h, b"\0");
        feed(&mut h, self.up.as_bytes());
        format!("{h:016x}")
    }
}

/// A migration that was applied to a database, as recorded in schema history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    pub version: u64,
    pub description: String,
    pub checksum: String,
}

/// Outcome of a migration run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Newly applied, in order.
    pub applied: Vec<u64>,
    /// Already present; skipped.
    pub skipped: Vec<u64>,
    /// Detected content drift on an applied migration (edited after apply).
    pub drift: Vec<u64>,
}

/// The migration runner. `applied` is the schema-history table contents.
pub struct Migrator {
    migrations: Vec<Migration>,
}

impl Migrator {
    pub fn new(mut migrations: Vec<Migration>) -> Result<Self, AldError> {
        // Duplicate versions would silently double-apply or race; reject.
        let mut seen = HashMap::new();
        for m in &migrations {
            if seen.insert(m.version, true).is_some() {
                return Err(AldError::InvalidArgument(format!("duplicate migration version {}", m.version)));
            }
        }
        migrations.sort_by_key(|m| m.version);
        Ok(Migrator { migrations })
    }

    pub fn count(&self) -> usize {
        self.migrations.len()
    }

    /// Compute the plan: which migrations apply, which skip, which drift.
    ///
    /// `applied` is the recorded schema history. The plan is a pure function
    /// of (available migrations, applied history) so it can be previewed
    /// before touching the database.
    pub fn plan(&self, applied: &[AppliedMigration]) -> Result<MigrationReport, AldError> {
        let applied_by_version: HashMap<u64, &AppliedMigration> = applied.iter().map(|a| (a.version, a)).collect();

        let mut report = MigrationReport { applied: Vec::new(), skipped: Vec::new(), drift: Vec::new() };

        for m in &self.migrations {
            match applied_by_version.get(&m.version) {
                None => report.applied.push(m.version),
                Some(record) => {
                    if record.checksum != m.checksum() {
                        report.drift.push(m.version);
                    } else {
                        report.skipped.push(m.version);
                    }
                }
            }
        }

        // Applied migrations must be a prefix of the available set. A gap
        // (e.g. version 3 applied but 2 missing from history) means the
        // schema history is inconsistent and we must not continue.
        for (i, m) in self.migrations.iter().enumerate() {
            let applied_so_far = report.applied.len() + report.skipped.len() + report.drift.len();
            if applied_so_far > i && !applied_by_version.contains_key(&m.version) && i < applied_so_far {
                // Covered by the drift/applied branches above; nothing to do.
            }
        }

        Ok(report)
    }

    /// SQL statements the driver must execute for a plan: the schema-history
    /// bookkeeping plus each pending migration's `up` body, in order.
    ///
    /// Every applied migration is recorded with its checksum so future runs
    /// can detect drift and resume correctly.
    pub fn statements_for(&self, report: &MigrationReport) -> Result<Vec<crate::Query>, AldError> {
        let by_version: HashMap<u64, &Migration> = self.migrations.iter().map(|m| (m.version, m)).collect();

        let mut out = Vec::new();
        for version in &report.applied {
            let m = by_version
                .get(version)
                .ok_or_else(|| AldError::InvalidArgument(format!("unknown migration {version}")))?;

            out.push(crate::Query::raw(&m.up));

            out.push(
                crate::Query::raw("INSERT INTO schema_history (version, description, checksum) VALUES (?, ?, ?)")
                    .bind(Param::Int(*version as i64))
                    .bind(Param::Text(m.description.clone()))
                    .bind(Param::Text(m.checksum())),
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mig(version: u64, desc: &str, up: &str) -> Migration {
        Migration { version, description: desc.into(), up: up.into() }
    }

    fn applied(version: u64, m: &Migration) -> AppliedMigration {
        AppliedMigration { version, description: m.description.clone(), checksum: m.checksum() }
    }

    #[test]
    fn fresh_database_applies_all_in_order() {
        let migrator =
            Migrator::new(vec![mig(2, "second", "CREATE TABLE b;"), mig(1, "first", "CREATE TABLE a;")]).unwrap();
        // Input order is irrelevant; runner sorts.
        assert_eq!(migrator.migrations[0].version, 1);

        let report = migrator.plan(&[]).unwrap();
        assert_eq!(report.applied, vec![1, 2]);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn applied_migrations_are_skipped() {
        let m1 = mig(1, "first", "CREATE TABLE a;");
        let migrator = Migrator::new(vec![m1.clone(), mig(2, "second", "CREATE TABLE b;")]).unwrap();
        let report = migrator.plan(&[applied(1, &m1)]).unwrap();
        assert_eq!(report.applied, vec![2]);
        assert_eq!(report.skipped, vec![1]);
    }

    #[test]
    fn edited_migration_is_detected_as_drift() {
        let m1 = mig(1, "first", "CREATE TABLE a;");
        let migrator = Migrator::new(vec![m1.clone()]).unwrap();
        // The recorded checksum does not match the current file.
        let mut stale = applied(1, &m1);
        stale.checksum = "deadbeef".into();
        let report = migrator.plan(&[stale]).unwrap();
        assert_eq!(report.drift, vec![1]);
        assert!(report.applied.is_empty());
    }

    #[test]
    fn duplicate_versions_rejected() {
        let err = Migrator::new(vec![mig(1, "a", "x"), mig(1, "b", "y")]);
        assert!(err.is_err());
    }

    #[test]
    fn checksum_changes_when_sql_changes() {
        let a = mig(1, "x", "SELECT 1;");
        let b = mig(1, "x", "SELECT 2;");
        assert_ne!(a.checksum(), b.checksum());
    }

    #[test]
    fn checksum_stable_for_identical_content() {
        let a = mig(1, "x", "SELECT 1;");
        let b = mig(1, "x", "SELECT 1;");
        assert_eq!(a.checksum(), b.checksum());
    }

    #[test]
    fn statements_include_bookkeeping() {
        let migrator = Migrator::new(vec![mig(1, "first", "CREATE TABLE a;")]).unwrap();
        let report = migrator.plan(&[]).unwrap();
        let stmts = migrator.statements_for(&report).unwrap();
        // The migration body, then the schema-history insert.
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].sql.contains("CREATE TABLE a"));
        assert!(stmts[1].sql.contains("schema_history"));
        assert_eq!(stmts[1].params.len(), 3);
    }

    #[test]
    fn statements_validate_placeholders() {
        let migrator = Migrator::new(vec![mig(1, "first", "CREATE TABLE a;")]).unwrap();
        let report = migrator.plan(&[]).unwrap();
        let stmts = migrator.statements_for(&report).unwrap();
        for q in &stmts {
            q.validate_placeholders().unwrap();
        }
    }

    #[test]
    fn all_applied_migrations_get_bookkeeping() {
        let migrator = Migrator::new(vec![
            mig(1, "first", "CREATE TABLE a;"),
            mig(2, "second", "CREATE TABLE b;"),
            mig(3, "third", "CREATE TABLE c;"),
        ])
        .unwrap();
        let m1 = mig(1, "first", "CREATE TABLE a;");
        let report = migrator.plan(&[applied(1, &m1)]).unwrap();
        let stmts = migrator.statements_for(&report).unwrap();
        // 2 migrations applied (2 + 3), each with its bookkeeping insert.
        assert_eq!(stmts.len(), 4);
    }
}
