//! Per-backend capability matrix.
//!
//! These are the real behavioral differences between PostgreSQL, MySQL, MariaDB and
//! SQLite for the operations Aldivine performs. The conformance suite in
//! [`crate::suite`] proves each claim against a live server; a backend only earns
//! `Verdict::Pass` when the assertions actually ran.

use crate::backend::Backend;
use ald_core::AldError;

/// A behavioral divergence the conformance suite asserts. Derived from what real
/// Aldivine servers do, not from marketing parity tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    Transactions,
    NestedTransactions,
    Json,
    Jsonb,
    Timestamps,
    TimestampMicros,
    GeneratedIds,
    Upsert,
    UpsertReturnChanged,
    MigrationLocking,
    Isolation,
    ReadCommitted,
    Serializable,
    DeadlockRetry,
    ConnectionHealth,
    PoolExhaustion,
    IndexOnJson,
    StringLength,
    Utf8,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Transactions => "transactions",
            Capability::NestedTransactions => "nested_transactions",
            Capability::Json => "json",
            Capability::Jsonb => "jsonb",
            Capability::Timestamps => "timestamps",
            Capability::TimestampMicros => "timestamp_micros",
            Capability::GeneratedIds => "generated_ids",
            Capability::Upsert => "upsert",
            Capability::UpsertReturnChanged => "upsert_return_changed",
            Capability::MigrationLocking => "migration_locking",
            Capability::Isolation => "isolation",
            Capability::ReadCommitted => "read_committed",
            Capability::Serializable => "serializable",
            Capability::DeadlockRetry => "deadlock_retry",
            Capability::ConnectionHealth => "connection_health",
            Capability::PoolExhaustion => "pool_exhaustion",
            Capability::IndexOnJson => "index_on_json",
            Capability::StringLength => "string_length",
            Capability::Utf8 => "utf8",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Wrap a backend error with the capability that was under test.
pub fn cap_err(capability: Capability, backend: Backend, reason: impl Into<String>) -> AldError {
    AldError::Config(format!("DB capability `{capability}` unsupported on {backend}: {}", reason.into()))
}

/// (backend, capabilities it claims). `Backend::supports` reads this.
pub static CAPABILITY_MATRIX: &[(Backend, &[Capability])] = &[
    (
        Backend::Postgres,
        &[
            Capability::Transactions,
            Capability::NestedTransactions,
            Capability::Json,
            Capability::Jsonb,
            Capability::Timestamps,
            Capability::TimestampMicros,
            Capability::GeneratedIds,
            Capability::Upsert,
            Capability::UpsertReturnChanged,
            Capability::MigrationLocking,
            Capability::Isolation,
            Capability::ReadCommitted,
            Capability::Serializable,
            Capability::DeadlockRetry,
            Capability::ConnectionHealth,
            Capability::PoolExhaustion,
            Capability::IndexOnJson,
            Capability::StringLength,
            Capability::Utf8,
        ],
    ),
    (
        // MariaDB is treated as a MySQL-family backend with its own identity; the
        // shared claims are listed once and MariaDB reuses them via is_mysql_family.
        Backend::Mysql,
        &[
            Capability::Transactions,
            // MySQL/MariaDB savepoints exist but InnoDB's nested semantics differ;
            // conformance marks this PARTIAL rather than claiming parity.
            Capability::Json,
            // No native JSONB; JSON is stored as LONGTEXT with validity checks.
            Capability::Timestamps,
            // MySQL TIMESTAMP is second-resolution; microsecond precision needs
            // DATETIME(6), which Aldivine uses — claimed and asserted.
            Capability::TimestampMicros,
            Capability::GeneratedIds,
            Capability::Upsert,
            // INSERT ... ON DUPLICATE KEY UPDATE cannot return the changed row the way
            // Postgres' RETURNING can; Aldivine re-selects instead.
            Capability::MigrationLocking,
            Capability::Isolation,
            Capability::ReadCommitted,
            // SERIALIZABLE on InnoDB is gap-locked REPEATABLE READ, not true
            // serializability — NOT claimed.
            Capability::DeadlockRetry,
            Capability::ConnectionHealth,
            Capability::PoolExhaustion,
            Capability::IndexOnJson,
            Capability::StringLength,
            Capability::Utf8,
        ],
    ),
    (Backend::Mariadb, &[]), // claims inherited from the MySQL family
    (
        Backend::Sqlite,
        &[
            Capability::Transactions,
            Capability::Json, // JSON1 extension; stored as TEXT
            Capability::Timestamps,
            Capability::GeneratedIds,
            Capability::Upsert, // ON CONFLICT ... DO UPDATE (3.24+)
            Capability::MigrationLocking,
            Capability::ConnectionHealth,
            Capability::Utf8,
            // Dev/local only: no nested transactions, no JSONB, no serializable
            // isolation, no microsecond timestamps, no pool-exhaustion semantics.
        ],
    ),
];

impl Backend {
    /// MySQL and MariaDB share a capability surface.
    pub fn capabilities(self) -> Vec<Capability> {
        if self.is_mysql_family() {
            CAPABILITY_MATRIX
                .iter()
                .find(|(b, _)| *b == Backend::Mysql)
                .map(|(_, caps)| caps.to_vec())
                .unwrap_or_default()
        } else {
            CAPABILITY_MATRIX.iter().find(|(b, _)| *b == self).map(|(_, caps)| caps.to_vec()).unwrap_or_default()
        }
    }
}
