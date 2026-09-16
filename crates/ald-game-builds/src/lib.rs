//! GTA build manager: known-build registry, assessment, emergency pinning.
//!
//! A build number is meaningless without its edition, and a number this
//! crate has never seen is **unsafe until proven otherwise**: unknown builds
//! are refused by default. This is the opposite of "accept and hope".
//!
//! Data honesty: this crate ships **no certified build corpus**. The
//! registry is data-driven ([`BuildRegistry::from_entries`] / TOML); entries
//! become certified only through the Compatibility Lab against real installs
//! (see `docs/GTA_BUILD_SUPPORT.md`). Tests use synthetic numbers (90000+)
//! that cannot collide with real builds, so a green suite proves logic, not
//! coverage of any real game version.
//!
//! Assessment ([`BuildRegistry::assess`]) answers, in order:
//! 1. Is the edition itself supported here? (`EditionProfile`)
//! 2. Is this exact build known for that edition?
//! 3. Is it blocked (known-bad: crash, exploit, broken natives)?
//! 4. Does Astryn meet the build's minimum version?
//!
//! Emergency compatibility ([`EmergencyPin`]): when a client arrives on an
//! unknown build (e.g. Rockstar shipped an update overnight), the operator
//! policy decides: refuse (default), or pin the client to the edition's
//! last-known-good profile with a visible warning. Pinning is explicit
//! operator opt-in per edition, never silent.

use std::collections::BTreeMap;

use ald_game_editions::{EditionProfile, GameEdition};
use thiserror::Error;

/// One known game build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameBuild {
    /// Numeric build identifier reported by the client (e.g. from the game).
    pub number: u32,
    pub edition: GameEdition,
    /// Minimum Astryn version that understands this build, `major.minor.patch`.
    /// Empty means "any".
    pub min_astryn: String,
    /// Known-bad builds are blocked with a reason, never silently admitted.
    pub blocked: Option<String>,
    /// Certification note (lab run, evidence link). Empty = uncertified.
    pub note: String,
}

/// Assessment of a connecting client's game build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildAssessment {
    /// Known build, supported edition, not blocked, Astryn new enough.
    Supported { build: GameBuild },
    /// Known build, but the client Astryn is older than `min_astryn`.
    RequiresAstrynUpdate { build: GameBuild, min_astryn: String },
    /// Known build, blocked for the stated reason.
    Blocked { build: GameBuild, reason: String },
    /// Edition recognized but this exact build was never certified.
    UnknownBuild { edition: GameEdition, number: u32 },
    /// The edition itself is not supported by this Aldivine release.
    UnsupportedEdition { edition: GameEdition, note: String },
}

