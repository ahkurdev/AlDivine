//! Aldivine core primitives: errors, identifiers, time.
//! Shared by every other crate. No external network/game dependencies.

pub mod error;
pub mod id;
pub mod time;

pub use error::{AldError, Result};
pub use id::{AldivinePlayerId, EntityId};
pub use time::{now_ms, now_secs};
