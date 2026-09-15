//! Resource discovery configuration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesConfig {
    /// Directory holding resource folders (each with an ald_manifest.toml).
    #[serde(default = "default_directory")]
    pub directory: String,
    /// Optional explicit resource allow-list. Empty = discover all.
    #[serde(default)]
    pub enabled: Vec<String>,
}

fn default_directory() -> String {
    "resources".into()
}

impl Default for ResourcesConfig {
    fn default() -> Self {
        ResourcesConfig { directory: default_directory(), enabled: Vec::new() }
    }
}
