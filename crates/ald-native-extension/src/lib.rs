//! Aldivine Native Extension ABI — manifest, negotiation, trust, and loading.
//!
//! Ordinary resources may NOT load arbitrary native binaries by default. An
//! extension loads only after it passes, in order:
//!
//! 1. **Manifest validation** — well-formed `extension.toml`, known
//!    capabilities only, safe library filename (bare filename, no traversal).
//! 2. **ABI negotiation** — the extension's `abi_version` must equal
//!    [`HOST_ABI`], and its `requires_runtime` semver range must match this
//!    runtime. Mismatch is a hard refusal, never a silent downgrade.
//! 3. **Trust gate** — Ed25519 signature over canonical bytes covering the
//!    manifest AND the sha256 of the library binary. Unsigned loads are
//!    refused unless the operator explicitly enables them. Signed loads
//!    require the key to be in the operator's trust list (default-deny).
//! 4. **Capability policy** — every requested capability must be known AND
//!    granted by operator policy (default: none granted).
//!
//! Only then is the library opened (via `libloading`, the same loader the
//! build grafts through `clang-sys` — no new native boundary) and the
//! required `ald_extension_init` entry point resolved and called.
//!
//! What this crate does NOT do: sandbox native code. A loaded extension runs
//! with the host's privileges. The trust gate keeps untrusted bytes out; it
//! does not contain trusted-but-buggy code. That boundary is documented, not
//! pretended.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// ABI version this host speaks. Extensions declaring anything else refuse
/// to load with [`ExtError::AbiMismatch`].
pub const HOST_ABI: u32 = 1;

/// Name of the required C entry point every extension must export:
/// `int32_t ald_extension_init(uint32_t host_abi)`. Return 0 on success.
pub const INIT_SYMBOL: &str = "ald_extension_init";

/// Runtime version used for `requires_runtime` negotiation.
pub const HOST_RUNTIME: &str = env!("CARGO_PKG_VERSION");

/// Capabilities a native extension may request. Unknown tokens are rejected
/// at authorize time — a typo must fail closed, not grant nothing silently.
pub const KNOWN_CAPABILITIES: &[&str] = &[
    "game.natives.call",
    "game.entities.read",
    "game.entities.write",
    "net.events.send",
    "net.events.recv",
    "db.query",
    "fs.read",
    "fs.write",
    "http.outbound",
    "timers",
    "logging",
];

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExtError {
    #[error("malformed extension manifest: {0}")]
    Malformed(String),
    #[error("extension '{id}' declares unknown capability '{capability}'")]
    UnknownCapability { id: String, capability: String },
    #[error("extension '{id}' wants ABI {wanted} but host speaks {HOST_ABI}")]
    AbiMismatch { id: String, wanted: u32 },
    #[error("extension '{id}' requires runtime '{required}' but host is {HOST_RUNTIME}")]
    RuntimeMismatch { id: String, required: String },
    #[error("extension '{id}' library path '{path}' is not a bare filename")]
    UnsafeLibraryPath { id: String, path: String },
    #[error("extension '{id}' is unsigned and unsigned extensions are not permitted")]
    Unsigned { id: String },
    #[error("signature verification failed for extension '{0}'")]
    BadSignature(String),
    #[error("signing key for extension '{0}' is not in the trust list")]
    UntrustedKey(String),
    #[error("library bytes for extension '{id}' do not match manifest hash")]
    HashMismatch { id: String },
    #[error("extension '{id}' requests capability '{capability}' which policy does not grant")]
    CapabilityDenied { id: String, capability: String },
    #[error("extension '{id}' library failed to open: {reason}")]
    LibraryLoad { id: String, reason: String },
    #[error("extension '{id}' does not export required symbol '{INIT_SYMBOL}'")]
    MissingSymbol { id: String },
    #[error("extension '{id}' init failed with code {code}")]
    InitFailed { id: String, code: i32 },
}

