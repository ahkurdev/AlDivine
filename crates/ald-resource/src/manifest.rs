use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub minimum_runtime: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub optional_dependencies: Vec<Dependency>,
    #[serde(default)]
    pub client_scripts: Vec<String>,
    #[serde(default)]
    pub server_scripts: Vec<String>,
    #[serde(default)]
    pub shared_scripts: Vec<String>,
    #[serde(default)]
    pub ui: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub exports: Vec<String>,
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub compatibility: Vec<String>,
    #[serde(default)]
    pub configuration: Vec<String>,
    #[serde(default)]
    pub database_migrations: Vec<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Parse(String),
    MissingField(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Parse(s) => write!(f, "manifest parse error: {s}"),
            ManifestError::MissingField(s) => write!(f, "manifest missing field: {s}"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    pub fn parse(toml_str: &str) -> Result<Self, ManifestError> {
        let m: Manifest = toml::from_str(toml_str).map_err(|e| ManifestError::Parse(e.to_string()))?;
        if m.name.trim().is_empty() {
            return Err(ManifestError::MissingField("name".into()));
        }
        if m.version.trim().is_empty() {
            return Err(ManifestError::MissingField("version".into()));
        }
        Ok(m)
    }

    pub fn capability_set(&self) -> Vec<String> {
        self.capabilities.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let t = r#"
name = "myresource"
version = "0.1.0"
capabilities = ["filesystem.read", "network.http"]
"#;
        let m = Manifest::parse(t).unwrap();
        assert_eq!(m.name, "myresource");
        assert_eq!(m.capabilities.len(), 2);
    }

    #[test]
    fn missing_name_errors() {
        let t = r#"
version = "0.1.0"
"#;
        assert!(Manifest::parse(t).is_err());
    }
}
