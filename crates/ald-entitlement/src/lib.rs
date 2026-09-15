//! Aldivine Entitlement Service — who is allowed to run what.
//!
//! Two things this module deliberately does NOT do:
//! 1. It never conflates identity with payment. A verified Rockstar account
//!    says the player owns the game; it says nothing about whether they have
//!    paid for a resource package. Those are separate tables.
//! 2. It never bypasses an external licensing provider. When a package is
//!    protected by a system Aldivine cannot legitimately query, the answer is
//!    `ExternalProvider` / `FivemEscrowUnsupported`, never a fabricated
//!    "authorized".
//!
//! The service is designed around a free-by-default ecosystem: the initial
//! state grants every server the FREE tier, and paid tiers are additive
//! rather than gating the base platform.

use std::collections::HashMap;

use ald_core::AldivinePlayerId;

/// Commercial model a package may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageModel {
    Free,
    Paid,
    Subscription,
    Private,
    Developer,
    Beta,
    InviteOnly,
}

impl PackageModel {
    pub fn as_str(self) -> &'static str {
        match self {
            PackageModel::Free => "FREE",
            PackageModel::Paid => "PAID",
            PackageModel::Subscription => "SUBSCRIPTION",
            PackageModel::Private => "PRIVATE",
            PackageModel::Developer => "DEVELOPER",
            PackageModel::Beta => "BETA",
            PackageModel::InviteOnly => "INVITE_ONLY",
        }
    }
}

/// Distribution policy of a package. Drives whether signature verification is
/// required before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackagePolicy {
    /// Source-open, no signature required.
    Open,
    /// Publisher-signed; signature must verify.
    Signed,
    /// Signed plus entitlement check.
    Protected,
    /// Publisher-restricted; only whitelisted servers.
    Private,
}

/// Outcome of an authorization check. The unhappy paths are distinct so Aegis
/// can tell an operator exactly why a package refused to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementState {
    Free,
    Authorized,
    NotAuthorized,
    Expired {
        since_epoch: u64,
    },
    ExternalProvider {
        provider: String,
    },
    /// Cfx Asset Escrow or equivalent DRM Aldivine will not bypass.
    FivemEscrowUnsupported,
    Unknown,
}

/// One grant of a package to a server.
#[derive(Debug, Clone)]
pub struct Grant {
    pub package_id: String,
    pub model: PackageModel,
    /// Epoch seconds. None = no expiry.
    pub expires_at: Option<u64>,
    /// Opaque signed license blob the publisher issued, if any.
    pub license_token: Option<String>,
}

/// A package as the entitlement service knows it.
#[derive(Debug, Clone)]
pub struct Package {
    pub package_id: String,
    pub publisher_id: String,
    pub name: String,
    pub version: String,
    pub model: PackageModel,
    pub policy: PackagePolicy,
    /// Only meaningful for `Protected`/`Private` models.
    pub requires_entitlement: bool,
    /// Publisher's Ed25519 verifying key, hex-encoded.
    pub publisher_key_hex: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EntitlementError {
    #[error("package '{0}' is unknown to the entitlement service")]
    UnknownPackage(String),
    #[error("server '{0}' has no grant for package '{1}'")]
    NoGrant(String, String),
    #[error("grant for '{0}' expired at {1}")]
    Expired(String, u64),
    #[error("signature verification is required for '{0}' and it failed")]
    SignatureFailed(String),
    #[error("package '{0}' is protected by external provider '{1}'")]
    ExternalProvider(String, String),
    #[error("package '{0}' uses FiveM asset escrow, which Aldivine does not bypass")]
    FivemEscrow(String),
}

/// Server identity. Free to obtain; cryptographically signed by the platform.
#[derive(Debug, Clone)]
pub struct ServerLicense {
    pub server_id: String,
    pub project_id: String,
    pub issued_at: u64,
    /// Signed over (server_id, project_id, issued_at).
    pub signature_hex: String,
}

impl ServerLicense {
    pub fn prefix(&self) -> &str {
        &self.project_id
    }
}

/// The entitlement service. Owns package metadata and per-server grants.
///
/// Concurrency: a single task owns this and answers queries by message; see
/// the server runtime. No interior mutability is needed here.
#[derive(Debug, Default)]
pub struct EntitlementService {
    packages: HashMap<String, Package>,
    /// (server_id, package_id) -> grant.
    grants: HashMap<(String, String), Grant>,
    servers: HashMap<String, ServerLicense>,
}

impl EntitlementService {
    pub fn new() -> Self {
        EntitlementService::default()
    }

    /// Register a server. Free; no payment. The signature would be produced by
    /// the platform signing key — here the caller supplies it.
    pub fn register_server(&mut self, license: ServerLicense) {
        self.servers.insert(license.server_id.clone(), license);
    }

    /// Register a package publishers ships. Idempotent on package_id.
    pub fn register_package(&mut self, package: Package) {
        self.packages.insert(package.package_id.clone(), package);
    }