impl ExtError {
    /// Whether this failure should quarantine the extension (operator
    /// attention required) rather than just fail the load. Trust, integrity,
    /// and policy violations quarantine; missing files and init failures are
    /// operational, not adversarial.
    pub fn quarantines(&self) -> bool {
        matches!(
            self,
            ExtError::BadSignature(_)
                | ExtError::UntrustedKey(_)
                | ExtError::Unsigned { .. }
                | ExtError::HashMismatch { .. }
                | ExtError::CapabilityDenied { .. }
                | ExtError::UnknownCapability { .. }
        )
    }
}

/// Extension manifest, normally parsed from `extension.toml` inside the
/// extension directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub extension_id: String,
    pub version: String,
    pub abi_version: u32,
    pub publisher_id: String,
    /// Bare filename of the native library (e.g. `ping.dll`). Paths,
    /// separators, and traversal are rejected at authorize time.
    pub library: String,
    #[serde(default)]
    pub requires_runtime: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub signature: ExtSignature,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtSignature {
    #[serde(default)]
    pub algorithm: String,
    #[serde(default)]
    pub public_key_hex: String,
    #[serde(default)]
    pub signature_hex: String,
}

impl ExtSignature {
    pub const ALGO: &'static str = "ed25519";

    pub fn is_signed(&self) -> bool {
        !self.signature_hex.is_empty()
    }
}

/// Operator policy. Default is deny-everything: unsigned refused, no keys
/// trusted, no capabilities granted. The server operator opens exactly what
/// each deployment needs.
#[derive(Debug, Clone, Default)]
pub struct ExtPolicy {
    pub allow_unsigned: bool,
    /// Lowercase hex of trusted Ed25519 public keys.
    pub trusted_keys: Vec<String>,
    /// Capability tokens this deployment grants to extensions.
    pub allowed_capabilities: Vec<String>,
}

/// An extension that passed manifest, ABI, trust, and policy checks.
/// Loading it (opening the library) is still a separate explicit step.
#[derive(Debug, Clone)]
pub struct Authorized {
    pub manifest: ExtensionManifest,
    pub lib_sha256_hex: String,
    /// Sorted, deduplicated capabilities (subset of manifest request).
    pub capabilities: Vec<String>,
}

impl ExtensionManifest {
    pub fn parse_toml(text: &str) -> Result<Self, ExtError> {
        toml::from_str(text).map_err(|e| ExtError::Malformed(e.to_string()))
    }

    /// Validate structural fields only (identity, version shape, library
    /// filename). Crypto and policy checks happen in [`authorize`].
    fn validate_shape(&self) -> Result<(), ExtError> {
        if self.extension_id.trim().is_empty() {
            return Err(ExtError::Malformed("extension_id is empty".into()));
        }
        if self.extension_id.len() > 128 {
            return Err(ExtError::Malformed("extension_id too long".into()));
        }
        semver::Version::parse(&self.version)
            .map_err(|_| ExtError::Malformed(format!("version '{}' is not semver", self.version)))?;
        if self.publisher_id.trim().is_empty() {
            return Err(ExtError::Malformed("publisher_id is empty".into()));
        }
        // Bare filename only: no separators, no parent refs, no drive or
        // stream syntax. The loader joins this onto the extension dir, so
        // anything else is a breakout attempt.
        if self.library.is_empty()
            || self.library.contains('/')
            || self.library.contains('\\')
            || self.library.contains(':')
            || self.library.split('.').next() == Some("")
            || self.library.split('/').any(|c| c == "..")
            || self.library.split('\\').any(|c| c == "..")
            || Path::new(&self.library).components().count() != 1
        {
            return Err(ExtError::UnsafeLibraryPath { id: self.extension_id.clone(), path: self.library.clone() });
        }
        Ok(())
    }
}

