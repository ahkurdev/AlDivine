//! Aldivine core primitives: errors, identifiers, time.
//! Shared by every other crate. No external network/game dependencies.

pub mod error;
pub mod id;
pub mod logging;
pub mod time;

pub use error::{AldError, Result};
pub use id::{AldivinePlayerId, EntityId};
pub use logging::{format_timestamp, inspect_log_file, install_crash_handler, DiagnosticLogger, LogLevel, LogSummary};
pub use time::{now_ms, now_secs};
