//! Aldivine package format (`.alpkg`) and signature verification.
//!
//! Signing protects against tampering, modified mirrors, malicious packages,
//! and supply-chain substitution. Signing is NOT encryption: a signed package
//! is still readable by anyone, it is just provably unmodified.
//!
//! Design notes:
//! - The manifest and the payload are signed separately. The manifest is small
//!   and can be verified before downloading the payload at all, which is how
//!   the client avoids fetching a tampered multi-gigabyte bundle.
//! - Hashes are SHA-256 over the *uncompressed* payload bytes.
//! - Verification never trusts the manifest's own claim about hashes when it
//!   can recompute; the signature covers the manifest, then the manifest's
//!   hashes are checked against the actual payload.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use ed25519_dalek::Verifier;

/// Metadata block embedded in a `.alpkg`. Signed by the publisher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub package_id: String,
    pub publisher_id: String,
    pub name: String,
    pub version: String,
    /// Semver-compatible minimum runtime required.
    pub minimum_runtime: String,
    pub framework: Option<String>,
    pub dependencies: Vec<String>,
    /// sha256 hex of each payload file, in deterministic order.
    pub file_hashes: Vec<FileHash>,
    pub total_size: u64,
    pub protection: ProtectionPolicy,
    /// Entitlement requirement; see ald-entitlement.
    pub entitlement_model: String,
    /// Publisher signature over the canonical manifest bytes.
    pub signature: Signature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHash {
    pub path: String,
    pub sha256_hex: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtectionPolicy {
    Open,
    Signed,
    Protected,
    Private,
}

/// An Ed25519 signature, stored base64 so the manifest stays text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub algorithm: String,
    pub public_key_hex: String,
    pub signature_hex: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("malformed package manifest: {0}")]
    Malformed(String),
    #[error("payload file '{path}' hash mismatch: manifest says {expected}, got {actual}")]
    HashMismatch { path: String, expected: String, actual: String },
    #[error("signature verification failed for package '{0}'")]
    BadSignature(String),
    #[error("package '{0}' requires a signature and has none")]
    MissingSignature(String),
    #[error("unsupported signature algorithm '{0}'")]
    UnsupportedAlgorithm(String),
}

impl Signature {
    pub const ALGO: &'static str = "ed25519";

    /// Sign a canonical byte buffer with a publisher key pair.
    pub fn sign(signing_key: &ed25519_dalek::SigningKey, message: &[u8]) -> Self {
        use ed25519_dalek::Signer;
        let sig = signing_key.sign(message);
        Signature {
            algorithm: Self::ALGO.into(),
            public_key_hex: hex_encode(&signing_key.verifying_key().to_bytes()),
            signature_hex: hex_encode(&sig.to_bytes()),
        }
    }

    /// Verify a canonical byte buffer against this signature.
    pub fn verify(&self, package_id: &str, message: &[u8]) -> Result<(), PackageError> {
        if self.algorithm != Self::ALGO {
            return Err(PackageError::UnsupportedAlgorithm(self.algorithm.clone()));
        }
        let pk_bytes =
            hex_decode_fixed_32(&self.public_key_hex).ok_or_else(|| PackageError::Malformed("public key".into()))?;
        let sig_bytes =
            hex_decode_fixed_64(&self.signature_hex).ok_or_else(|| PackageError::Malformed("signature".into()))?;
        let verifying = ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes)
            .map_err(|_| PackageError::Malformed("public key".into()))?;
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        verifying.verify(message, &signature).map_err(|_| PackageError::BadSignature(package_id.to_string()))
    }
}

/// Deterministic canonical form of the manifest for signing.
///
/// The signed bytes MUST be byte-stable across platforms and crate versions,
/// so we serialize the fields that matter in fixed order and strip the
/// signature itself (a signature cannot cover itself).
pub fn canonical_manifest_bytes(manifest: &PackageManifest) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(manifest.package_id.as_bytes());
    out.push(0);
    out.extend_from_slice(manifest.publisher_id.as_bytes());
    out.push(0);
    out.extend_from_slice(manifest.name.as_bytes());
    out.push(0);
    out.extend_from_slice(manifest.version.as_bytes());
    out.push(0);
    out.extend_from_slice(manifest.minimum_runtime.as_bytes());
    out.push(0);
    out.extend_from_slice(manifest.framework.as_deref().unwrap_or("").as_bytes());
    out.push(0);
    for dep in &manifest.dependencies {
        out.extend_from_slice(dep.as_bytes());
        out.push(0);
    }
    for fh in &manifest.file_hashes {
        out.extend_from_slice(fh.path.as_bytes());
        out.push(0);
        out.extend_from_slice(fh.sha256_hex.as_bytes());
        out.push(0);
        out.extend_from_slice(fh.size.to_string().as_bytes());
        out.push(0);
    }
    out.extend_from_slice(manifest.total_size.to_string().as_bytes());
    out.push(0);
    out.extend_from_slice(manifest.protection.as_str().as_bytes());
    out.push(0);
    out.extend_from_slice(manifest.entitlement_model.as_bytes());
    out
}

impl ProtectionPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            ProtectionPolicy::Open => "OPEN",
            ProtectionPolicy::Signed => "SIGNED",
            ProtectionPolicy::Protected => "PROTECTED",
            ProtectionPolicy::Private => "PRIVATE",
        }
    }
}

/// SHA-256 of an arbitrary buffer, as lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

