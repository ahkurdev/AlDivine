use ald_core::AldError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_max")]
    pub max_players: u32,
    #[serde(default)]
    pub description: String,
}

fn default_name() -> String {
    "Aldivine Server".into()
}
fn default_max() -> u32 {
    64
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig { name: default_name(), max_players: default_max(), description: String::new() }
    }
}

impl ServerConfig {
    pub fn validate(&self) -> Result<(), AldError> {
        if self.name.trim().is_empty() {
            return Err(AldError::Config("server.name required".into()));
        }
        if self.max_players == 0 || self.max_players > 4096 {
            return Err(AldError::Config("server.max_players out of range 1..=4096".into()));
        }
        Ok(())
    }
}
