//! NovaGate Launcher — core backend.
//!
//! The launcher's first real job: find a GTA V installation, identify which
//! distribution it came from, and report that *separately* from entitlement.
//!
//! Critical distinction the whole security model rests on:
//!   INSTALLATION_DETECTED  !=  ENTITLEMENT_VERIFIED
//! Finding game files on disk proves the files exist. It does not prove the
//! player owns the game. Detection must never be reported as ownership.
//!
//! Detection uses only legitimate read-only signals:
//!   - Windows registry uninstall keys (publisher-written install metadata)
//!   - well-known Steam library locations + steamapps manifests
//!   - Epic manifest files
//!   - the expected game file structure (the decisive signal)
//!
//! It never modifies the GTA V installation, reads no credentials, and scans
//! no private session data.

use serde::{Deserialize, Serialize};

pub mod browser;
pub mod detection;
pub mod paths;
pub mod update;

pub use browser::{BrowserState, DirectConnect, HistoryEntry, ResolvedConnect, ServerEntry};
pub use detection::{detect_gta_installation, DetectionStatus, GameDistribution, InstallationResult};
pub use paths::AldivinePaths;
pub use update::{validate_update, verify_update_file, UpdateError};

/// Launcher update channels. Never execute unsigned remote binaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateChannel {
    Stable,
    Beta,
    Canary,
    Development,
}

impl UpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
            UpdateChannel::Canary => "canary",
            UpdateChannel::Development => "development",
        }
    }
}

/// A signed launcher update manifest. Hashes and signatures are verified
/// before any file is executed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub channel: UpdateChannel,
    pub minimum_launcher_version: String,
    pub files: Vec<UpdateFile>,
    pub release_notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFile {
    pub path: String,
    pub sha256_hex: String,
    pub size: u64,
    /// Signature over the canonical manifest; see ald-package.
    pub signature_hex: String,
}