impl BuildAssessment {
    /// Whether the client may proceed to join.
    pub fn admittable(&self) -> bool {
        matches!(self, BuildAssessment::Supported { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BuildError {
    #[error("invalid min_astryn '{0}' (expected major.minor.patch)")]
    BadAstrynVersion(String),
}

/// Registry of known builds, keyed by (edition, number).
#[derive(Debug, Default)]
pub struct BuildRegistry {
    builds: BTreeMap<(GameEdition, u32), GameBuild>,
    profiles: BTreeMap<GameEdition, EditionProfile>,
}

impl BuildRegistry {
    pub fn new() -> Self {
        BuildRegistry::default()
    }

    /// Declare an edition profile (supported or refused-with-reason).
    pub fn set_profile(&mut self, profile: EditionProfile) {
        self.profiles.insert(profile.edition, profile);
    }

    /// Register one known build. Rejects malformed `min_astryn` so bad data
    /// cannot enter the registry quietly.
    pub fn add(&mut self, build: GameBuild) -> Result<(), BuildError> {
        if !build.min_astryn.is_empty() {
            parse_triple(&build.min_astryn).ok_or_else(|| BuildError::BadAstrynVersion(build.min_astryn.clone()))?;
        }
        self.builds.insert((build.edition, build.number), build);
        Ok(())
    }

    pub fn build_count(&self) -> usize {
        self.builds.len()
    }

    /// Assess a connecting client. `astryn_version` is this client's
    /// `major.minor.patch`; unparsable client versions are treated as too
    /// old (fail closed), never as "probably fine".
    pub fn assess(&self, edition: GameEdition, number: u32, astryn_version: &str) -> BuildAssessment {
        if let Some(profile) = self.profiles.get(&edition) {
            if !profile.supported {
                return BuildAssessment::UnsupportedEdition { edition, note: profile.note.clone() };
            }
        }
        // No profile = edition not declared = unsupported. Absence of a
        // profile must not read as support.
        if !self.profiles.contains_key(&edition) {
            return BuildAssessment::UnsupportedEdition { edition, note: "edition not declared".into() };
        }
        let build = match self.builds.get(&(edition, number)) {
            Some(b) => b.clone(),
            None => return BuildAssessment::UnknownBuild { edition, number },
        };
        if let Some(reason) = build.blocked.clone() {
            return BuildAssessment::Blocked { build, reason };
        }
        if !build.min_astryn.is_empty() {
            let min = parse_triple(&build.min_astryn).expect("validated at add()");
            match parse_triple(astryn_version) {
                Some(client) if client >= min => {}
                _ => {
                    return BuildAssessment::RequiresAstrynUpdate { min_astryn: build.min_astryn.clone(), build };
                }
            }
        }
        BuildAssessment::Supported { build }
    }
}

fn parse_triple(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Operator policy for unknown builds: refuse, or pin to last-known-good.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmergencyPin {
    pub edition: GameEdition,
    /// Explicit opt-in. Default (false) refuses unknown builds.
    pub allow_pin: bool,
    /// Build number whose profile the pinned client is treated under.
    pub last_good_build: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    /// Policy refuses; client sees the reason.
    Refused { edition: GameEdition, number: u32, reason: String },
    /// Pinned: treat as `last_good_build` with a visible warning. The
    /// warning text is part of the outcome so callers cannot drop it.
    Pinned { edition: GameEdition, number: u32, as_build: u32, warning: String },
}

/// Apply emergency policy to an unknown build. Known assessments pass
/// through untouched — pinning never overrides a Blocked verdict.
pub fn apply_emergency_policy(
    assessment: &BuildAssessment,
    pin: &EmergencyPin,
    registry: &BuildRegistry,
) -> Option<PinOutcome> {
    let (edition, number) = match assessment {
        BuildAssessment::UnknownBuild { edition, number } => (*edition, *number),
        _ => return None,
    };
    if !pin.allow_pin || pin.edition != edition {
        return Some(PinOutcome::Refused {
            edition,
            number,
            reason: "Game build not recognized by this server.".into(),
        });
    }
    match registry.builds.get(&(edition, pin.last_good_build)) {
        Some(_) => Some(PinOutcome::Pinned {
            edition,
            number,
            as_build: pin.last_good_build,
            warning: format!(
                "Unrecognized {} build {number}; running under last-known-good profile {}. Update Aldivine for full support.",
                edition.as_str(),
                pin.last_good_build
            ),
        }),
        None => Some(PinOutcome::Refused {
            edition,
            number,
            reason: "Game build not recognized and no safe fallback is configured.".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic build numbers (90000+) that cannot collide with real ones:
    /// these tests prove registry logic, never real-game coverage.
    const SYN_A: u32 = 90001;
    const SYN_B: u32 = 90002;
    const SYN_UNKNOWN: u32 = 90999;

    fn supported_registry() -> BuildRegistry {
        let mut r = BuildRegistry::new();
        r.set_profile(EditionProfile::supported(GameEdition::Enhanced));
        r.set_profile(EditionProfile::supported(GameEdition::Legacy));
        r.add(GameBuild {
            number: SYN_A,
            edition: GameEdition::Enhanced,
            min_astryn: "1.4.0".into(),
            blocked: None,
            note: "synthetic test entry".into(),
        })
        .unwrap();
        r.add(GameBuild {
            number: SYN_B,
            edition: GameEdition::Enhanced,
            min_astryn: "".into(),
            blocked: Some("synthetic crash regression".into()),
            note: "synthetic test entry".into(),
        })
        .unwrap();
        r
    }

    #[test]
    fn known_build_supported() {
        let r = supported_registry();
        match r.assess(GameEdition::Enhanced, SYN_A, "1.4.0") {
            BuildAssessment::Supported { build } => assert_eq!(build.number, SYN_A),
            other => panic!("expected Supported, got {other:?}"),
        }
    }

    #[test]
    fn newer_astryn_also_supported() {
        let r = supported_registry();
        assert!(r.assess(GameEdition::Enhanced, SYN_A, "2.0.0").admittable());
    }

    #[test]
    fn old_astryn_must_update() {
        let r = supported_registry();
        match r.assess(GameEdition::Enhanced, SYN_A, "1.3.9") {
            BuildAssessment::RequiresAstrynUpdate { min_astryn, .. } => assert_eq!(min_astryn, "1.4.0"),
            other => panic!("expected RequiresAstrynUpdate, got {other:?}"),
        }
    }

    #[test]
    fn unparsable_client_version_fails_closed() {
        let r = supported_registry();
        assert!(matches!(
            r.assess(GameEdition::Enhanced, SYN_A, "dev-build"),
            BuildAssessment::RequiresAstrynUpdate { .. }
        ));
    }

    #[test]
    fn blocked_build_never_admits() {
        let r = supported_registry();
        match r.assess(GameEdition::Enhanced, SYN_B, "9.9.9") {
            BuildAssessment::Blocked { reason, .. } => assert!(reason.contains("crash")),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn unknown_build_refused_assessment() {
        let r = supported_registry();
        assert_eq!(
            r.assess(GameEdition::Enhanced, SYN_UNKNOWN, "9.9.9"),
            BuildAssessment::UnknownBuild { edition: GameEdition::Enhanced, number: SYN_UNKNOWN }
        );
        assert!(!r.assess(GameEdition::Enhanced, SYN_UNKNOWN, "9.9.9").admittable());
    }

    #[test]
    fn undeclared_edition_is_unsupported() {
        let mut r = BuildRegistry::new();
        r.set_profile(EditionProfile::supported(GameEdition::Enhanced));
        assert!(matches!(r.assess(GameEdition::Legacy, SYN_A, "9.9.9"), BuildAssessment::UnsupportedEdition { .. }));
    }

    #[test]
    fn refused_edition_carries_note() {
        let mut r = BuildRegistry::new();
        r.set_profile(EditionProfile::unsupported(GameEdition::Legacy, "bridge not yet ported"));
        match r.assess(GameEdition::Legacy, SYN_A, "9.9.9") {
            BuildAssessment::UnsupportedEdition { note, .. } => assert!(note.contains("bridge")),
            other => panic!("expected UnsupportedEdition, got {other:?}"),
        }
    }

    #[test]
    fn malformed_min_astryn_rejected_at_add() {
        let mut r = BuildRegistry::new();
        for bad in ["1.4", "v1.4.0", "1.4.0.1", "latest", ""] {
            if bad.is_empty() {
                continue; // empty means "any", legal
            }
            let err = r
                .add(GameBuild {
                    number: SYN_A,
                    edition: GameEdition::Enhanced,
                    min_astryn: bad.into(),
                    blocked: None,
                    note: String::new(),
                })
                .unwrap_err();
            assert_eq!(err, BuildError::BadAstrynVersion(bad.into()), "{bad:?}");
        }
    }

    #[test]
    fn emergency_default_refuses_unknown() {
        let r = supported_registry();
        let a = r.assess(GameEdition::Enhanced, SYN_UNKNOWN, "9.9.9");
        let pin = EmergencyPin { edition: GameEdition::Enhanced, allow_pin: false, last_good_build: SYN_A };
        assert!(matches!(apply_emergency_policy(&a, &pin, &r), Some(PinOutcome::Refused { .. })));
    }

    #[test]
    fn emergency_pin_needs_matching_edition_and_known_fallback() {
        let r = supported_registry();
        let a = r.assess(GameEdition::Enhanced, SYN_UNKNOWN, "9.9.9");
        // Wrong edition on the pin: refuse.
        let wrong = EmergencyPin { edition: GameEdition::Legacy, allow_pin: true, last_good_build: SYN_A };
        assert!(matches!(apply_emergency_policy(&a, &wrong, &r), Some(PinOutcome::Refused { .. })));
        // Unknown fallback build: refuse, not a blind pin.
        let bad_fb = EmergencyPin { edition: GameEdition::Enhanced, allow_pin: true, last_good_build: SYN_UNKNOWN };
        assert!(matches!(apply_emergency_policy(&a, &bad_fb, &r), Some(PinOutcome::Refused { .. })));
    }

    #[test]
    fn emergency_pin_carries_warning() {
        let r = supported_registry();
        let a = r.assess(GameEdition::Enhanced, SYN_UNKNOWN, "9.9.9");
        let pin = EmergencyPin { edition: GameEdition::Enhanced, allow_pin: true, last_good_build: SYN_A };
        match apply_emergency_policy(&a, &pin, &r) {
            Some(PinOutcome::Pinned { as_build, warning, .. }) => {
                assert_eq!(as_build, SYN_A);
                assert!(warning.contains(&SYN_UNKNOWN.to_string()));
            }
            other => panic!("expected Pinned, got {other:?}"),
        }
    }

    #[test]
    fn emergency_never_overrides_known_verdicts() {
        let r = supported_registry();
        let pin = EmergencyPin { edition: GameEdition::Enhanced, allow_pin: true, last_good_build: SYN_A };
        for a in [
            r.assess(GameEdition::Enhanced, SYN_A, "9.9.9"),
            r.assess(GameEdition::Enhanced, SYN_B, "9.9.9"),
            r.assess(GameEdition::Enhanced, SYN_A, "0.0.1"),
        ] {
            assert_eq!(apply_emergency_policy(&a, &pin, &r), None, "{a:?}");
        }
    }
}
