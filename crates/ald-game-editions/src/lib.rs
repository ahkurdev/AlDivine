//! GTA edition model: Legacy vs Enhanced.
//!
//! The two editions are separate compatibility universes (different
//! executables, renderer paths, asset expectations). Everything downstream —
//! GameBridge profiles, native tables, asset/DataFile rules — keys off this
//! enum, so misclassification must be impossible by construction:
//!
//! - Edition is only ever **declared** (by NovaGate detection or operator
//!   config) or **parsed** from an explicit string. There is no filename
//!   guessing here: install-layout heuristics live in NovaGate detection,
//!   which reports what it found; this crate types the answer.
//! - Unknown strings fail to parse. There is no `Unknown` variant that later
//!   code could accidentally treat as supported.

use thiserror::Error;

/// Which GTA V generation the client runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GameEdition {
    /// Original PC release line (pre-Gen9).
    Legacy,
    /// Current-generation (Gen9) release line.
    Enhanced,
}

impl GameEdition {
    /// Canonical lowercase identifier used in configs and protocols.
    pub fn as_str(self) -> &'static str {
        match self {
            GameEdition::Legacy => "legacy",
            GameEdition::Enhanced => "enhanced",
        }
    }

    /// Parse an explicit edition string. Accepts `legacy`, `enhanced`
    /// (case-insensitive, surrounding whitespace ignored). Anything else —
    /// including `""` — is an error, never a guess.
    pub fn parse(s: &str) -> Result<Self, EditionError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "legacy" => Ok(GameEdition::Legacy),
            "enhanced" => Ok(GameEdition::Enhanced),
            other => Err(EditionError::UnknownEdition(other.to_string())),
        }
    }
}

/// Per-edition profile: the compatibility surface that differs between
/// generations. Concrete tables (natives, assets, DataFiles) arrive in later
/// phases; this struct reserves their slots so callers key off one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditionProfile {
    pub edition: GameEdition,
    /// Whether this Aldivine release attempts to support the edition at all.
    /// `false` means clean refusal with a clear message, not silent breakage.
    pub supported: bool,
    /// Operator-facing note, e.g. why an edition is unsupported in this build.
    pub note: String,
}

impl EditionProfile {
    pub fn unsupported(edition: GameEdition, note: &str) -> Self {
        EditionProfile { edition, supported: false, note: note.to_string() }
    }

    pub fn supported(edition: GameEdition) -> Self {
        EditionProfile { edition, supported: true, note: String::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditionError {
    #[error("unknown game edition '{0}' (expected 'legacy' or 'enhanced')")]
    UnknownEdition(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_both_editions_case_insensitive() {
        assert_eq!(GameEdition::parse("legacy"), Ok(GameEdition::Legacy));
        assert_eq!(GameEdition::parse("Legacy"), Ok(GameEdition::Legacy));
        assert_eq!(GameEdition::parse("  ENHANCED "), Ok(GameEdition::Enhanced));
    }

    #[test]
    fn parse_rejects_everything_else() {
        for bad in ["", "  ", "gen9", "nextgen", "ps5", "xbox", "fivem", "legacy2", "enhance"] {
            assert!(GameEdition::parse(bad).is_err(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn identifiers_round_trip() {
        assert_eq!(GameEdition::parse(GameEdition::Legacy.as_str()), Ok(GameEdition::Legacy));
        assert_eq!(GameEdition::parse(GameEdition::Enhanced.as_str()), Ok(GameEdition::Enhanced));
    }

    #[test]
    fn unsupported_profile_carries_reason() {
        let p = EditionProfile::unsupported(GameEdition::Legacy, "bridge not yet ported");
        assert!(!p.supported);
        assert!(p.note.contains("bridge"));
        assert!(EditionProfile::supported(GameEdition::Enhanced).supported);
    }
}
