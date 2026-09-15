//! Aldivine DB Capability Layer.
//!
//! Spec: "Do NOT assume every DB behaves like PostgreSQL." This crate normalizes the
//! behavioral differences between PostgreSQL, MySQL, MariaDB and SQLite across the
//! surface Aldivine actually uses, and ships a conformance suite that runs the SAME
//! assertions against every provider.
//!
//! Nothing here is a placeholder: [`Capability`] is derived from what real servers do,
//! and [`ConformanceSuite`] is executed against a live backend in the conformance tests.
//! A provider that cannot be reached is not marked IMPLEMENTED.

pub mod backend;
pub mod capability;
pub mod dialect;
pub mod suite;

pub use backend::{Backend, Connection, QueryResult, Row};
pub use capability::{cap_err, Capability};
pub use dialect::{Dialect, PlaceholderStyle};

/// One normalized capability report for a backend, produced by [`crate::suite::ConformanceSuite`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityReport {
    pub backend: Backend,
    /// Overall verdict: one failed check BLOCKS the whole backend.
    pub verdict: Verdict,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// Every capability the backend claims is supported actually works.
    Pass,
    /// At least one claimed capability failed its conformance check.
    Blocked,
}

impl Verdict {
    pub fn is_pass(self) -> bool {
        matches!(self, Verdict::Pass)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub capability: Capability,
    pub status: CheckStatus,
    /// Present on failure, so Aegis can show the exact divergence.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    /// The backend supports the capability and the conformance assertion passed.
    Pass,
    /// The backend advertises support but the behavior diverged.
    Fail,
    /// Not applicable to this backend (e.g. migration locking on SQLite dev).
    NotApplicable,
}

impl CheckResult {
    fn pass(c: Capability) -> Self {
        CheckResult { capability: c, status: CheckStatus::Pass, detail: String::new() }
    }

    fn fail(c: Capability, detail: impl Into<String>) -> Self {
        CheckResult { capability: c, status: CheckStatus::Fail, detail: detail.into() }
    }

    fn na(c: Capability) -> Self {
        CheckResult { capability: c, status: CheckStatus::NotApplicable, detail: String::new() }
    }
}

impl CapabilityReport {
    pub fn new(backend: Backend, checks: Vec<CheckResult>) -> Self {
        let verdict = if checks
            .iter()
            .any(|c| c.status == CheckStatus::Fail)
        {
            Verdict::Blocked
        } else {
            Verdict::Pass
        };
        CapabilityReport { backend, verdict, checks }
    }

    pub fn pass_count(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Pass).count()
    }

    pub fn fail_count(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Fail).count()
    }
}