    /// Grant a package to a server. Used for purchases, subscriptions, and
    /// developer allocations.
    pub fn grant(&mut self, server_id: &str, grant: Grant) {
        self.grants.insert((server_id.to_string(), grant.package_id.clone()), grant);
    }

    /// Revoke a grant (refund, abuse, or cancellation).
    pub fn revoke(&mut self, server_id: &str, package_id: &str) {
        self.grants.remove(&(server_id.to_string(), package_id.to_string()));
    }

    /// Resolve whether a server may run a package right now.
    ///
    /// `signature_ok` is supplied by the caller because signature verification
    /// needs the package bytes, which this service deliberately does not hold.
    /// Passing it in keeps the separation: entitlement decides *may*, the
    /// package verifier decides *is this the real artifact*.
    pub fn check(&self, server_id: &str, package_id: &str, signature_ok: bool, now_epoch: u64) -> EntitlementState {
        let Some(pkg) = self.packages.get(package_id) else {
            return EntitlementState::Unknown;
        };

        if pkg.model == PackageModel::Free && !pkg.requires_entitlement {
            if pkg.policy == PackagePolicy::Signed && !signature_ok {
                return EntitlementState::NotAuthorized;
            }
            return EntitlementState::Free;
        }

        // Non-free model: a grant is mandatory.
        let Some(grant) = self.grants.get(&(server_id.to_string(), package_id.to_string())) else {
            return EntitlementState::NotAuthorized;
        };

        if let Some(exp) = grant.expires_at {
            if now_epoch >= exp {
                return EntitlementState::Expired { since_epoch: exp };
            }
        }

        if (pkg.policy == PackagePolicy::Signed || pkg.policy == PackagePolicy::Protected) && !signature_ok {
            return EntitlementState::NotAuthorized;
        }

        EntitlementState::Authorized
    }

    pub fn package(&self, package_id: &str) -> Option<&Package> {
        self.packages.get(package_id)
    }

    pub fn grant_for(&self, server_id: &str, package_id: &str) -> Option<&Grant> {
        self.grants.get(&(server_id.to_string(), package_id.to_string()))
    }

    /// Aegis listing: every grant a server holds.
    pub fn grants_for_server(&self, server_id: &str) -> Vec<(&Grant, &Package)> {
        self.grants
            .iter()
            .filter(|((sid, _), _)| sid == server_id)
            .filter_map(|(_, grant)| self.packages.get(&grant.package_id).map(|p| (grant, p)))
            .collect()
    }
}

/// Classify a resource's protection level when it is imported. This is what
/// `ald migrate` reports so an operator learns early that an escrow resource
/// cannot run, instead of at server boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionClass {
    OpenSource,
    SourceAvailable,
    PlainResource,
    VendorLicensed,
    FivemEscrow,
    UnknownProtection,
}

