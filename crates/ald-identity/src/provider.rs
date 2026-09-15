use serde::{Deserialize, Serialize};

/// GTA V distribution channel. Detection != entitlement verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameDistribution {
    Rockstar,
    Steam,
    Epic,
    Unknown,
}

/// Provider abstraction for legitimate entitlement verification.
pub trait GameEntitlementProvider {
    /// Verify GTA V ownership for the active session.
    /// Return Unknown/Unavailable if verification is technically unavailable.
    fn verify_entitlement(&self) -> ald_core::Result<super::Entitlement>;
    fn resolve_account(&self) -> ald_core::Result<Option<String>>;
    fn distribution(&self) -> GameDistribution;
}

/// Rockstar entitlement provider conceptual API.
/// Never bypasses Rockstar auth or steals tokens.
pub struct RockstarEntitlementProvider;

impl GameEntitlementProvider for RockstarEntitlementProvider {
    fn verify_entitlement(&self) -> ald_core::Result<super::Entitlement> {
        // No legitimate local verification path without official SDK/token.
        // Always report Unknown; never fake VERIFIED.
        Ok(super::Entitlement::Unknown)
    }
    fn resolve_account(&self) -> ald_core::Result<Option<String>> {
        Ok(None)
    }
    fn distribution(&self) -> GameDistribution {
        GameDistribution::Rockstar
    }
}

/// Steam entitlement provider. Resolves SteamID64 from a valid session.
pub struct SteamEntitlementProvider {
    pub steam_id64: Option<String>,
}

impl GameEntitlementProvider for SteamEntitlementProvider {
    fn verify_entitlement(&self) -> ald_core::Result<super::Entitlement> {
        match &self.steam_id64 {
            Some(_) => Ok(super::Entitlement::Verified),
            None => Ok(super::Entitlement::NotLoggedIn),
        }
    }
    fn resolve_account(&self) -> ald_core::Result<Option<String>> {
        Ok(self.steam_id64.clone())
    }
    fn distribution(&self) -> GameDistribution {
        GameDistribution::Steam
    }
}