/// Canonical bytes covered by the extension signature: manifest fields in
/// fixed order plus the sha256 of the exact library bytes being loaded.
/// A signature cannot be transplanted onto a different binary or manifest.
pub fn canonical_extension_bytes(manifest: &ExtensionManifest, lib_sha256_hex: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for field in [
        manifest.extension_id.as_str(),
        manifest.version.as_str(),
        &manifest.abi_version.to_string(),
        manifest.publisher_id.as_str(),
        manifest.requires_runtime.as_deref().unwrap_or(""),
    ] {
        out.extend_from_slice(field.as_bytes());
        out.push(0);
    }
    let mut caps: Vec<&str> = manifest.capabilities.iter().map(String::as_str).collect();
    caps.sort_unstable();
    for cap in caps {
        out.extend_from_slice(cap.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(lib_sha256_hex.as_bytes());
    out.push(0);
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks(2) {
        let hi = hex_val(chunk[0])?;
        let lo = hex_val(chunk[1])?;
        out.push((hi << 4) | lo);
    }
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

fn verify_signature(manifest: &ExtensionManifest, lib_sha256_hex: &str, policy: &ExtPolicy) -> Result<(), ExtError> {
    use ed25519_dalek::Verifier;
    let sig = &manifest.signature;
    if !sig.is_signed() {
        if policy.allow_unsigned {
            return Ok(());
        }
        return Err(ExtError::Unsigned { id: manifest.extension_id.clone() });
    }
    if sig.algorithm != ExtSignature::ALGO {
        return Err(ExtError::Malformed(format!("unsupported signature algorithm '{}'", sig.algorithm)));
    }
    let key_hex = sig.public_key_hex.to_lowercase();
    if !policy.trusted_keys.iter().any(|k| k.to_lowercase() == key_hex) {
        return Err(ExtError::UntrustedKey(manifest.extension_id.clone()));
    }
    let pk_bytes = hex_decode(&sig.public_key_hex)
        .filter(|v| v.len() == 32)
        .ok_or_else(|| ExtError::Malformed("public key is not 32-byte hex".into()))?;
    let sig_bytes = hex_decode(&sig.signature_hex)
        .filter(|v| v.len() == 64)
        .ok_or_else(|| ExtError::Malformed("signature is not 64-byte hex".into()))?;
    let mut pk_arr = [0u8; 32];
    pk_arr.copy_from_slice(&pk_bytes);
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let verifying = ed25519_dalek::VerifyingKey::from_bytes(&pk_arr)
        .map_err(|_| ExtError::Malformed("public key rejected".into()))?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);
    let message = canonical_extension_bytes(manifest, lib_sha256_hex);
    verifying.verify(&message, &signature).map_err(|_| ExtError::BadSignature(manifest.extension_id.clone()))
}

/// Run every check short of opening the library: shape, ABI, runtime range,
/// library hash binding, signature + trust, and capability policy.
pub fn authorize(
    manifest: &ExtensionManifest,
    lib_bytes: &[u8],
    expected_lib_sha256_hex: &str,
    policy: &ExtPolicy,
) -> Result<Authorized, ExtError> {
    manifest.validate_shape()?;
    if manifest.abi_version != HOST_ABI {
        return Err(ExtError::AbiMismatch { id: manifest.extension_id.clone(), wanted: manifest.abi_version });
    }
    if let Some(req_str) = manifest.requires_runtime.as_deref() {
        let req = semver::VersionReq::parse(req_str)
            .map_err(|_| ExtError::Malformed(format!("requires_runtime '{req_str}' is not a valid range")))?;
        let host = semver::Version::parse(HOST_RUNTIME).expect("crate version is valid semver");
        if !req.matches(&host) {
            return Err(ExtError::RuntimeMismatch { id: manifest.extension_id.clone(), required: req_str.into() });
        }
    }
    // The hash binds the manifest to the exact bytes about to be loaded.
    // A swapped binary after authorization cannot pass a re-check.
    let actual = sha256_hex(lib_bytes);
    if actual != expected_lib_sha256_hex.to_lowercase() {
        return Err(ExtError::HashMismatch { id: manifest.extension_id.clone() });
    }
    verify_signature(manifest, &actual, policy)?;
    let mut granted: BTreeSet<String> = BTreeSet::new();
    for cap in &manifest.capabilities {
        if !KNOWN_CAPABILITIES.contains(&cap.as_str()) {
            return Err(ExtError::UnknownCapability { id: manifest.extension_id.clone(), capability: cap.clone() });
        }
        if !policy.allowed_capabilities.iter().any(|a| a == cap) {
            return Err(ExtError::CapabilityDenied { id: manifest.extension_id.clone(), capability: cap.clone() });
        }
        granted.insert(cap.clone());
    }
    Ok(Authorized { manifest: manifest.clone(), lib_sha256_hex: actual, capabilities: granted.into_iter().collect() })
}

/// A loaded extension. Dropping it unloads the library.
pub struct LoadedExtension {
    pub extension_id: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub init_code: i32,
    lib: libloading::Library,
}

impl std::fmt::Debug for LoadedExtension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedExtension")
            .field("extension_id", &self.extension_id)
            .field("version", &self.version)
            .field("capabilities", &self.capabilities)
            .field("init_code", &self.init_code)
            .finish_non_exhaustive()
    }
}

