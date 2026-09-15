//! Login pipeline — the connection gate every client passes through.
//!
//! The pipeline is deliberately split into ordered stages so that a failure
//! can be attributed to exactly one policy, and so the client gets a stable,
//! safe error code instead of internal details.
//!
//! Order matters and is load-bearing:
//! 1. game installation / distribution detection
//! 2. entitlement resolution (ownership)
//! 3. platform identity resolution
//! 4. device identity generation
//! 5. AstraNet connection (server observes remote IP — never trusts client IP)
//! 6. session validation
//! 7. server policy check (identity requirements)
//! 8. ban / permission checks
//!
//! Rules this module enforces:
//! - A legitimate Epic owner is never blocked solely for lacking Steam unless
//!   the server explicitly requires Steam (`require_steam`).
//! - Device ID generation is privacy-preserving; see ald-device-identity.
//! - Never trust a client-reported IP; the authoritative remote IP comes from
//!   the socket peer address.

use crate::PlayerIdentity;
use serde::{Deserialize, Serialize};

/// A failure in exactly one pipeline stage, with a client-safe code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRejection {
    /// Stable error code, e.g. `ALD-STEAM-001`. Shown to the client.
    pub code: String,
    /// Short human-readable reason, safe to send to the client.
    pub reason: String,
    /// Detailed internal context for Aegis. Never sent to the client.
    pub internal_detail: String,
}

impl ConnectionRejection {
    pub fn new(code: &str, reason: &str, internal_detail: impl Into<String>) -> Self {
        ConnectionRejection {
            code: code.to_string(),
            reason: reason.to_string(),
            internal_detail: internal_detail.into(),
        }
    }
}

/// Server-side identity requirements. Validated from `server.toml` `[identity]`.
///
/// Defaults follow the spec's example: Aldivine account and GTA entitlement
/// required, Rockstar/Steam/Epic optional, device id required, connection IP
/// recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRequirements {
    #[serde(default = "default_true")]
    pub require_aldivine_account: bool,
    #[serde(default = "default_true")]
    pub require_gta_entitlement: bool,
    #[serde(default)]
    pub require_rockstar: bool,
    #[serde(default)]
    pub require_steam: bool,
    #[serde(default)]
    pub require_epic: bool,
    #[serde(default = "default_true")]
    pub require_device_id: bool,
    #[serde(default = "default_true")]
    pub record_connection_ip: bool,
}

fn default_true() -> bool {
    true
}

impl Default for IdentityRequirements {
    fn default() -> Self {
        IdentityRequirements {
            require_aldivine_account: true,
            require_gta_entitlement: true,
            require_rockstar: false,
            require_steam: false,
            require_epic: false,
            require_device_id: true,
            record_connection_ip: true,
        }
    }
}

/// Outcome of running the policy stage against an aggregated identity.
pub enum PolicyOutcome {
    /// All requirements satisfied; the player may proceed to ban checks.
    Pass,
    /// One requirement failed. Contains the client-safe rejection.
    Reject(ConnectionRejection),
}