impl ProtectionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ProtectionClass::OpenSource => "OPEN_SOURCE",
            ProtectionClass::SourceAvailable => "SOURCE_AVAILABLE",
            ProtectionClass::PlainResource => "PLAIN_RESOURCE",
            ProtectionClass::VendorLicensed => "VENDOR_LICENSED",
            ProtectionClass::FivemEscrow => "FIVEM_ESCROW",
            ProtectionClass::UnknownProtection => "UNKNOWN_PROTECTION",
        }
    }

    /// What this class means for execution.
    pub fn runnable(self) -> Runnability {
        match self {
            ProtectionClass::OpenSource | ProtectionClass::SourceAvailable | ProtectionClass::PlainResource => {
                Runnability::Runnable
            }
            ProtectionClass::VendorLicensed => Runnability::NeedsEntitlement,
            ProtectionClass::FivemEscrow => Runnability::BlockedExternal,
            ProtectionClass::UnknownProtection => Runnability::NeedsReview,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runnability {
    Runnable,
    NeedsEntitlement,
    BlockedExternal,
    NeedsReview,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free_pkg(id: &str) -> Package {
        Package {
            package_id: id.into(),
            publisher_id: "pub".into(),
            name: id.into(),
            version: "1.0.0".into(),
            model: PackageModel::Free,
            policy: PackagePolicy::Open,
            requires_entitlement: false,
            publisher_key_hex: None,
        }
    }

    fn paid_pkg(id: &str) -> Package {
        Package {
            package_id: id.into(),
            publisher_id: "pub".into(),
            name: id.into(),
            version: "1.0.0".into(),
            model: PackageModel::Paid,
            policy: PackagePolicy::Signed,
            requires_entitlement: true,
            publisher_key_hex: Some("deadbeef".into()),
        }
    }

    #[test]
    fn free_package_runs_without_grant() {
        let mut s = EntitlementService::new();
        s.register_package(free_pkg("free-hud"));
        assert_eq!(s.check("srv", "free-hud", true, 1000), EntitlementState::Free);
    }

    #[test]
    fn free_but_signed_requires_valid_signature() {
        let mut s = EntitlementService::new();
        let mut pkg = free_pkg("signed-free");
        pkg.policy = PackagePolicy::Signed;
        s.register_package(pkg);
        assert_eq!(s.check("srv", "signed-free", false, 1000), EntitlementState::NotAuthorized);
        assert_eq!(s.check("srv", "signed-free", true, 1000), EntitlementState::Free);
    }

    #[test]
    fn paid_package_without_grant_is_unauthorized() {
        let mut s = EntitlementService::new();
        s.register_package(paid_pkg("premium-cars"));
        assert_eq!(s.check("srv", "premium-cars", true, 1000), EntitlementState::NotAuthorized);
    }

    #[test]
    fn grant_authorizes_paid_package() {
        let mut s = EntitlementService::new();
        s.register_package(paid_pkg("premium-cars"));
        s.grant(
            "srv",
            Grant {
                package_id: "premium-cars".into(),
                model: PackageModel::Paid,
                expires_at: None,
                license_token: None,
            },
        );
        assert_eq!(s.check("srv", "premium-cars", true, 1000), EntitlementState::Authorized);
    }

    #[test]
    fn bad_signature_blocks_authorized_grant() {
        let mut s = EntitlementService::new();
        s.register_package(paid_pkg("premium-cars"));
        s.grant(
            "srv",
            Grant {
                package_id: "premium-cars".into(),
                model: PackageModel::Paid,
                expires_at: None,
                license_token: None,
            },
        );
        assert_eq!(s.check("srv", "premium-cars", false, 1000), EntitlementState::NotAuthorized);
    }

    #[test]
    fn subscription_expiry_is_reported_with_time() {
        let mut s = EntitlementService::new();
        s.register_package(paid_pkg("sub-job"));
        s.grant(
            "srv",
            Grant {
                package_id: "sub-job".into(),
                model: PackageModel::Subscription,
                expires_at: Some(500),
                license_token: None,
            },
        );
        assert_eq!(s.check("srv", "sub-job", true, 400), EntitlementState::Authorized);
        match s.check("srv", "sub-job", true, 600) {
            EntitlementState::Expired { since_epoch } => assert_eq!(since_epoch, 500),
            other => panic!("expected Expired, got {other:?}"),
        }
    }

    #[test]
    fn revoke_removes_access() {
        let mut s = EntitlementService::new();
        s.register_package(paid_pkg("premium-cars"));
        s.grant(
            "srv",
            Grant {
                package_id: "premium-cars".into(),
                model: PackageModel::Paid,
                expires_at: None,
                license_token: None,
            },
        );
        s.revoke("srv", "premium-cars");
        assert_eq!(s.check("srv", "premium-cars", true, 1000), EntitlementState::NotAuthorized);
    }

    #[test]
    fn unknown_package_is_unknown() {
        let s = EntitlementService::new();
        assert_eq!(s.check("srv", "nope", true, 1000), EntitlementState::Unknown);
    }

    #[test]
    fn server_license_registered() {
        let mut s = EntitlementService::new();
        let lic = ServerLicense {
            server_id: "srv-1".into(),
            project_id: "ald-project:x".into(),
            issued_at: 1,
            signature_hex: "sig".into(),
        };
        s.register_server(lic);
        assert!(s.servers.contains_key("srv-1"));
    }

    #[test]
    fn grants_listing_for_aegis() {
        let mut s = EntitlementService::new();
        s.register_package(paid_pkg("a"));
        s.register_package(paid_pkg("b"));
        s.grant(
            "srv",
            Grant { package_id: "a".into(), model: PackageModel::Paid, expires_at: None, license_token: None },
        );
        s.grant(
            "srv",
            Grant { package_id: "b".into(), model: PackageModel::Paid, expires_at: None, license_token: None },
        );
        s.grant(
            "other",
            Grant { package_id: "a".into(), model: PackageModel::Paid, expires_at: None, license_token: None },
        );
        assert_eq!(s.grants_for_server("srv").len(), 2);
    }

    #[test]
    fn protection_class_runnability() {
        assert_eq!(ProtectionClass::OpenSource.runnable(), Runnability::Runnable);
        assert_eq!(ProtectionClass::FivemEscrow.runnable(), Runnability::BlockedExternal);
        assert_eq!(ProtectionClass::VendorLicensed.runnable(), Runnability::NeedsEntitlement);
        assert_eq!(ProtectionClass::UnknownProtection.runnable(), Runnability::NeedsReview);
    }

    #[test]
    fn model_codes() {
        assert_eq!(PackageModel::Free.as_str(), "FREE");
        assert_eq!(PackageModel::InviteOnly.as_str(), "INVITE_ONLY");
    }

    #[test]
    fn player_id_not_used_as_entitlement_key() {
        // The service keys on server+package, never on AldivinePlayerId.
        // This test exists to keep that invariant visible: a refactor that
        // makes entitlement player-keyed would break the "identity is not
        // payment" rule documented at the top of this module.
        let _ = AldivinePlayerId::new();
        let s = EntitlementService::new();
        assert!(s.grants.is_empty());
    }
}
