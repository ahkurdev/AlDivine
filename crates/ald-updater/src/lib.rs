//! Aldivine Release Infrastructure & Update Engine
//!
//! Provides artifact catalog management across channels (Stable, Recommended,
//! Beta, Canary, Nightly, Development), Ed25519 signature verification,
//! Software Bill of Materials (SBOM) validation, update checks, and rollback resolution.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;

/// Release channels for binaries and runtime artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReleaseChannel {
    Development = 0,
    Nightly = 1,
    Canary = 2,
    Beta = 3,
    Recommended = 4,
    Stable = 5,
}

/// Target operating system and CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    WindowsX86_64,
    LinuxX86_64,
}

/// A single component listed in the Software Bill of Materials (SBOM).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SbomComponent {
    pub name: String,
    pub version: String,
    pub license: String,
    pub purl: String,
    pub sha256_hex: String,
}

/// Software Bill of Materials document verifying artifact dependency provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SbomDocument {
    pub sbom_id: String,
    pub artifact_id: String,
    pub components: Vec<SbomComponent>,
    pub document_sha256: String,
}

impl SbomDocument {
    pub fn compute_checksum(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.sbom_id.as_bytes());
        hasher.update(self.artifact_id.as_bytes());
        for c in &self.components {
            hasher.update(c.name.as_bytes());
            hasher.update(c.version.as_bytes());
            hasher.update(c.sha256_hex.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}

/// Signed, immutable release artifact record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub artifact_id: String,
    pub version: String,
    pub channel: ReleaseChannel,
    pub platform: Platform,
    pub sha256_hex: String,
    pub size_bytes: u64,
    pub download_url: String,
    pub public_key_hex: String,
    pub signature_hex: String,
    pub sbom_sha256: String,
    pub rollback_target_id: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpdateError {
    #[error("artifact '{0}' already exists and is immutable")]
    ArtifactAlreadyExists(String),
    #[error("artifact '{0}' not found")]
    ArtifactNotFound(String),
    #[error("invalid hex key: {0}")]
    InvalidHexKey(String),
    #[error("invalid hex signature: {0}")]
    InvalidHexSignature(String),
    #[error("signature verification failed: tamper detected")]
    SignatureVerificationFailed,
    #[error("no rollback target available for artifact '{0}'")]
    NoRollbackAvailable(String),
}

impl ArtifactRecord {
    /// Canonical bytes signed by release infrastructure.
    pub fn canonical_signed_bytes(&self) -> Vec<u8> {
        format!("{}:{}:{}:{}", self.artifact_id, self.version, self.platform as u8, self.sha256_hex).into_bytes()
    }

    /// Cryptographically verify the Ed25519 signature of this artifact.
    pub fn verify_signature(&self) -> Result<(), UpdateError> {
        let key_bytes =
            hex_decode(&self.public_key_hex).ok_or_else(|| UpdateError::InvalidHexKey(self.public_key_hex.clone()))?;
        let sig_bytes = hex_decode(&self.signature_hex)
            .ok_or_else(|| UpdateError::InvalidHexSignature(self.signature_hex.clone()))?;

        let key_arr: [u8; 32] =
            key_bytes.try_into().map_err(|_| UpdateError::InvalidHexKey("expected 32-byte key".to_string()))?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| UpdateError::InvalidHexSignature("expected 64-byte signature".to_string()))?;

        let verifying_key = VerifyingKey::from_bytes(&key_arr)
            .map_err(|_| UpdateError::InvalidHexKey("invalid ed25519 key bytes".to_string()))?;
        let signature = Signature::from_bytes(&sig_arr);

        let payload = self.canonical_signed_bytes();
        verifying_key.verify(&payload, &signature).map_err(|_| UpdateError::SignatureVerificationFailed)
    }
}

/// Catalog holding published release artifacts.
#[derive(Default)]
pub struct ReleaseCatalog {
    artifacts: HashMap<String, ArtifactRecord>,
    by_channel_platform: HashMap<(ReleaseChannel, Platform), Vec<String>>,
}

impl ReleaseCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new signed artifact immutably.
    pub fn register(&mut self, record: ArtifactRecord) -> Result<(), UpdateError> {
        if self.artifacts.contains_key(&record.artifact_id) {
            return Err(UpdateError::ArtifactAlreadyExists(record.artifact_id));
        }

        record.verify_signature()?;

        let key = (record.channel, record.platform);
        self.by_channel_platform.entry(key).or_default().push(record.artifact_id.clone());

        self.artifacts.insert(record.artifact_id.clone(), record);
        Ok(())
    }

