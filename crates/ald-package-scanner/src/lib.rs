//! Aldivine Registry Package Scanner
//!
//! Static analysis, capability auditing, native-extension detection,
//! obfuscation heuristics, and reputation-based quarantine decision engine.

use ald_package::PackageManifest;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// Severity of a detected finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info = 0,
    Low = 1,
    Medium = 2,
    High = 3,
    Critical = 4,
}

/// Category of security scan finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FindingCategory {
    NativeExtension,
    SandboxViolation,
    Obfuscation,
    ExcessiveCapability,
    RevokedEntity,
    MetadataAnomaly,
}

/// A specific finding identified during package analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub category: FindingCategory,
    pub severity: Severity,
    pub message: String,
    pub file_path: Option<String>,
}

/// Publisher reputation level within the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PublisherReputation {
    Blacklisted = 0,
    Suspicious = 1,
    Standard = 2,
    Verified = 3,
    Trusted = 4,
}

/// Disposition verdict for an analyzed package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanVerdict {
    Approved,
    FlaggedForReview,
    Quarantined,
    Rejected,
}

/// Complete report returned by the package scanner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    pub package_id: String,
    pub publisher_id: String,
    pub risk_score: u32,
    pub findings: Vec<Finding>,
    pub verdict: ScanVerdict,
}

#[derive(Debug, Error)]
pub enum ScannerError {
    #[error("manifest invalid: {0}")]
    InvalidManifest(String),
}

/// In-memory reputation and revocation authority database.
#[derive(Debug, Clone, Default)]
pub struct ReputationAuthority {
    publishers: HashMap<String, PublisherReputation>,
    revoked_packages: HashSet<String>,
    revoked_keys: HashSet<String>,
}

impl ReputationAuthority {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_publisher_reputation(&mut self, publisher_id: impl Into<String>, rep: PublisherReputation) {
        self.publishers.insert(publisher_id.into(), rep);
    }

    pub fn get_publisher_reputation(&self, publisher_id: &str) -> PublisherReputation {
        self.publishers.get(publisher_id).copied().unwrap_or(PublisherReputation::Standard)
    }

    pub fn revoke_package(&mut self, package_id: impl Into<String>) {
        self.revoked_packages.insert(package_id.into());
    }

    pub fn is_package_revoked(&self, package_id: &str) -> bool {
        self.revoked_packages.contains(package_id)
    }

    pub fn revoke_key(&mut self, key_hex: impl Into<String>) {
        self.revoked_keys.insert(key_hex.into());
    }

    pub fn is_key_revoked(&self, key_hex: &str) -> bool {
        self.revoked_keys.contains(key_hex)
    }
}

/// Package file payload input for scanning.
pub struct PackageFileEntry<'a> {
    pub path: &'a str,
    pub content: &'a [u8],
}

/// Configuration for the package scanner rules.
#[derive(Debug, Clone)]
pub struct ScannerConfig {
    pub max_risk_score_for_approval: u32,
    pub flag_threshold: u32,
    pub quarantine_threshold: u32,
    pub allow_native_extensions: bool,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            max_risk_score_for_approval: 20,
            flag_threshold: 40,
            quarantine_threshold: 75,
            allow_native_extensions: false,
        }
    }
}

/// Static analyzer and rule checker.
pub struct PackageScanner {
    config: ScannerConfig,
    authority: ReputationAuthority,
}

impl PackageScanner {
    pub fn new(config: ScannerConfig, authority: ReputationAuthority) -> Self {
        Self { config, authority }
    }