impl LoadedExtension {
    /// Probe whether the loaded library exports `name`. Used to verify the
    /// resolve path in both directions without platform-specific hacks.
    pub fn has_symbol(&self, name: &str) -> bool {
        if name.is_empty() || !name.is_ascii() {
            return false;
        }
        // SAFETY: probing only; no call, no dereference.
        unsafe { self.lib.get::<*const std::ffi::c_void>(name.as_bytes()).is_ok() }
    }
}

/// Open the authorized library, resolve [`INIT_SYMBOL`], and call it with
/// [`HOST_ABI`]. Non-zero init codes fail the load.
pub fn load(authorized: &Authorized, extension_dir: &Path) -> Result<LoadedExtension, ExtError> {
    load_with_abi(authorized, extension_dir, HOST_ABI)
}

/// Same as [`load`] but with an explicit ABI value on the init call. The
/// manifest negotiation still applies; this exists so hosts and tests can
/// probe an extension's behavior under a foreign ABI without lying in the
/// manifest. A well-behaved extension must fail non-zero.
pub fn load_with_abi(
    authorized: &Authorized,
    extension_dir: &Path,
    host_abi: u32,
) -> Result<LoadedExtension, ExtError> {
    let id = authorized.manifest.extension_id.clone();
    let lib_path = extension_dir.join(&authorized.manifest.library);
    // SAFETY: `library` was proven a bare filename at authorize time, so the
    // join cannot escape `extension_dir` via traversal. Re-check anyway —
    // authorize and load may be called by different code.
    if authorized.manifest.library.contains('/') || authorized.manifest.library.contains('\\') {
        return Err(ExtError::UnsafeLibraryPath { id, path: authorized.manifest.library.clone() });
    }
    // SAFETY: `lib_path` is confined to `extension_dir` (bare filename
    // re-checked above) and the caller authorized these exact bytes.
    let lib = unsafe { libloading::Library::new(&lib_path) }
        .map_err(|e| ExtError::LibraryLoad { id: id.clone(), reason: e.to_string() })?;
    // SAFETY: signature of the entry point is part of the ABI contract.
    let init: libloading::Symbol<unsafe extern "C" fn(u32) -> i32> =
        unsafe { lib.get(INIT_SYMBOL.as_bytes()) }.map_err(|_| ExtError::MissingSymbol { id: id.clone() })?;
    // SAFETY: the extension is trusted (signature + policy passed) and the
    // call signature is fixed by the ABI. The extension runs in-process —
    // see the module docs; this is not a sandbox.
    let code = unsafe { init(host_abi) };
    if code != 0 {
        return Err(ExtError::InitFailed { id: id.clone(), code });
    }
    Ok(LoadedExtension {
        extension_id: id,
        version: authorized.manifest.version.clone(),
        capabilities: authorized.capabilities.clone(),
        init_code: code,
        lib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn test_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn key_hex(key: &SigningKey) -> String {
        let bytes = key.verifying_key().to_bytes();
        let mut s = String::with_capacity(64);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Fake library bytes stand in for the .dll/.so until the load tests,
    /// which use a real compiled fixture extension (see below).
    fn fake_lib() -> Vec<u8> {
        b"fake-native-library-bytes".to_vec()
    }

    fn signed_manifest(key: &SigningKey, mutate: impl FnOnce(&mut ExtensionManifest)) -> (ExtensionManifest, Vec<u8>) {
        let lib = fake_lib();
        let lib_hash = sha256_hex(&lib);
        let mut m = ExtensionManifest {
            extension_id: "example-ping".into(),
            version: "1.2.0".into(),
            abi_version: HOST_ABI,
            publisher_id: "aldivine".into(),
            library: "ping.dll".into(),
            requires_runtime: Some(">=0.1.0".into()),
            capabilities: vec!["timers".into(), "logging".into()],
            signature: ExtSignature::default(),
        };
        mutate(&mut m);
        let msg = canonical_extension_bytes(&m, &lib_hash);
        use ed25519_dalek::Signer;
        let sig = key.sign(&msg);
        let sig_hex = {
            let mut s = String::with_capacity(128);
            for b in sig.to_bytes() {
                s.push_str(&format!("{b:02x}"));
            }
            s
        };
        m.signature =
            ExtSignature { algorithm: ExtSignature::ALGO.into(), public_key_hex: key_hex(key), signature_hex: sig_hex };
        (m, lib)
    }

    fn trusting_policy(key: &SigningKey) -> ExtPolicy {
        ExtPolicy {
            allow_unsigned: false,
            trusted_keys: vec![key_hex(key)],
            allowed_capabilities: vec!["timers".into(), "logging".into()],
        }
    }

    fn authorize_ok(m: &ExtensionManifest, lib: &[u8], policy: &ExtPolicy) -> Authorized {
        authorize(m, lib, &sha256_hex(lib), policy).expect("authorize")
    }

    #[test]
    fn manifest_parses_from_toml() {
        let text = r#"
extension_id = "example-ping"
version = "1.2.0"
abi_version = 1
publisher_id = "aldivine"
library = "ping.dll"
requires_runtime = ">=0.1.0"
capabilities = ["timers", "logging"]
"#;
        let m = ExtensionManifest::parse_toml(text).unwrap();
        assert_eq!(m.extension_id, "example-ping");
        assert_eq!(m.capabilities.len(), 2);
        assert!(!m.signature.is_signed());
    }

    #[test]
    fn garbage_toml_rejected() {
        assert!(matches!(ExtensionManifest::parse_toml("[[["), Err(ExtError::Malformed(_))));
    }

    #[test]
    fn valid_signed_extension_authorizes() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |_| {});
        let auth = authorize_ok(&m, &lib, &trusting_policy(&key));
        assert_eq!(auth.capabilities, vec!["logging".to_string(), "timers".to_string()]);
        assert_eq!(auth.lib_sha256_hex, sha256_hex(&lib));
    }

    #[test]
    fn abi_mismatch_refused() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |m| m.abi_version = HOST_ABI + 1);
        let err = authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&key)).unwrap_err();
        assert!(matches!(err, ExtError::AbiMismatch { wanted, .. } if wanted == HOST_ABI + 1));
        assert!(!err.quarantines());
    }

    #[test]
    fn runtime_range_enforced() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |m| m.requires_runtime = Some(">=999.0.0".into()));
        let err = authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&key)).unwrap_err();
        assert!(matches!(err, ExtError::RuntimeMismatch { .. }));
    }

    #[test]
    fn bad_runtime_range_is_malformed() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |m| m.requires_runtime = Some("not a range !!!".into()));
        assert!(matches!(authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&key)), Err(ExtError::Malformed(_))));
    }

    #[test]
    fn non_semver_version_rejected() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |m| m.version = "v1".into());
        assert!(matches!(authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&key)), Err(ExtError::Malformed(_))));
    }

    #[test]
    fn traversal_library_names_rejected() {
        for bad in [
            "../evil.dll",
            "..\\evil.dll",
            "/abs/evil.so",
            "C:\\evil.dll",
            "C:evil.dll",
            "sub/dir.dll",
            "sub\\dir.dll",
            "",
            ".hidden.so",
            "evil.dll:stream",
        ] {
            let key = test_key();
            let (m, lib) = signed_manifest(&key, |m| m.library = bad.into());
            let err = authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&key)).unwrap_err();
            assert!(matches!(err, ExtError::UnsafeLibraryPath { .. }), "{bad}: {err}");
        }
    }

    #[test]
    fn unsigned_refused_by_default() {
        let mut m = ExtensionManifest {
            extension_id: "x".into(),
            version: "1.0.0".into(),
            abi_version: HOST_ABI,
            publisher_id: "p".into(),
            library: "x.dll".into(),
            requires_runtime: None,
            capabilities: vec![],
            signature: ExtSignature::default(),
        };
        let lib = fake_lib();
        let err = authorize(&m, &lib, &sha256_hex(&lib), &ExtPolicy::default()).unwrap_err();
        assert!(matches!(err, ExtError::Unsigned { .. }));
        assert!(err.quarantines());
        // Explicit opt-in loads unsigned (dev loop); still binds the hash.
        m.library = "x.dll".into();
        let policy = ExtPolicy { allow_unsigned: true, ..Default::default() };
        authorize(&m, &lib, &sha256_hex(&lib), &policy).expect("unsigned allowed by policy");
    }

    #[test]
    fn tampered_manifest_rejected() {
        let key = test_key();
        let (mut m, lib) = signed_manifest(&key, |_| {});
        m.version = "9.9.9".into();
        let err = authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&key)).unwrap_err();
        assert!(matches!(err, ExtError::BadSignature(_)));
        assert!(err.quarantines());
    }

    #[test]
    fn swapped_library_rejected_by_hash() {
        let key = test_key();
        let (m, _) = signed_manifest(&key, |_| {});
        let other = b"attacker-controlled-bytes".to_vec();
        let err = authorize(&m, &other, &sha256_hex(&fake_lib()), &trusting_policy(&key)).unwrap_err();
        assert!(matches!(err, ExtError::HashMismatch { .. }));
        assert!(err.quarantines());
    }

    #[test]
    fn wrong_key_rejected() {
        let key = test_key();
        let other = test_key();
        let (m, lib) = signed_manifest(&key, |_| {});
        // Signed by `key` but only `other` is trusted.
        let err = authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&other)).unwrap_err();
        assert!(matches!(err, ExtError::UntrustedKey(_)));
        assert!(err.quarantines());
    }

    #[test]
    fn unknown_capability_rejected() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |m| m.capabilities.push("godmode".into()));
        let mut policy = trusting_policy(&key);
        policy.allowed_capabilities.push("godmode".into());
        let err = authorize(&m, &lib, &sha256_hex(&lib), &policy).unwrap_err();
        assert!(matches!(err, ExtError::UnknownCapability { .. }));
        assert!(err.quarantines());
    }

    #[test]
    fn ungranted_capability_denied() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |m| m.capabilities.push("db.query".into()));
        // `db.query` is known but not in the allowed set.
        let err = authorize(&m, &lib, &sha256_hex(&lib), &trusting_policy(&key)).unwrap_err();
        assert!(matches!(err, ExtError::CapabilityDenied { ref capability, .. } if capability == "db.query"));
        assert!(err.quarantines());
    }

    #[test]
    fn default_policy_denies_everything() {
        let policy = ExtPolicy::default();
        assert!(!policy.allow_unsigned);
        assert!(policy.trusted_keys.is_empty());
        assert!(policy.allowed_capabilities.is_empty());
    }

    #[test]
    fn canonical_bytes_deterministic_and_order_free() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |_| {});
        let h = sha256_hex(&lib);
        let a = canonical_extension_bytes(&m, &h);
        let mut m2 = m.clone();
        m2.capabilities.reverse();
        assert_eq!(a, canonical_extension_bytes(&m2, &h));
    }

    // ------------------------------------------------------------------
    // Real loading: a tiny cdylib fixture exporting ald_extension_init.
    // ------------------------------------------------------------------

    fn ensure_fixture_built() -> std::path::PathBuf {
        let exe = std::env::current_exe().expect("current test binary");
        let debug = exe.parent().expect("deps dir").parent().expect("profile dir").to_path_buf();
        let lib_name = if cfg!(windows) {
            "ald_native_extension_testext.dll"
        } else if cfg!(target_os = "macos") {
            "libald_native_extension_testext.dylib"
        } else {
            "libald_native_extension_testext.so"
        };
        let path = debug.join(lib_name);
        if !path.exists() {
            // Single-crate `cargo test -p` does not build workspace siblings,
            // so build the fixture on demand. Offline-safe: zero dependencies.
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let status = std::process::Command::new(&cargo)
                .arg("build")
                .arg("--offline")
                .arg("-p")
                .arg("ald-native-extension-testext")
                .status()
                .expect("cargo must be runnable from tests");
            assert!(status.success(), "fixture build failed");
            assert!(path.exists(), "fixture still missing at {}", path.display());
        }
        path
    }

    fn fixture_authorized(lib_path: &std::path::Path) -> (Authorized, std::path::PathBuf) {
        let lib_bytes = std::fs::read(lib_path).expect("fixture bytes");
        let lib_hash = sha256_hex(&lib_bytes);
        let key = test_key();
        let file_name = lib_path.file_name().unwrap().to_string_lossy().into_owned();
        let mut m = ExtensionManifest {
            extension_id: "testext".into(),
            version: "0.1.0".into(),
            abi_version: HOST_ABI,
            publisher_id: "aldivine-tests".into(),
            library: file_name,
            requires_runtime: None,
            capabilities: vec!["logging".into()],
            signature: ExtSignature::default(),
        };
        let msg = canonical_extension_bytes(&m, &lib_hash);
        use ed25519_dalek::Signer;
        let sig = key.sign(&msg);
        let mut sig_hex = String::with_capacity(128);
        for b in sig.to_bytes() {
            sig_hex.push_str(&format!("{b:02x}"));
        }
        m.signature = ExtSignature {
            algorithm: ExtSignature::ALGO.into(),
            public_key_hex: key_hex(&key),
            signature_hex: sig_hex,
        };
        let policy = ExtPolicy {
            allow_unsigned: false,
            trusted_keys: vec![key_hex(&key)],
            allowed_capabilities: vec!["logging".into()],
        };
        let auth = authorize(&m, &lib_bytes, &lib_hash, &policy).expect("fixture authorizes");
        (auth, lib_path.parent().unwrap().to_path_buf())
    }

    #[test]
    fn real_extension_loads_and_init_returns_zero() {
        let path = ensure_fixture_built();
        let (auth, dir) = fixture_authorized(&path);
        let loaded = load(&auth, &dir).expect("fixture loads");
        assert_eq!(loaded.extension_id, "testext");
        assert_eq!(loaded.init_code, 0);
        assert!(loaded.has_symbol(INIT_SYMBOL));
        assert!(!loaded.has_symbol("ald_no_such_symbol"));
    }

    #[test]
    fn foreign_abi_init_fails_the_load() {
        let path = ensure_fixture_built();
        let (auth, dir) = fixture_authorized(&path);
        let err = load_with_abi(&auth, &dir, HOST_ABI + 100).unwrap_err();
        assert!(matches!(err, ExtError::InitFailed { code: 7, .. }), "{err}");
    }

    #[test]
    fn missing_library_file_is_operational_not_quarantine() {
        let key = test_key();
        let (m, lib) = signed_manifest(&key, |_| {});
        let auth = authorize_ok(&m, &lib, &trusting_policy(&key));
        let err = load(&auth, Path::new("nonexistent-dir")).unwrap_err();
        assert!(matches!(err, ExtError::LibraryLoad { .. }));
        assert!(!err.quarantines());
    }
}
