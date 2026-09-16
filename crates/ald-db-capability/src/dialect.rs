//! SQL dialect normalization.
//!
//! The single most common porting bug in a multi-DB server product is assuming one
//! placeholder syntax and one quoting rule. This module emits provider-correct SQL
//! for the small set of statements Aldivine generates.

use crate::backend::Backend;

/// How a backend spells a bind parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderStyle {
    /// `$1`, `$2`, ... (Postgres)
    Numbered,
    /// `?` repeated (MySQL / MariaDB / SQLite)
    Question,
}

impl PlaceholderStyle {
    /// Render `n` placeholders for an INSERT VALUES tuple of `cols` columns.
    pub fn tuple(&self, cols: usize) -> String {
        let one = match self {
            PlaceholderStyle::Numbered => "$1",
            PlaceholderStyle::Question => "?",
        };
        std::iter::repeat_n(one, cols).collect::<Vec<_>>().join(", ")
    }

    /// Render `n` placeholders, numbered from `start` (Postgres needs global indices
    /// across a whole statement, not per-tuple).
    pub fn numbered_from(start: usize, n: usize) -> String {
        (start..start + n).map(|i| format!("${i}")).collect::<Vec<_>>().join(", ")
    }
}

pub struct Dialect {
    pub backend: Backend,
    pub placeholder: PlaceholderStyle,
    /// Identifier quote character (backtick vs double-quote).
    pub ident_quote: char,
}

impl Dialect {
    pub fn for_backend(b: Backend) -> Self {
        match b {
            Backend::Postgres => Dialect { backend: b, placeholder: PlaceholderStyle::Numbered, ident_quote: '"' },
            Backend::Mysql | Backend::Mariadb => {
                Dialect { backend: b, placeholder: PlaceholderStyle::Question, ident_quote: '`' }
            }
            Backend::Sqlite => Dialect { backend: b, placeholder: PlaceholderStyle::Question, ident_quote: '"' },
        }
    }

    /// Quote an identifier safely for this backend.
    pub fn ident(&self, name: &str) -> String {
        let q = self.ident_quote;
        let escaped = name.replace(q, &format!("{q}{q}"));
        format!("{q}{escaped}{q}")
    }