/// Verify every hash in a manifest against the real payload bytes.
///
/// `payloads` maps the manifest path to the actual file contents. Missing
/// files are an error, not a silent pass.
pub fn verify_payloads(
    manifest: &PackageManifest,
    payloads: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<(), PackageError> {
    for fh in &manifest.file_hashes {
        let data =
            payloads.get(&fh.path).ok_or_else(|| PackageError::Malformed(format!("missing payload '{}'", fh.path)))?;
        let actual = sha256_hex(data);
        if actual != fh.sha256_hex {
            return Err(PackageError::HashMismatch { path: fh.path.clone(), expected: fh.sha256_hex.clone(), actual });
        }
    }
    Ok(())
}

/// Full verification: signature over the canonical manifest, then payload
/// hashes. Call this before executing any package code.
pub fn verify_package(
    manifest: &PackageManifest,
    payloads: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<(), PackageError> {
    match manifest.protection {
        ProtectionPolicy::Open => {}
        ProtectionPolicy::Signed | ProtectionPolicy::Protected | ProtectionPolicy::Private => {
            manifest.signature.verify(&manifest.package_id, &canonical_manifest_bytes(manifest))?;
        }
    }
    verify_payloads(manifest, payloads)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = hex_val(chunk[0])?;
        let lo = hex_val(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

/// Decode hex into a fixed 32-byte buffer (public keys).
fn hex_decode_fixed_32(hex: &str) -> Option<[u8; 32]> {
    let v = hex_decode(hex)?;
    if v.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Some(out)
}

/// Decode hex into a fixed 64-byte buffer (signatures).
fn hex_decode_fixed_64(hex: &str) -> Option<[u8; 64]> {
    let v = hex_decode(hex)?;
    if v.len() != 64 {
        return None;
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&v);
    Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn signed_manifest(payload: &[(String, Vec<u8>)]) -> (PackageManifest, std::collections::HashMap<String, Vec<u8>>) {
        let keys = SigningKey::generate(&mut OsRng);
        let mut file_hashes = Vec::new();
        let mut payloads = std::collections::HashMap::new();
        let mut total = 0u64;
        for (path, data) in payload {
            file_hashes.push(FileHash { path: path.clone(), sha256_hex: sha256_hex(data), size: data.len() as u64 });
            total += data.len() as u64;
            payloads.insert(path.clone(), data.clone());
        }
        let mut manifest = PackageManifest {
            package_id: "pkg".into(),
            publisher_id: "pub".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            minimum_runtime: "0.1.0".into(),
            framework: Some("aldivine".into()),
            dependencies: vec!["dep-a".into()],
            file_hashes,
            total_size: total,
            protection: ProtectionPolicy::Signed,
            entitlement_model: "FREE".into(),
            signature: Signature {
                algorithm: Signature::ALGO.into(),
                public_key_hex: String::new(),
                signature_hex: String::new(),
            },
        };
        let bytes = canonical_manifest_bytes(&manifest);
        manifest.signature = Signature::sign(&keys, &bytes);
        (manifest, payloads)
    }

    #[test]
    fn signed_package_verifies() {
        let (manifest, payloads) = signed_manifest(&[("server/init.lua".into(), b"print('hi')".to_vec())]);
        verify_package(&manifest, &payloads).unwrap();
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let (manifest, mut payloads) = signed_manifest(&[("server/init.lua".into(), b"print('hi')".to_vec())]);
        // Attacker swaps in malicious bytes without re-signing.
        *payloads.get_mut("server/init.lua").unwrap() = b"print('pwned')".to_vec();
        let err = verify_package(&manifest, &payloads).unwrap_err();
        assert!(matches!(err, PackageError::HashMismatch { .. }));
    }

    #[test]
    fn tampered_manifest_is_rejected() {
        let (mut manifest, payloads) = signed_manifest(&[("a".into(), b"aaa".to_vec())]);
        // Attacker bumps the version without re-signing.
        manifest.version = "2.0.0".into();
        assert!(matches!(verify_package(&manifest, &payloads), Err(PackageError::BadSignature(_))));
    }

    #[test]
    fn wrong_key_fails() {
        let (mut manifest, payloads) = signed_manifest(&[("a".into(), b"aaa".to_vec())]);
        // Replace the public key with a different one.
        let other = SigningKey::generate(&mut OsRng);
        manifest.signature.public_key_hex = hex_encode(&other.verifying_key().to_bytes());
        assert!(matches!(verify_package(&manifest, &payloads), Err(PackageError::BadSignature(_))));
    }

    #[test]
    fn open_policy_needs_no_signature() {
        let (mut manifest, payloads) = signed_manifest(&[("a".into(), b"aaa".to_vec())]);
        manifest.protection = ProtectionPolicy::Open;
        manifest.signature.public_key_hex = String::new();
        manifest.signature.signature_hex = String::new();
        verify_package(&manifest, &payloads).unwrap();
    }

    #[test]
    fn canonical_bytes_are_deterministic() {
        let (manifest, _) = signed_manifest(&[("a".into(), b"aaa".to_vec())]);
        let b1 = canonical_manifest_bytes(&manifest);
        let b2 = canonical_manifest_bytes(&manifest);
        assert_eq!(b1, b2);
    }

    #[test]
    fn missing_payload_file_is_an_error() {
        let (manifest, _) = signed_manifest(&[("a".into(), b"aaa".to_vec())]);
        let empty = std::collections::HashMap::new();
        assert!(matches!(verify_package(&manifest, &empty), Err(PackageError::Malformed(_))));
    }

    #[test]
    fn unsupported_algorithm_rejected() {
        let (mut manifest, payloads) = signed_manifest(&[("a".into(), b"aaa".to_vec())]);
        manifest.signature.algorithm = "rsa-4096".into();
        assert!(matches!(verify_package(&manifest, &payloads), Err(PackageError::UnsupportedAlgorithm(_))));
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(sha256_hex(b"hello"), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn hex_roundtrip() {
        let data = [0u8, 1, 255, 128, 64];
        assert_eq!(hex_decode(&hex_encode(&data)).unwrap(), data);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("gg").is_none());
    }
}
