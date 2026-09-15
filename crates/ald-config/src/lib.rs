//! Server and runtime configuration with strict validation.
//! Invalid security-relevant values are rejected on load, never silently ignored.

use ald_core::AldError;
use serde::{Deserialize, Serialize};

pub mod identity;
pub mod network;
pub mod resources;
pub mod security;
pub mod server;

pub use identity::IdentityConfig;
pub use network::NetworkConfig;
pub use resources::ResourcesConfig;
pub use security::SecurityConfig;
pub use server::ServerConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub network: NetworkConfig,
    pub identity: IdentityConfig,
    pub security: SecurityConfig,
    #[serde(default)]
    pub resources: ResourcesConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_worker_threads")]
    pub worker_threads: usize,
    #[serde(default = "default_max_resources")]
    pub max_resources: usize,
}

fn default_worker_threads() -> usize {
    4
}
fn default_max_resources() -> usize {
    256
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig { worker_threads: default_worker_threads(), max_resources: default_max_resources() }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: ServerConfig::default(),
            network: NetworkConfig::default(),
            identity: IdentityConfig::default(),
            security: SecurityConfig::default(),
            resources: ResourcesConfig::default(),
            runtime: RuntimeConfig { worker_threads: default_worker_threads(), max_resources: default_max_resources() },
        }
    }
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Self, AldError> {
        let cfg: Config = toml::from_str(s).map_err(|e| AldError::Config(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), AldError> {
        self.server.validate()?;
        self.network.validate()?;
        self.identity.validate()?;
        self.security.validate()?;
        if self.runtime.worker_threads == 0 {
            return Err(AldError::Config("worker_threads must be >= 1".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        let c = Config::default();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn parse_and_validate() {
        let toml = r#"
[server]
name = "test"
max_players = 32

[network]
bind = "0.0.0.0:30120"
protocol_version = 1

[identity]
require_aldivine_account = true
require_gta_entitlement = true

[security]
max_connections_per_ip = 4
enable_astranet_firewall = true
"#;
        let c = Config::from_toml(toml).expect("parse");
        assert_eq!(c.server.max_players, 32);
        assert!(c.identity.require_aldivine_account);
    }

    #[test]
    fn bad_security_rejected() {
        let toml = r#"
[server]
name = "x"
max_players = 1
[network]
bind = "0.0.0.0:1"
protocol_version = 1
[identity]
require_aldivine_account = false
require_gta_entitlement = false
[security]
max_connections_per_ip = 0
enable_astranet_firewall = true
"#;
        assert!(Config::from_toml(toml).is_err());
    }
}