    /// Scan a package given its manifest and a list of file entries.
    pub fn scan(&self, manifest: &PackageManifest, files: &[PackageFileEntry<'_>]) -> ScanReport {
        let mut findings = Vec::new();

        // 1. Reputation & revocation checks
        if self.authority.is_package_revoked(&manifest.package_id) {
            findings.push(Finding {
                category: FindingCategory::RevokedEntity,
                severity: Severity::Critical,
                message: format!("Package ID '{}' is on the revocation list", manifest.package_id),
                file_path: None,
            });
        }

        let pub_rep = self.authority.get_publisher_reputation(&manifest.publisher_id);
        if pub_rep == PublisherReputation::Blacklisted {
            findings.push(Finding {
                category: FindingCategory::RevokedEntity,
                severity: Severity::Critical,
                message: format!("Publisher '{}' is blacklisted", manifest.publisher_id),
                file_path: None,
            });
        } else if pub_rep == PublisherReputation::Suspicious {
            findings.push(Finding {
                category: FindingCategory::RevokedEntity,
                severity: Severity::High,
                message: format!("Publisher '{}' has suspicious reputation", manifest.publisher_id),
                file_path: None,
            });
        }

        if self.authority.is_key_revoked(&manifest.signature.public_key_hex) {
            findings.push(Finding {
                category: FindingCategory::RevokedEntity,
                severity: Severity::Critical,
                message: format!("Signature key '{}' is revoked", manifest.signature.public_key_hex),
                file_path: None,
            });
        }

        // 2. Metadata consistency
        if manifest.package_id.trim().is_empty() {
            findings.push(Finding {
                category: FindingCategory::MetadataAnomaly,
                severity: Severity::High,
                message: "Empty package ID".to_string(),
                file_path: None,
            });
        }

        // 3. Scan files
        for file in files {
            self.scan_file_path(file.path, &mut findings);
            self.scan_file_content(file.path, file.content, &mut findings);
        }

        // Calculate risk score (0 to 100 max)
        let mut risk_score: u32 = 0;
        for f in &findings {
            let pts = match f.severity {
                Severity::Info => 0,
                Severity::Low => 5,
                Severity::Medium => 15,
                Severity::High => 35,
                Severity::Critical => 80,
            };
            risk_score = risk_score.saturating_add(pts);
        }
        if risk_score > 100 {
            risk_score = 100;
        }

        // Apply publisher discount for Trusted/Verified if no Critical findings
        let has_critical = findings.iter().any(|f| f.severity == Severity::Critical);
        if !has_critical {
            if pub_rep == PublisherReputation::Trusted {
                risk_score = risk_score.saturating_sub(15);
            } else if pub_rep == PublisherReputation::Verified {
                risk_score = risk_score.saturating_sub(5);
            }
        }

        // Determine verdict
        let verdict = if has_critical || risk_score >= self.config.quarantine_threshold {
            if pub_rep == PublisherReputation::Blacklisted || self.authority.is_package_revoked(&manifest.package_id) {
                ScanVerdict::Rejected
            } else {
                ScanVerdict::Quarantined
            }
        } else if risk_score >= self.config.flag_threshold {
            ScanVerdict::FlaggedForReview
        } else if risk_score <= self.config.max_risk_score_for_approval {
            ScanVerdict::Approved
        } else {
            ScanVerdict::FlaggedForReview
        };

        ScanReport {
            package_id: manifest.package_id.clone(),
            publisher_id: manifest.publisher_id.clone(),
            risk_score,
            findings,
            verdict,
        }
    }

    fn scan_file_path(&self, path: &str, findings: &mut Vec<Finding>) {
        let p_lower = path.to_ascii_lowercase();

        // Native binaries
        let native_extensions = [".dll", ".so", ".dylib", ".exe", ".bin", ".elf"];
        for ext in native_extensions {
            if p_lower.ends_with(ext) {
                let sev = if self.config.allow_native_extensions { Severity::Low } else { Severity::Critical };
                findings.push(Finding {
                    category: FindingCategory::NativeExtension,
                    severity: sev,
                    message: format!("Found native binary file '{}'", path),
                    file_path: Some(path.to_string()),
                });
                break;
            }
        }
    }

    fn scan_file_content(&self, path: &str, content: &[u8], findings: &mut Vec<Finding>) {
        // Only inspect text / script formats for patterns
        let is_script = path.ends_with(".lua")
            || path.ends_with(".js")
            || path.ends_with(".ts")
            || path.ends_with(".toml")
            || path.ends_with(".json");

        if !is_script {
            return;
        }

        let Ok(text) = std::str::from_utf8(content) else {
            // Non-UTF8 in script file is suspicious
            findings.push(Finding {
                category: FindingCategory::Obfuscation,
                severity: Severity::Medium,
                message: "Non-UTF-8 bytes inside script source file".to_string(),
                file_path: Some(path.to_string()),
            });
            return;
        };

        // Forbidden breakout patterns
        let dangerous_tokens = [
            ("os.execute", "Direct OS shell execution call", Severity::High),
            ("io.popen", "Process spawn via popen", Severity::High),
            ("package.loadlib", "Dynamic C library loader", Severity::High),
            ("child_process", "Node child_process execution", Severity::High),
            ("setfenv", "Lua environment tampering attempt", Severity::Medium),
            ("loadstring(", "Dynamic arbitrary string evaluation", Severity::Medium),
        ];

        for (token, msg, sev) in dangerous_tokens {
            let count = text.matches(token).count();
            for _ in 0..count.min(5) {
                findings.push(Finding {
                    category: FindingCategory::SandboxViolation,
                    severity: sev,
                    message: msg.to_string(),
                    file_path: Some(path.to_string()),
                });
            }
        }

        // Obfuscation heuristics: continuous long hex/base64 tokens without whitespace
        for word in text.split_whitespace() {
            let trimmed =
                word.trim_matches(|c: char| matches!(c, '\'' | '"' | '`' | '(' | ')' | ';' | ',' | '[' | ']'));
            if trimmed.len() > 300 {
                // Check if alphanumeric / base64 chars
                let is_b64 = trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
                if is_b64 {
                    findings.push(Finding {
                        category: FindingCategory::Obfuscation,
                        severity: Severity::Medium,
                        message: format!("Suspicious high-length encoded string literal (len: {})", trimmed.len()),
                        file_path: Some(path.to_string()),
                    });
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_package::{PackageManifest, ProtectionPolicy, Signature};

    fn make_test_manifest(id: &str, pub_id: &str) -> PackageManifest {
        PackageManifest {
            package_id: id.to_string(),
            publisher_id: pub_id.to_string(),
            name: id.to_string(),
            version: "1.0.0".to_string(),
            minimum_runtime: "0.1.0".to_string(),
            framework: None,
            dependencies: vec![],
            file_hashes: vec![],
            total_size: 1024,
            protection: ProtectionPolicy::Open,
            entitlement_model: "open".to_string(),
            signature: Signature {
                algorithm: "ed25519".to_string(),
                public_key_hex: "00".repeat(32),
                signature_hex: "00".repeat(64),
            },
        }
    }

    #[test]
    fn clean_package_is_approved() {
        let scanner = PackageScanner::new(ScannerConfig::default(), ReputationAuthority::new());
        let manifest = make_test_manifest("clean-pack", "good-dev");
        let files = [
            PackageFileEntry { path: "server/init.lua", content: b"print('clean server code')" },
            PackageFileEntry { path: "client/init.lua", content: b"print('clean client code')" },
        ];

        let report = scanner.scan(&manifest, &files);
        assert_eq!(report.verdict, ScanVerdict::Approved);
        assert_eq!(report.risk_score, 0);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn blacklisted_publisher_is_rejected() {
        let mut auth = ReputationAuthority::new();
        auth.set_publisher_reputation("bad-actor", PublisherReputation::Blacklisted);

        let scanner = PackageScanner::new(ScannerConfig::default(), auth);
        let manifest = make_test_manifest("pack", "bad-actor");

        let report = scanner.scan(&manifest, &[]);
        assert_eq!(report.verdict, ScanVerdict::Rejected);
        assert!(report.risk_score >= 80);
    }

    #[test]
    fn revoked_package_is_rejected() {
        let mut auth = ReputationAuthority::new();
        auth.revoke_package("compromised-v1");

        let scanner = PackageScanner::new(ScannerConfig::default(), auth);
        let manifest = make_test_manifest("compromised-v1", "normal-dev");

        let report = scanner.scan(&manifest, &[]);
        assert_eq!(report.verdict, ScanVerdict::Rejected);
    }

    #[test]
    fn native_binary_detected_and_quarantined() {
        let scanner = PackageScanner::new(ScannerConfig::default(), ReputationAuthority::new());
        let manifest = make_test_manifest("native-pack", "normal-dev");
        let files = [
            PackageFileEntry { path: "lib/payload.dll", content: b"MZ\x90\x00" },
            PackageFileEntry { path: "server/init.lua", content: b"os.execute('whoami')" },
        ];

        let report = scanner.scan(&manifest, &files);
        assert_eq!(report.verdict, ScanVerdict::Quarantined);
        assert!(report.findings.iter().any(|f| f.category == FindingCategory::NativeExtension));
        assert!(report.findings.iter().any(|f| f.category == FindingCategory::SandboxViolation));
    }

    #[test]
    fn suspicious_long_encoded_string_flagged() {
        let scanner = PackageScanner::new(ScannerConfig::default(), ReputationAuthority::new());
        let manifest = make_test_manifest("obfuscated-pack", "normal-dev");
        let mut long_b64 = String::from("local encoded = '");
        for _ in 0..400 {
            long_b64.push('A');
        }
        long_b64.push('\'');

        let files = [PackageFileEntry { path: "server/init.lua", content: long_b64.as_bytes() }];

        let report = scanner.scan(&manifest, &files);
        assert!(report.findings.iter().any(|f| f.category == FindingCategory::Obfuscation));
    }

    #[test]
    fn flagged_for_review_on_medium_findings() {
        let scanner = PackageScanner::new(ScannerConfig::default(), ReputationAuthority::new());
        let manifest = make_test_manifest("review-pack", "normal-dev");
        let files = [PackageFileEntry {
            path: "server/init.lua",
            content: b"setfenv(1, {}); local f = loadstring('x=1'); setfenv(2, {})",
        }];

        let report = scanner.scan(&manifest, &files);
        // setfenv + loadstring + setfenv = 15 + 15 + 15 = 45 >= 40 (flag_threshold)
        assert_eq!(report.verdict, ScanVerdict::FlaggedForReview);
        assert!(report.risk_score >= 40 && report.risk_score < 75);
    }

    #[test]
    fn trusted_publisher_receives_score_discount() {
        let mut auth = ReputationAuthority::new();
        auth.set_publisher_reputation("trusted-dev", PublisherReputation::Trusted);

        let scanner = PackageScanner::new(ScannerConfig::default(), auth);
        let manifest = make_test_manifest("pack", "trusted-dev");
        let files = [PackageFileEntry {
            path: "server/init.lua",
            content: b"setfenv(1, {})", // 15 points
        }];

        let report = scanner.scan(&manifest, &files);
        // 15 - 15 (discount) = 0
        assert_eq!(report.risk_score, 0);
        assert_eq!(report.verdict, ScanVerdict::Approved);
    }

    #[test]
    fn revoked_signing_key_triggers_critical() {
        let mut auth = ReputationAuthority::new();
        let bad_key = "11".repeat(32);
        auth.revoke_key(&bad_key);

        let scanner = PackageScanner::new(ScannerConfig::default(), auth);
        let mut manifest = make_test_manifest("pack", "normal-dev");
        manifest.signature.public_key_hex = bad_key;

        let report = scanner.scan(&manifest, &[]);
        assert_eq!(report.verdict, ScanVerdict::Quarantined);
        assert!(report
            .findings
            .iter()
            .any(|f| f.category == FindingCategory::RevokedEntity && f.severity == Severity::Critical));
    }

    #[test]
    fn empty_package_id_triggers_metadata_anomaly() {
        let scanner = PackageScanner::new(ScannerConfig::default(), ReputationAuthority::new());
        let manifest = make_test_manifest("   ", "normal-dev");

        let report = scanner.scan(&manifest, &[]);
        assert!(report.findings.iter().any(|f| f.category == FindingCategory::MetadataAnomaly));
    }

    #[test]
    fn allow_native_extensions_config_lowers_severity() {
        let config = ScannerConfig { allow_native_extensions: true, ..Default::default() };
        let scanner = PackageScanner::new(config, ReputationAuthority::new());
        let manifest = make_test_manifest("native-pack", "normal-dev");
        let files = [PackageFileEntry { path: "lib/ext.dll", content: b"MZ\x90\x00" }];

        let report = scanner.scan(&manifest, &files);
        let ext_finding = report.findings.iter().find(|f| f.category == FindingCategory::NativeExtension).unwrap();
        assert_eq!(ext_finding.severity, Severity::Low);
        assert_eq!(report.verdict, ScanVerdict::Approved);
    }
}
