//! NovaGate update system.
//!
//! Launcher and client binaries are fetched over the network and then
//! executed, which makes the update path one of the most dangerous surfaces
//! in the platform. Rules enforced here:
//!
//! 1. Never execute an unsigned remote binary.
//! 2. Verify the manifest signature before downloading the payload.
//! 3. Verify every file hash after download, before execution.
//! 4. Reject version downgrades and channel violations.
//! 5. Reject manifests from a channel the user is not on.

use crate::{UpdateChannel, UpdateFile, UpdateManifest};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("manifest signature could not be verified")]
    BadSignature,
    #[error("file '{0}' hash mismatch: expected {1}, got {2}")]
    HashMismatch(String, String, String),
    #[error("current launcher version {current} is below the manifest minimum {minimum}")]
    BelowMinimum { current: String, minimum: String },
    #[error("downgrade refused: current {current} > offered {offered}")]
    Downgrade { current: String, offered: String },
    #[error("manifest channel {manifest} does not match selected channel {selected}")]
    ChannelMismatch { manifest: String, selected: String },
    #[error("manifest has no files")]
    Empty,
}

/// Policy decision on an offered manifest, before anything is downloaded.
///
/// `manifest_signature_ok` is the result of verifying the publisher signature
/// over the canonical manifest bytes (see ald-package); it is supplied by the
/// caller so this module stays free of key handling.
pub fn validate_update(
    manifest: &UpdateManifest,
    current_version: &str,
    selected_channel: UpdateChannel,
    manifest_signature_ok: bool,
) -> Result<(), UpdateError> {
    if !manifest_signature_ok {
        return Err(UpdateError::BadSignature);
    }
    if manifest.files.is_empty() {
        return Err(UpdateError::Empty);
    }
    if manifest.channel.as_str() != selected_channel.as_str() {
        return Err(UpdateError::ChannelMismatch {
            manifest: manifest.channel.as_str().into(),
            selected: selected_channel.as_str().into(),
        });
    }
    if !satisfies_minimum(current_version, &manifest.minimum_launcher_version) {
        return Err(UpdateError::BelowMinimum {
            current: current_version.into(),
            minimum: manifest.minimum_launcher_version.clone(),
        });
    }
    // Downgrades are refused so a compromised or buggy manifest cannot move a
    // user to a vulnerable older build.
    if is_downgrade(current_version, &manifest.version) {
        return Err(UpdateError::Downgrade { current: current_version.into(), offered: manifest.version.clone() });
    }
    Ok(())
}

/// Verify one downloaded file against its manifest entry.
pub fn verify_update_file(file: &UpdateFile, bytes: &[u8]) -> Result<(), UpdateError> {
    let actual = sha256_hex(bytes);
    if actual != file.sha256_hex {
        return Err(UpdateError::HashMismatch(file.path.clone(), file.sha256_hex.clone(), actual));
    }
    Ok(())
}

/// A minimal semver major.minor.patch comparator, enough for update gating.
/// Non-semver strings compare as equal-ordering (no upgrade, no downgrade).
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v).split('+').next()?;
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next().unwrap_or("0").parse().ok()?;
    let patch: u64 = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn satisfies_minimum(current: &str, minimum: &str) -> bool {
    match (parse_semver(current), parse_semver(minimum)) {
        (Some(c), Some(m)) => c >= m,
        // Unparseable versions are treated as satisfying the minimum rather
        // than hard-blocking an update; downgrade detection still applies.
        _ => true,
    }
}

fn is_downgrade(current: &str, offered: &str) -> bool {
    match (parse_semver(current), parse_semver(offered)) {
        (Some(c), Some(o)) => o < c,
        _ => false,
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, channel: UpdateChannel, minimum: &str) -> UpdateManifest {
        UpdateManifest {
            version: version.into(),
            channel,
            minimum_launcher_version: minimum.into(),
            files: vec![UpdateFile {
                path: "NovaGate.exe".into(),
                sha256_hex: sha256_hex(b"payload"),
                size: 7,
                signature_hex: "sig".into(),
            }],
            release_notes: String::new(),
        }
    }

    #[test]
    fn valid_update_accepted() {
        let m = manifest("1.2.0", UpdateChannel::Stable, "1.0.0");
        assert!(validate_update(&m, "1.1.0", UpdateChannel::Stable, true).is_ok());
    }

    #[test]
    fn unsigned_manifest_rejected() {
        let m = manifest("1.2.0", UpdateChannel::Stable, "1.0.0");
        assert!(matches!(validate_update(&m, "1.1.0", UpdateChannel::Stable, false), Err(UpdateError::BadSignature)));
    }

    #[test]
    fn channel_mismatch_rejected() {
        // A canary manifest must not install onto a stable channel.
        let m = manifest("1.2.0", UpdateChannel::Canary, "1.0.0");
        assert!(matches!(
            validate_update(&m, "1.1.0", UpdateChannel::Stable, true),
            Err(UpdateError::ChannelMismatch { .. })
        ));
    }

    #[test]
    fn below_minimum_rejected() {
        let m = manifest("1.2.0", UpdateChannel::Stable, "2.0.0");
        assert!(matches!(
            validate_update(&m, "1.1.0", UpdateChannel::Stable, true),
            Err(UpdateError::BelowMinimum { .. })
        ));
    }

    #[test]
    fn downgrade_rejected() {
        let m = manifest("1.0.0", UpdateChannel::Stable, "1.0.0");
        assert!(matches!(
            validate_update(&m, "1.5.0", UpdateChannel::Stable, true),
            Err(UpdateError::Downgrade { .. })
        ));
    }

    #[test]
    fn empty_manifest_rejected() {
        let mut m = manifest("1.2.0", UpdateChannel::Stable, "1.0.0");
        m.files.clear();
        assert!(matches!(validate_update(&m, "1.1.0", UpdateChannel::Stable, true), Err(UpdateError::Empty)));
    }

    #[test]
    fn correct_payload_verifies() {
        let f = &manifest("1.0.0", UpdateChannel::Stable, "1.0.0").files[0];
        assert!(verify_update_file(f, b"payload").is_ok());
    }

    #[test]
    fn tampered_payload_rejected() {
        let f = &manifest("1.0.0", UpdateChannel::Stable, "1.0.0").files[0];
        assert!(matches!(verify_update_file(f, b"malicious"), Err(UpdateError::HashMismatch(..))));
    }

    #[test]
    fn semver_comparison() {
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("v2.0.0+build"), Some((2, 0, 0)));
        assert!(is_downgrade("1.2.3", "1.2.2"));
        assert!(!is_downgrade("1.2.3", "1.2.3"));
        assert!(satisfies_minimum("1.2.3", "1.2.3"));
        assert!(!satisfies_minimum("1.2.2", "1.2.3"));
    }

    #[test]
    fn non_semver_does_not_trigger_downgrade() {
        assert!(!is_downgrade("dev-build-xyz", "1.0.0"));
    }
}
