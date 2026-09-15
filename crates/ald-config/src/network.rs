use ald_core::AldError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_proto")]
    pub protocol_version: u16,
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
}

fn default_bind() -> String {
    "0.0.0.0:30120".into()
}
fn default_proto() -> u16 {
    1
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig { bind: default_bind(), protocol_version: default_proto(), trusted_proxies: Vec::new() }
    }
}

impl NetworkConfig {
    pub fn validate(&self) -> Result<(), AldError> {
        if self.bind.is_empty() {
            return Err(AldError::Config("network.bind required".into()));
        }
        if self.protocol_version == 0 {
            return Err(AldError::Config("network.protocol_version must be >= 1".into()));
        }
        Ok(())
    }
}
