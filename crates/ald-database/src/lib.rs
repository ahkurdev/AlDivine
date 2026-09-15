//! Database abstraction: connection config, parameterized query guard, and a
//! repository trait. Concrete drivers (Postgres/SQLite) are added in Phase 17.
//! The core invariant: never build SQL via string concatenation of untrusted
//! values — always use placeholders.

use ald_core::AldError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbConfig {
    pub url: String,
    #[serde(default = "default_pool")]
    pub max_pool_size: u32,
    #[serde(default = "default_timeout")]
    pub connection_timeout_secs: u64,
}

fn default_pool() -> u32 {
    16
}
fn default_timeout() -> u64 {
    30
}

impl Default for DbConfig {
    fn default() -> Self {
        DbConfig {
            url: "sqlite::memory:".into(),
            max_pool_size: default_pool(),
            connection_timeout_secs: default_timeout(),
        }
    }
}

impl DbConfig {
    pub fn validate(&self) -> Result<(), AldError> {
        if self.url.trim().is_empty() {
            return Err(AldError::Config("db.url required".into()));
        }
        if self.max_pool_size == 0 {
            return Err(AldError::Config("db.max_pool_size must be >= 1".into()));
        }
        Ok(())
    }
}

/// A SQL statement with bound parameters. Guarantees separation of code and data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub sql: String,
    pub params: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    Text(String),
    Int(i64),
    Bool(bool),
    Null,
}

impl Query {
    pub fn raw(sql: &str) -> Self {
        Query { sql: sql.to_string(), params: Vec::new() }
    }
    pub fn bind(mut self, p: Param) -> Self {
        self.params.push(p);
        self
    }
    /// Count '?' placeholders; must match param count. Prevents accidental
    /// mismatches that could lead to parameter misalignment / injection.
    pub fn validate_placeholders(&self) -> Result<(), AldError> {
        let placeholders = self.sql.matches('?').count();
        if placeholders != self.params.len() {
            return Err(AldError::InvalidArgument(format!(
                "placeholder count {} != param count {}",
                placeholders,
                self.params.len()
            )));
        }
        Ok(())
    }
}

/// Repository pattern: services talk to this, never raw SQL scattered around.
pub trait Repository {
    fn execute(&self, q: &Query) -> ald_core::Result<u64>;
    fn fetch_one(&self, q: &Query) -> ald_core::Result<Option<serde_json::Value>>;
    fn fetch_all(&self, q: &Query) -> ald_core::Result<Vec<serde_json::Value>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_config_default_valid() {
        assert!(DbConfig::default().validate().is_ok());
    }

    #[test]
    fn placeholder_mismatch_detected() {
        let q = Query::raw("SELECT * FROM players WHERE id = ? AND name = ?").bind(Param::Int(1));
        assert!(q.validate_placeholders().is_err());
    }

    #[test]
    fn placeholder_match_ok() {
        let q = Query::raw("SELECT * FROM players WHERE id = ?").bind(Param::Int(1)).bind(Param::Text("x".into()));
        // Only one placeholder -> mismatch with 2 params
        assert!(q.validate_placeholders().is_err());
        let q2 = Query::raw("SELECT * FROM players WHERE id = ?").bind(Param::Int(1));
        assert!(q2.validate_placeholders().is_ok());
    }
}