    /// `INSERT ... ON CONFLICT` (Postgres/SQLite) vs
    /// `INSERT ... ON DUPLICATE KEY UPDATE` (MySQL/MariaDB).
    pub fn upsert(&self, table: &str, cols: &[&str], conflict_col: &str) -> String {
        let idents: Vec<String> = cols.iter().map(|c| self.ident(c)).collect();
        let placeholders = match self.placeholder {
            PlaceholderStyle::Numbered => PlaceholderStyle::numbered_from(1, cols.len()),
            PlaceholderStyle::Question => self.placeholder.tuple(cols.len()),
        };
        let assignments = cols
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let ph = match self.placeholder {
                    PlaceholderStyle::Numbered => format!("${}", i + 1),
                    PlaceholderStyle::Question => "?".to_string(),
                };
                format!("{} = {}", self.ident(c), ph)
            })
            .collect::<Vec<_>>()
            .join(", ");

        if self.backend.is_mysql_family() {
            format!(
                "INSERT INTO {} ({}) VALUES ({}) ON DUPLICATE KEY UPDATE {}",
                self.ident(table),
                idents.join(", "),
                placeholders,
                assignments
            )
        } else {
            format!(
                "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {}",
                self.ident(table),
                idents.join(", "),
                placeholders,
                self.ident(conflict_col),
                assignments
            )
        }
    }

    /// JSON column type for this backend.
    pub fn json_type(&self) -> &'static str {
        match self.backend {
            Backend::Postgres => "JSONB",
            _ => "JSON",
        }
    }

    /// Microsecond-capable timestamp type.
    pub fn timestamp_type(&self) -> &'static str {
        match self.backend {
            Backend::Postgres => "TIMESTAMPTZ",
            // MySQL/MariaDB: DATETIME(6) carries fractional seconds; plain TIMESTAMP
            // does not. SQLite has no native type (TEXT ISO-8601).
            Backend::Mysql | Backend::Mariadb => "DATETIME(6)",
            Backend::Sqlite => "TEXT",
        }
    }

    /// Migration-lock DDL. Postgres uses an advisory lock; MySQL/MariaDB use
    /// GET_LOCK(); SQLite serializes at the connection level.
    pub fn migration_lock(&self) -> &'static str {
        match self.backend {
            Backend::Postgres => "SELECT pg_advisory_lock(0x616C64)", // 'ald'
            Backend::Mysql | Backend::Mariadb => "SELECT GET_LOCK('aldivine_migrations', 30)",
            Backend::Sqlite => "SELECT 1",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    #[test]
    fn placeholder_styles() {
        assert_eq!(PlaceholderStyle::Question.tuple(3), "?, ?, ?");
        assert_eq!(PlaceholderStyle::numbered_from(1, 3), "$1, $2, $3");
        assert_eq!(PlaceholderStyle::numbered_from(4, 2), "$4, $5");
    }

    #[test]
    fn ident_quoting() {
        assert_eq!(Dialect::for_backend(Backend::Postgres).ident("order"), "\"order\"");
        assert_eq!(Dialect::for_backend(Backend::Mysql).ident("order"), "`order`");
        // Doubling is the SQL-standard escape.
        assert_eq!(Dialect::for_backend(Backend::Postgres).ident("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn upsert_dialects() {
        let pg = Dialect::for_backend(Backend::Postgres);
        assert!(pg.upsert("t", &["k", "v"], "k").contains("ON CONFLICT"));
        let my = Dialect::for_backend(Backend::Mysql);
        assert!(my.upsert("t", &["k", "v"], "k").contains("ON DUPLICATE KEY UPDATE"));
        let lite = Dialect::for_backend(Backend::Sqlite);
        assert!(lite.upsert("t", &["k", "v"], "k").contains("ON CONFLICT"));
    }

    #[test]
    fn json_types() {
        assert_eq!(Dialect::for_backend(Backend::Postgres).json_type(), "JSONB");
        assert_eq!(Dialect::for_backend(Backend::Mariadb).json_type(), "JSON");
        assert_eq!(Dialect::for_backend(Backend::Sqlite).json_type(), "JSON");
    }

    #[test]
    fn timestamp_types() {
        assert_eq!(Dialect::for_backend(Backend::Postgres).timestamp_type(), "TIMESTAMPTZ");
        assert_eq!(Dialect::for_backend(Backend::Mysql).timestamp_type(), "DATETIME(6)");
        assert_eq!(Dialect::for_backend(Backend::Sqlite).timestamp_type(), "TEXT");
    }

    #[test]
    fn backend_from_url() {
        assert_eq!(Backend::from_url("postgres://u:p@h/db").unwrap(), Backend::Postgres);
        assert_eq!(Backend::from_url("PostgreSQL://u:p@h/db").unwrap(), Backend::Postgres);
        assert_eq!(Backend::from_url("mysql://u:p@h/db").unwrap(), Backend::Mysql);
        assert_eq!(Backend::from_url("mariadb://u:p@h/db").unwrap(), Backend::Mariadb);
        assert_eq!(Backend::from_url("sqlite::memory:").unwrap(), Backend::Sqlite);
        assert!(Backend::from_url("u:p@h/db").is_err());
        assert!(Backend::from_url("oracle://h/db").is_err());
    }

    #[test]
    fn mysql_family_shares_capabilities() {
        assert_eq!(Backend::Mysql.capabilities(), Backend::Mariadb.capabilities());
        assert!(Backend::Mariadb.supports(Capability::Json));
        assert!(!Backend::Mariadb.supports(Capability::Jsonb));
        assert!(Backend::Postgres.supports(Capability::Jsonb));
    }

    #[test]
    fn sqlite_is_dev_only() {
        assert!(!Backend::Sqlite.supports(Capability::Serializable));
        assert!(!Backend::Sqlite.supports(Capability::NestedTransactions));
        assert!(Backend::Sqlite.supports(Capability::Upsert));
    }
}
