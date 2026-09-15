use ald_core::AldError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_max_conn")]
    pub max_connections_per_ip: u32,
    #[serde(default = "default_fw")]
    pub enable_astranet_firewall: bool,
    #[serde(default = "default_rate")]
    pub global_event_rate_limit: u32,
}

fn default_max_conn() -> u32 {
    8
}
fn default_fw() -> bool {
    true
}
fn default_rate() -> u32 {
    1000
}

impl Default for SecurityConfig {
    fn default() -> Self {
        SecurityConfig {
            max_connections_per_ip: default_max_conn(),
            enable_astranet_firewall: default_fw(),
            global_event_rate_limit: default_rate(),
        }
    }
}

impl SecurityConfig {
    pub fn validate(&self) -> Result<(), AldError> {
        if self.max_connections_per_ip == 0 {
            return Err(AldError::Config("security.max_connections_per_ip must be >= 1".into()));
        }
        if self.global_event_rate_limit == 0 {
            return Err(AldError::Config("security.global_event_rate_limit must be >= 1".into()));
        }
        Ok(())
    }
}