/// Validate an aggregated identity against server policy.
///
/// This is the policy stage only (stage 7). Entitlement resolution and
/// platform identity resolution happen earlier in the launcher/client and
/// their results are carried in `identity`.
pub fn check_identity_policy(identity: &PlayerIdentity, req: &IdentityRequirements) -> PolicyOutcome {
    // Aldivine account presence. The id is generated locally, so "missing"
    // here means the earlier handshake did not complete.
    if req.require_aldivine_account && identity.aldivine_id.to_string().is_empty() {
        return PolicyOutcome::Reject(ConnectionRejection::new(
            "ALD-IDENTITY-001",
            "Unable to validate Aldivine session.",
            "aldivine_player_id missing at policy stage",
        ));
    }

    // GTA entitlement — the key anti-piracy gate. Never mark verified
    // without a real provider result.
    if req.require_gta_entitlement {
        match identity.rockstar.entitlement {
            crate::Entitlement::Verified => {}
            crate::Entitlement::NotOwned => {
                return PolicyOutcome::Reject(ConnectionRejection::new(
                    "ALD-GAME-002",
                    "GTA V entitlement verification failed.",
                    "rockstar entitlement NOT_OWNED",
                ))
            }
            crate::Entitlement::NotLoggedIn => {
                return PolicyOutcome::Reject(ConnectionRejection::new(
                    "ALD-GAME-002",
                    "GTA V entitlement verification failed.",
                    "rockstar entitlement NOT_LOGGED_IN",
                ))
            }
            // Unavailable/Unknown/Error: the provider could not answer. Do NOT
            // treat this as a pass — that would fake verification by absence.
            other => {
                return PolicyOutcome::Reject(ConnectionRejection::new(
                    "ALD-GAME-002",
                    "GTA V entitlement verification failed.",
                    format!("rockstar entitlement {}", other.as_code()),
                ))
            }
        }
    }

    if req.require_rockstar && (!identity.rockstar.verified || identity.rockstar.account_id.is_none()) {
        return PolicyOutcome::Reject(ConnectionRejection::new(
            "ALD-IDENTITY-001",
            "Unable to validate Aldivine session.",
            "rockstar identity required but unverified",
        ));
    }

    // Steam requirement is explicit: an Epic owner is not blocked unless the
    // server opted into requiring Steam.
    if req.require_steam && identity.steam.is_none() {
        return PolicyOutcome::Reject(ConnectionRejection::new(
            "ALD-STEAM-001",
            "This server requires Steam identity.",
            "steam identity required but absent",
        ));
    }

    if req.require_epic && (identity.epic.account_id.is_none() || !identity.epic.verified) {
        return PolicyOutcome::Reject(ConnectionRejection::new(
            "ALD-IDENTITY-001",
            "Unable to validate Aldivine session.",
            "epic identity required but unverified",
        ));
    }

    if req.require_device_id && identity.device.is_none() {
        return PolicyOutcome::Reject(ConnectionRejection::new(
            "ALD-DEVICE-001",
            "Device identity could not be generated.",
            "device identity required but absent",
        ));
    }

    PolicyOutcome::Pass
}

/// Authoritative remote IP. `peer_addr` comes from the accepted socket, never
/// from a client-supplied field.
pub fn authoritative_remote_ip(peer_addr: &str) -> String {
    peer_addr.to_string()
}

/// A trusted-proxy decision for forwarded headers.
///
/// Servers behind a reverse proxy or DDoS scrubber may receive the real client
/// IP in `X-Forwarded-For` / `Forwarded` / `X-Real-IP`. Blindly trusting those
/// headers allows trivial IP spoofing, so only headers from an explicitly
/// configured trusted proxy are accepted.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    /// Allowed peer addresses of trusted proxies, as-normalized strings.
    pub peers: Vec<String>,
}

impl TrustedProxies {
    pub fn new(peers: Vec<String>) -> Self {
        TrustedProxies { peers }
    }