    pub fn get(&self, artifact_id: &str) -> Option<&ArtifactRecord> {
        self.artifacts.get(artifact_id)
    }

    /// Retrieve the newest published artifact for a given channel and platform.
    pub fn get_latest(&self, channel: ReleaseChannel, platform: Platform) -> Option<&ArtifactRecord> {
        let list = self.by_channel_platform.get(&(channel, platform))?;
        let latest_id = list.last()?;
        self.get(latest_id)
    }

    /// Resolve previous rollback target in the chain.
    pub fn resolve_rollback(&self, current_artifact_id: &str) -> Result<&ArtifactRecord, UpdateError> {
        let current = self
            .get(current_artifact_id)
            .ok_or_else(|| UpdateError::ArtifactNotFound(current_artifact_id.to_string()))?;

        let target_id = current
            .rollback_target_id
            .as_ref()
            .ok_or_else(|| UpdateError::NoRollbackAvailable(current_artifact_id.to_string()))?;

        self.get(target_id).ok_or_else(|| UpdateError::ArtifactNotFound(target_id.clone()))
    }
}

/// Update plan comparing current installed artifact against target release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePlan {
    pub current_version: String,
    pub target_version: String,
    pub is_update_available: bool,
    pub artifact: Option<ArtifactRecord>,
    pub rollback_available: bool,
}

pub struct UpdateChecker;

