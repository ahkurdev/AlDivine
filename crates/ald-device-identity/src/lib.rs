//! Privacy-preserving device fingerprint.
//! Raw hardware serials MUST NOT leave this crate or reach scripts/admin UI.
//! We build a keyed HMAC fingerprint over normalized signals so the same
//! physical device yields a stable id, but no raw serial is derivable and the
//! value is bound to a server-provided key.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Normalized hardware signals gathered locally. Never serialized raw.
#[derive(Debug, Clone, Default)]
pub struct DeviceSignals {
    pub cpu_vendor_model: String,
    pub total_memory_gb: u32,
    pub gpu_model: String,
    pub disk_model_family: String,
    pub os_build: String,
    pub machine_id_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::High => "HIGH",
            Confidence::Medium => "MEDIUM",
            Confidence::Low => "LOW",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId {
    pub version: u8,
    pub fingerprint: String,
}

impl DeviceId {
    pub fn encoded(&self) -> String {
        format!("device:v{}:{}", self.version, self.fingerprint)
    }
}

/// Derive a keyed fingerprint. `server_key` is provided per-deployment and must
/// never be shared with clients in a way that lets them forge ids.
pub fn derive_device_id(signals: &DeviceSignals, server_key: &[u8], version: u8) -> DeviceId {
    let mut mac = HmacSha256::new_from_slice(server_key).expect("HMAC accepts any key length");
    // Normalize: lowercase, trim, sort-free stable order.
    let norm = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        signals.cpu_vendor_model.to_lowercase().trim(),
        signals.total_memory_gb,
        signals.gpu_model.to_lowercase().trim(),
        signals.disk_model_family.to_lowercase().trim(),
        signals.os_build.to_lowercase().trim(),
        signals.machine_id_hash.trim()
    );
    mac.update(norm.as_bytes());
    let result = mac.finalize().into_bytes();
    let fingerprint = URL_SAFE_NO_PAD.encode(result);
    DeviceId { version, fingerprint }
}

/// Generate a random per-device local secret used as `machine_id_hash` seed.
/// Stored locally only; never transmitted.
pub fn generate_local_secret() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Match two device ids under allowed hardware drift tolerance.
/// Same fingerprint => HIGH. Different but same version with tolerant signals
/// would be MEDIUM; here we only have fingerprints, so differ => LOW/None.
/// Callers combine with other signals for final confidence.
pub fn match_confidence(a: &DeviceId, b: &DeviceId, same_version: bool) -> Option<Confidence> {
    if a.fingerprint == b.fingerprint {
        Some(Confidence::High)
    } else if same_version && a.version == b.version {
        Some(Confidence::Low)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DeviceSignals {
        DeviceSignals {
            cpu_vendor_model: "AuthenticAMD Ryzen 9".into(),
            total_memory_gb: 32,
            gpu_model: "NVIDIA RTX 4070".into(),
            disk_model_family: "Samsung NVMe".into(),
            os_build: "Windows 10.0.19045".into(),
            machine_id_hash: "localsecret".into(),
        }
    }

    #[test]
    fn stable_fingerprint() {
        let key = b"server-secret-key";
        let a = derive_device_id(&sample(), key, 1);
        let b = derive_device_id(&sample(), key, 1);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.encoded(), format!("device:v1:{}", a.fingerprint));
    }

    #[test]
    fn different_key_different_id() {
        let a = derive_device_id(&sample(), b"key-a", 1);
        let b = derive_device_id(&sample(), b"key-b", 1);
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn normalized_case_insensitive() {
        let key = b"k";
        let mut s = sample();
        s.cpu_vendor_model = "  authenticamd RYZEN 9 ".into();
        let a = derive_device_id(&s, key, 1);
        let b = derive_device_id(&sample(), key, 1);
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn match_levels() {
        let key = b"k";
        let a = derive_device_id(&sample(), key, 1);
        let b = derive_device_id(&sample(), key, 1);
        let c = derive_device_id(&sample(), b"other", 1);
        assert_eq!(match_confidence(&a, &b, true), Some(Confidence::High));
        assert_eq!(match_confidence(&a, &c, true), Some(Confidence::Low));
    }
}