    /// Resolve the client IP. Returns the forwarded value only if `peer_addr`
    /// is itself a trusted proxy; otherwise the peer address is authoritative.
    ///
    /// `first_forwarded` is the first (leftmost, original client) value of the
    /// forwarded header chain, already extracted and trimmed by the caller.
    pub fn resolve_client_ip(&self, peer_addr: &str, forwarded: Option<&str>) -> String {
        let trusted = self.peers.iter().any(|p| p == peer_addr);
        match (trusted, forwarded) {
            (true, Some(f)) if !f.trim().is_empty() => f.trim().to_string(),
            _ => peer_addr.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Entitlement;
    use ald_core::AldivinePlayerId;

    fn identity_with_entitlement(e: Entitlement) -> PlayerIdentity {
        let mut p = PlayerIdentity::new(AldivinePlayerId::new());
        p.rockstar.entitlement = e;
        p.rockstar.verified = e == Entitlement::Verified;
        p.rockstar.account_id = if e == Entitlement::Verified { Some("rs-1".into()) } else { None };
        // Device id is required by default; attach a stand-in so the policy
        // stage under test is entitlement/identity, not device absence.
        p.device = Some(crate::aggregation::DeviceIdentity {
            device_id: "device:v1:test".into(),
            confidence: "HIGH".into(),
            version: 1,
            first_seen: 0,
            last_seen: 0,
        });
        p
    }

    #[test]
    fn verified_entitlement_passes() {
        let id = identity_with_entitlement(Entitlement::Verified);
        match check_identity_policy(&id, &IdentityRequirements::default()) {
            PolicyOutcome::Pass => {}
            _ => panic!("expected pass"),
        }
    }

    #[test]
    fn unverified_entitlement_rejected() {
        for e in [
            Entitlement::NotOwned,
            Entitlement::NotLoggedIn,
            Entitlement::Unavailable,
            Entitlement::Unknown,
            Entitlement::Error,
        ] {
            let id = identity_with_entitlement(e);
            match check_identity_policy(&id, &IdentityRequirements::default()) {
                PolicyOutcome::Reject(r) => assert_eq!(r.code, "ALD-GAME-002"),
                _ => panic!("expected rejection for {:?}", e),
            }
        }
    }

    #[test]
    fn epic_owner_without_steam_is_allowed_by_default() {
        // The explicit anti-NAT-discrimination rule.
        let mut id = identity_with_entitlement(Entitlement::Verified);
        id.epic.account_id = Some("epic-1".into());
        id.epic.verified = true;
        assert!(matches!(check_identity_policy(&id, &IdentityRequirements::default()), PolicyOutcome::Pass));
    }

    #[test]
    fn steam_required_rejects_epic_only_owner() {
        let mut id = identity_with_entitlement(Entitlement::Verified);
        id.epic.account_id = Some("epic-1".into());
        id.epic.verified = true;
        let req = IdentityRequirements { require_steam: true, ..IdentityRequirements::default() };
        match check_identity_policy(&id, &req) {
            PolicyOutcome::Reject(r) => assert_eq!(r.code, "ALD-STEAM-001"),
            _ => panic!("expected ALD-STEAM-001"),
        }
    }

    #[test]
    fn steam_owner_passes_steam_required() {
        let mut id = identity_with_entitlement(Entitlement::Verified);
        id.attach_steam(crate::SteamIdentity {
            steam_id64: "76561198000000000".into(),
            steam_hex: "steam:1100001025e4c00".into(),
            verified: true,
        });
        let req = IdentityRequirements { require_steam: true, ..IdentityRequirements::default() };
        assert!(matches!(check_identity_policy(&id, &req), PolicyOutcome::Pass));
    }

    #[test]
    fn missing_device_id_rejected_when_required() {
        // Start from empty: no entitlement, no device.
        let mut id = PlayerIdentity::new(AldivinePlayerId::new());
        id.rockstar.entitlement = Entitlement::Verified;
        id.rockstar.verified = true;
        id.rockstar.account_id = Some("rs-1".into());
        id.device = None;
        assert!(matches!(
            check_identity_policy(&id, &IdentityRequirements::default()),
            PolicyOutcome::Reject(r) if r.code == "ALD-DEVICE-001"
        ));
    }

    #[test]
    fn unknown_entitlement_does_not_sneak_through() {
        // Regression guard: Unknown must not be treated as "probably fine".
        let id = identity_with_entitlement(Entitlement::Unknown);
        assert!(matches!(
            check_identity_policy(&id, &IdentityRequirements::default()),
            PolicyOutcome::Reject(r) if r.code == "ALD-GAME-002"
        ));
    }

    #[test]
    fn trusted_proxy_resolves_forwarded_ip() {
        let tp = TrustedProxies::new(vec!["10.0.0.2".into()]);
        assert_eq!(tp.resolve_client_ip("10.0.0.2", Some("203.0.113.9")), "203.0.113.9");
    }

    #[test]
    fn untrusted_proxy_ignores_forwarded_header() {
        let tp = TrustedProxies::new(vec!["10.0.0.2".into()]);
        // Attacker sends X-Forwarded-For from an untrusted peer.
        assert_eq!(tp.resolve_client_ip("198.51.100.7", Some("203.0.113.9")), "198.51.100.7");
    }

    #[test]
    fn no_trusted_proxies_uses_peer_addr() {
        let tp = TrustedProxies::default();
        assert_eq!(tp.resolve_client_ip("198.51.100.7", Some("203.0.113.9")), "198.51.100.7");
    }

    #[test]
    fn empty_forwarded_falls_back_to_peer() {
        let tp = TrustedProxies::new(vec!["10.0.0.2".into()]);
        assert_eq!(tp.resolve_client_ip("10.0.0.2", Some("")), "10.0.0.2");
    }

    #[test]
    fn requirements_default_matches_spec() {
        let r = IdentityRequirements::default();
        assert!(r.require_aldivine_account);
        assert!(r.require_gta_entitlement);
        assert!(!r.require_rockstar);
        assert!(!r.require_steam);
        assert!(!r.require_epic);
        assert!(r.require_device_id);
        assert!(r.record_connection_ip);
    }
}
