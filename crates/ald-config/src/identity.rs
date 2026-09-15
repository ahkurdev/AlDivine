use ald_core::AldError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    #[serde(default)]
    pub require_aldivine_account: bool,
    #[serde(default)]
    pub require_gta_entitlement: bool,
    #[serde(default)]
    pub require_rockstar: bool,
    #[serde(default)]
    pub require_steam: bool,
    #[serde(default)]
    pub require_epic: bool,
    #[serde(default = "default_device")]
    pub require_device_id: bool,
    #[serde(default = "default_ip")]
    pub record_connection_ip: bool,
}

fn default_device() -> bool {
    true
}
fn default_ip() -> bool {
    true
}

impl Default for IdentityConfig {
    fn default() -> Self {
        IdentityConfig {
            require_aldivine_account: false,
            require_gta_entitlement: false,
            require_rockstar: false,
            require_steam: false,
            require_epic: false,
            require_device_id: default_device(),
            record_connection_ip: default_ip(),
        }
    }
}

impl IdentityConfig {
    pub fn validate(&self) -> Result<(), AldError> {
        // Server must not require Steam by default; this is a config sanity check.
        if self.require_steam && !self.require_gta_entitlement {
            return Err(AldError::Config("require_steam requires require_gta_entitlement".into()));
        }
        Ok(())
    }
}