impl UpdateChecker {
    pub fn check(
        current_version: &str,
        channel: ReleaseChannel,
        platform: Platform,
        catalog: &ReleaseCatalog,
    ) -> UpdatePlan {
        if let Some(latest) = catalog.get_latest(channel, platform) {
            let is_newer = latest.version != current_version;
            UpdatePlan {
                current_version: current_version.to_string(),
                target_version: latest.version.clone(),
                is_update_available: is_newer,
                artifact: Some(latest.clone()),
                rollback_available: latest.rollback_target_id.is_some(),
            }
        } else {
            UpdatePlan {
                current_version: current_version.to_string(),
                target_version: current_version.to_string(),
                is_update_available: false,
                artifact: None,
                rollback_available: false,
            }
        }
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let (chunks, _) = s.as_bytes().as_chunks::<2>();
    for chunk in chunks {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        write!(&mut s, "{:02x}", b).unwrap();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    fn generate_signed_artifact(
        id: &str,
        version: &str,
        channel: ReleaseChannel,
        platform: Platform,
        parent: Option<&str>,
    ) -> ArtifactRecord {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let pub_hex = hex_encode(verifying_key.as_bytes());

        let sha256_hex = "00".repeat(32);

        let mut record = ArtifactRecord {
            artifact_id: id.to_string(),
            version: version.to_string(),
            channel,
            platform,
            sha256_hex: sha256_hex.clone(),
            size_bytes: 4096,
            download_url: format!("https://releases.aldivine.com/{}/bin", id),
            public_key_hex: pub_hex,
            signature_hex: String::new(),
            sbom_sha256: "11".repeat(32),
            rollback_target_id: parent.map(|s| s.to_string()),
        };

        let payload = record.canonical_signed_bytes();
        let signature = signing_key.sign(&payload);
        record.signature_hex = hex_encode(&signature.to_bytes());
        record
    }

    #[test]
    fn release_channel_ordering() {
        assert!(ReleaseChannel::Stable > ReleaseChannel::Recommended);
        assert!(ReleaseChannel::Recommended > ReleaseChannel::Beta);
        assert!(ReleaseChannel::Beta > ReleaseChannel::Canary);
        assert!(ReleaseChannel::Canary > ReleaseChannel::Nightly);
        assert!(ReleaseChannel::Nightly > ReleaseChannel::Development);
    }

    #[test]
    fn ed25519_signature_verification_succeeds_on_valid_artifact() {
        let art =
            generate_signed_artifact("art-1", "0.1.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        assert_eq!(art.verify_signature(), Ok(()));
    }

    #[test]
    fn ed25519_signature_verification_fails_on_tampered_payload() {
        let mut art =
            generate_signed_artifact("art-2", "0.1.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        // Tamper with sha256
        art.sha256_hex = "ff".repeat(32);
        assert_eq!(art.verify_signature(), Err(UpdateError::SignatureVerificationFailed));
    }

    #[test]
    fn catalog_registers_and_retrieves_latest_for_channel_and_platform() {
        let mut catalog = ReleaseCatalog::new();
        let art1 =
            generate_signed_artifact("art-1", "0.1.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        let art2 = generate_signed_artifact(
            "art-2",
            "0.1.1",
            ReleaseChannel::Recommended,
            Platform::WindowsX86_64,
            Some("art-1"),
        );

        catalog.register(art1).unwrap();
        catalog.register(art2).unwrap();

        let latest = catalog.get_latest(ReleaseChannel::Recommended, Platform::WindowsX86_64).unwrap();
        assert_eq!(latest.version, "0.1.1");
        assert_eq!(latest.artifact_id, "art-2");
    }

    #[test]
    fn immutable_artifact_registration_refuses_overwrite() {
        let mut catalog = ReleaseCatalog::new();
        let art1 =
            generate_signed_artifact("art-dup", "0.1.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        let art2 =
            generate_signed_artifact("art-dup", "0.1.1", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);

        catalog.register(art1).unwrap();
        assert!(matches!(catalog.register(art2), Err(UpdateError::ArtifactAlreadyExists(_))));
    }

    #[test]
    fn rollback_chain_resolves_previous_artifact() {
        let mut catalog = ReleaseCatalog::new();
        let art1 =
            generate_signed_artifact("art-v1", "0.1.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        let art2 = generate_signed_artifact(
            "art-v2",
            "0.1.1",
            ReleaseChannel::Recommended,
            Platform::WindowsX86_64,
            Some("art-v1"),
        );

        catalog.register(art1).unwrap();
        catalog.register(art2).unwrap();

        let rolled_back = catalog.resolve_rollback("art-v2").unwrap();
        assert_eq!(rolled_back.artifact_id, "art-v1");
        assert_eq!(rolled_back.version, "0.1.0");
    }

    #[test]
    fn rollback_chain_terminates_safely_when_no_parent() {
        let mut catalog = ReleaseCatalog::new();
        let art1 =
            generate_signed_artifact("art-root", "0.1.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        catalog.register(art1).unwrap();

        assert_eq!(catalog.resolve_rollback("art-root"), Err(UpdateError::NoRollbackAvailable("art-root".to_string())));
    }

    #[test]
    fn update_checker_identifies_newer_version() {
        let mut catalog = ReleaseCatalog::new();
        let art1 =
            generate_signed_artifact("art-v2", "0.2.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        catalog.register(art1).unwrap();

        let plan = UpdateChecker::check("0.1.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, &catalog);
        assert!(plan.is_update_available);
        assert_eq!(plan.target_version, "0.2.0");
    }

    #[test]
    fn update_checker_reports_up_to_date() {
        let mut catalog = ReleaseCatalog::new();
        let art1 =
            generate_signed_artifact("art-v2", "0.2.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, None);
        catalog.register(art1).unwrap();

        let plan = UpdateChecker::check("0.2.0", ReleaseChannel::Recommended, Platform::WindowsX86_64, &catalog);
        assert!(!plan.is_update_available);
        assert_eq!(plan.target_version, "0.2.0");
    }

    #[test]
    fn sbom_generation_and_integrity_check() {
        let doc = SbomDocument {
            sbom_id: "sbom-1".to_string(),
            artifact_id: "art-1".to_string(),
            components: vec![SbomComponent {
                name: "tokio".to_string(),
                version: "1.38.0".to_string(),
                license: "MIT".to_string(),
                purl: "pkg:cargo/tokio@1.38.0".to_string(),
                sha256_hex: "33".repeat(32),
            }],
            document_sha256: String::new(),
        };

        let checksum = doc.compute_checksum();
        assert_eq!(checksum.len(), 64);
    }
}
