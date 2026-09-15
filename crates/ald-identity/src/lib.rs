//! Aldivine Identity Service — platform-level identity aggregation.
//! External platform ids link to AldivinePlayerId but are never primary keys.

use serde::{Deserialize, Serialize};

pub mod aggregation;
pub mod ban;
pub mod pipeline;
pub mod provider;

pub use aggregation::{
    DeviceIdentity, EpicIdentity, IdentityAggregation, NetworkIdentity, PlayerIdentity, RockstarIdentity,
};
pub use ban::{BanMatchResult, BanMatchSignal};
pub use pipeline::{ConnectionRejection, IdentityRequirements, PolicyOutcome, TrustedProxies};
pub use provider::{GameDistribution, GameEntitlementProvider};

/// Canonical entitlement states. Never report Verified without validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Entitlement {
    Verified,
    NotOwned,
    NotLoggedIn,
    Unavailable,
    Unknown,
    Error,
}

impl Entitlement {
    pub fn as_code(&self) -> &'static str {
        match self {
            Entitlement::Verified => "VERIFIED",
            Entitlement::NotOwned => "NOT_OWNED",
            Entitlement::NotLoggedIn => "NOT_LOGGED_IN",
            Entitlement::Unavailable => "UNAVAILABLE",
            Entitlement::Unknown => "UNKNOWN",
            Entitlement::Error => "ERROR",
        }
    }
}

/// Steam identity. SteamID64 is canonical; steam_hex is the compatibility rep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteamIdentity {
    pub steam_id64: String,
    pub steam_hex: String,
    pub verified: bool,
}

impl SteamIdentity {
    /// Convert a SteamID64 to the `steam:110000xxxx` compatibility form.
    /// Steam hex = 0x110000100000000 + (steam_id64 - 76561197960265728).
    pub fn hex_from_id64(id64: &str) -> Option<String> {
        let n: u64 = id64.parse().ok()?;
        if n < 76561197960265728 {
            return None;
        }
        let account_id = n - 76561197960265728;
        let hex = 0x110000100000000u64 + account_id;
        Some(format!("steam:{hex:x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_core::AldivinePlayerId;

    #[test]
    fn steam_hex_conversion() {
        let h = SteamIdentity::hex_from_id64("76561198000000000").unwrap();
        assert_eq!(h, "steam:1100001025e4c00");
    }

    #[test]
    fn entitlement_codes() {
        assert_eq!(Entitlement::Verified.as_code(), "VERIFIED");
        assert_eq!(Entitlement::Unknown.as_code(), "UNKNOWN");
    }

    #[test]
    fn aggregate_creates_player() {
        let agg = PlayerIdentity::new(AldivinePlayerId::new());
        assert!(!agg.aldivine_id.to_string().is_empty());
    }
}
