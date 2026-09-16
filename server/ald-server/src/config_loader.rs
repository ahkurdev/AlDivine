use std::path::Path;

use ald_config::Config;
use ald_core::AldError;
use ald_servercfg::normalize::NormalizedServerConfig;
use ald_servercfg::parse::ParseOptions;

pub enum ConfigSource {
    Explicit(String),
    ServerCfg,
    ServerToml,
    Default,
}

impl ConfigSource {
    pub fn describe(&self) -> String {
        match self {
            ConfigSource::Explicit(p) => format!("explicit --config {p}"),
            ConfigSource::ServerCfg => "server.cfg".into(),
            ConfigSource::ServerToml => "server.toml".into(),
            ConfigSource::Default => "built-in default".into(),
        }
    }
}

pub fn resolve_config_path(args: &[String]) -> (Option<String>, ConfigSource) {
    if let Some(pos) = args.iter().position(|a| a == "--config" || a == "-c") {
        if let Some(p) = args.get(pos + 1) {
            return (Some(p.clone()), ConfigSource::Explicit(p.clone()));
        }
    }
    if Path::new("server.cfg").exists() {
        return (Some("server.cfg".into()), ConfigSource::ServerCfg);
    }
    if Path::new("server.toml").exists() {
        return (Some("server.toml".into()), ConfigSource::ServerToml);
    }
    (None, ConfigSource::Default)
}

pub fn load_config(path: Option<&str>, source: &ConfigSource) -> Result<Config, AldError> {
    match source {
        ConfigSource::Explicit(p) => load_file(p),
        ConfigSource::ServerCfg => load_file(path.unwrap_or("server.cfg")),
        ConfigSource::ServerToml => load_file(path.unwrap_or("server.toml")),
        ConfigSource::Default => Ok(Config::default()),
    }
}

fn load_file(path: &str) -> Result<Config, AldError> {
    let text = std::fs::read_to_string(path).map_err(|e| AldError::Config(format!("read {path}: {e}")))?;
    if path.ends_with(".cfg") {
        from_server_cfg(&text, path)
    } else {
        Config::from_toml(&text)
    }
}

pub fn from_server_cfg(text: &str, source: &str) -> Result<Config, AldError> {
    let opts = ParseOptions::default();
    let norm = NormalizedServerConfig::from_root(source, text, &opts)
        .map_err(|e| AldError::Config(format!("server.cfg parse: {e}")))?;
    apply_normalized(norm)
}

pub fn apply_normalized(norm: NormalizedServerConfig) -> Result<Config, AldError> {
    let mut cfg = Config::default();
    if let Some(name) = norm.hostname {
        cfg.server.name = name;
    }
    if let Some(max) = norm.max_clients {
        cfg.server.max_players = max;
    }
    if let Some(ep) = norm.endpoint {
        cfg.network.bind = ep;
    } else if let Some(first) = norm.endpoints.first() {
        cfg.network.bind = first.addr.clone();
    }
    let mut enabled: Vec<String> = Vec::new();
    for r in &norm.resources {
        if matches!(r.action.as_str(), "ensure" | "start") && !enabled.contains(&r.name) {
            enabled.push(r.name.clone());
        }
    }
    if !enabled.is_empty() {
        cfg.resources.enabled = enabled;
    }
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_maps_hostname_and_players() {
        let text = "sv_hostname \"Test Server\"\nsv_maxclients 48\nendpoint_add_udp \"0.0.0.0:30120\"\nensure spawn\n";
        let cfg = from_server_cfg(text, "test.cfg").unwrap();
        assert_eq!(cfg.server.name, "Test Server");
        assert_eq!(cfg.server.max_players, 48);
        assert_eq!(cfg.network.bind, "0.0.0.0:30120");
        assert!(cfg.resources.enabled.contains(&"spawn".to_string()));
    }

    #[test]
    fn cfg_stop_not_enabled() {
        let text = "sv_hostname \"X\"\nstop badres\n";
        let cfg = from_server_cfg(text, "test.cfg").unwrap();
        assert!(!cfg.resources.enabled.contains(&"badres".to_string()));
    }

    #[test]
    fn cfg_bad_max_rejected() {
        let text = "sv_hostname \"X\"\nsv_maxclients 0\n";
        assert!(from_server_cfg(text, "test.cfg").is_err());
    }

    #[test]
    fn toml_still_loads() {
        let toml = "[server]\nname = \"t\"\nmax_players = 16\n[network]\nbind = \"127.0.0.1:30120\"\nprotocol_version = 1\n[identity]\n[security]\n";
        let cfg = Config::from_toml(toml).unwrap();
        assert_eq!(cfg.server.max_players, 16);
    }
}
