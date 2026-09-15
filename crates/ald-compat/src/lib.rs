//! Aldivine Compatibility Layer (ACL).
//!
//! Purpose: let a resource written against ESX/QBCore/Qbox run on Aldivine
//! without forking, by translating its calls into native framework services.
//!
//! What this is NOT: a reimplementation of those frameworks. Adapters are
//! *read-mostly projections* — they expose the shape the legacy resource
//! expects (GetPlayerFromId, money accessors, inventory helpers) and route
//! every mutation through the authoritative Aldivine service. A legacy call
//! that has no safe server-authoritative equivalent returns NotSupported
//! rather than silently no-op'ing.
//!
//! Every adapter sits behind the `LegacyAdapter` trait so the host can
//! enumerate them, and so a resource can only ever reach the adapter that its
//! manifest declares (`compat = "esx"`).

use std::fmt;

use ald_core::AldivinePlayerId;

/// Which legacy ecosystem a resource was written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyFlavor {
    Esx,
    Qbcore,
    Qbox,
}

impl LegacyFlavor {
    pub fn manifest_key(self) -> &'static str {
        match self {
            LegacyFlavor::Esx => "esx",
            LegacyFlavor::Qbcore => "qbcore",
            LegacyFlavor::Qbox => "qbox",
        }
    }

    pub fn from_manifest_key(k: &str) -> Option<Self> {
        match k {
            "esx" => Some(LegacyFlavor::Esx),
            "qbcore" => Some(LegacyFlavor::Qbcore),
            "qbox" => Some(LegacyFlavor::Qbox),
            _ => None,
        }
    }
}

impl fmt::Display for LegacyFlavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.manifest_key())
    }
}

/// Projection of a legacy player object (ESX xPlayer / QBCore Player).
/// All money accessors are in cents; adapters convert to the float-ish
/// semantics the legacy API pretended to have only at the boundary.
pub struct LegacyPlayer {
    pub id: AldivinePlayerId,
    pub source: u32,
    pub name: String,
    pub job: String,
    pub grade: u32,
    /// cash + bank, in cents.
    pub money: u64,
}

/// One capability the adapter claims to support, with its coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Behaviorally equivalent, server-authoritative.
    Supported,
    /// Works for the common case; documented divergences remain.
    Partial,
    /// No safe native equivalent; the resource must be ported.
    NotSupported,
}

/// The trait every compatibility adapter implements. Implementations hold no
/// state of their own — they borrow the authoritative services, so legacy
/// mutations land in the same validated code path as native ones.
pub trait LegacyAdapter {
    fn flavor(&self) -> LegacyFlavor;

    fn player(&self, source: u32) -> Option<LegacyPlayer>;

    fn coverage(&self, capability: &str) -> Coverage;

    /// Names of capabilities this adapter knows about.
    fn capabilities(&self) -> &'static [&'static str];
}

/// Error returned when a legacy call cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum CompatError {
    #[error("legacy capability '{0}' is not supported on Aldivine; port the resource")]
    NotSupported(String),
    #[error("player source {0} is not connected")]
    UnknownPlayer(u32),
    #[error("adapter for flavor '{0}' is not loaded")]
    NoAdapter(&'static str),
}
