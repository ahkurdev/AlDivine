//! Backend identity + the minimal sync execution surface the conformance suite needs.
//!
//! ponytail: this is deliberately a small, blocking trait rather than a full async
//! pool. The capability layer exists to document behavioral differences, not to be the
//! hot-path driver; `ald-server` wires a real pooled driver (sqlx) on top of the same
//! Dialect/Capability decisions this crate derives.

use ald_core::AldError;

/// The concrete RDBMS Aldivine is talking to. Drives every divergence decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    Postgres,
    Mysql,
    Mariadb,
    /// Dev/local only — never a production recommendation.
    Sqlite,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Postgres => "postgres",
            Backend::Mysql => "mysql",
            Backend::Mariadb => "mariadb",
            Backend::Sqlite => "sqlite",
        }
    }

    /// The spec's default for new native deployments.
    pub fn recommended() -> Backend {
        Backend::Postgres
    }

    /// Parse a URL scheme (`postgres://`, `mysql://`, `sqlite::memory:`, ...) into
    /// a backend. Accepts both RFC-3986 `scheme://rest` and sqlx's `scheme:rest`.
    pub fn from_url(url: &str) -> Result<Backend, AldError> {
        // Take everything before the first ':' that is not part of `://`.
        let scheme = url
            .find("://")
            .map(|i| &url[..i])
            .or_else(|| url.find(':').map(|i| &url[..i]))
            .unwrap_or("")
            .to_ascii_lowercase();
        match scheme.as_str() {
            "postgres" | "postgresql" => Ok(Backend::Postgres),
            "mysql" => Ok(Backend::Mysql),
            "mariadb" => Ok(Backend::Mariadb),
            "sqlite" => Ok(Backend::Sqlite),
            "" => Err(AldError::Config("db.url is missing a scheme".into())),
            other => Err(AldError::Config(format!("unsupported db scheme `{other}`"))),
        }
    }

    /// MySQL and MariaDB share a wire protocol and, for Aldivine's purposes, a
    /// capability surface — they are treated as one family with distinct identity.
    pub fn is_mysql_family(self) -> bool {
        matches!(self, Backend::Mysql | Backend::Mariadb)
    }

    /// True if this backend claims `cap`. MariaDB inherits the MySQL family's
    /// claims rather than carrying a duplicate row.
    pub fn supports(self, cap: crate::Capability) -> bool {
        let lookup = if self.is_mysql_family() { Backend::Mysql } else { self };
        crate::capability::CAPABILITY_MATRIX
            .iter()
            .find(|(b, _)| *b == lookup)
            .map(|(_, caps)| caps.contains(&cap))
            .unwrap_or(false)
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row of a result set, addressed by column name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub cols: Vec<(String, Option<String>)>,
}

impl Row {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.cols
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.as_deref())
    }

    pub fn len(&self) -> usize {
        self.cols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }
}

/// Outcome of a statement that does not return rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryResult {
    pub rows_affected: u64,
}

/// The execution surface a live backend must implement for the conformance suite.
pub trait Connection {
    fn backend(&self) -> Backend;

    /// Execute a statement that returns no rows.
    fn execute(&mut self, sql: &str) -> Result<QueryResult, AldError>;

    /// Run a statement that returns rows.
    fn query(&mut self, sql: &str) -> Result<Vec<Row>, AldError>;

    /// Run a statement inside a transaction. The closure must see a consistent view
    /// and its writes must be invisible to other connections until commit.
    fn transaction(&mut self, work: &mut dyn FnMut(&mut dyn Connection) -> Result<(), AldError>)
        -> Result<(), AldError>;

    /// True if a round-trip currently succeeds.
    fn ping(&mut self) -> Result<(), AldError>;

    /// Provider-native error when a feature is unavailable, so callers can map it.
    fn unsupported(&self, what: &str) -> AldError {
        AldError::Config(format!("{} does not support {what}", self.backend()))
    }
}
