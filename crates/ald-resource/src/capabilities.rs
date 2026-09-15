use ald_core::AldError;

/// Resource capability tokens. Resources must declare these in the manifest.
pub const KNOWN_CAPABILITIES: &[&str] = &[
    "filesystem.read",
    "filesystem.write",
    "network.http",
    "database.query",
    "database.transaction",
    "player.read",
    "player.manage",
    "entity.create",
    "entity.manage",
    "resource.control",
    "server.commands",
    "identity.steam.read",
    "identity.rockstar.read",
];

pub struct Capability {
    pub token: String,
}

/// Validate a capability list against known tokens. Unknown tokens are rejected.
pub fn parse_capabilities(tokens: &[String]) -> Result<Vec<Capability>, AldError> {
    let mut out = Vec::new();
    for t in tokens {
        if !KNOWN_CAPABILITIES.contains(&t.as_str()) {
            return Err(AldError::CapabilityDenied(format!("unknown capability '{t}'")));
        }
        out.push(Capability { token: t.clone() });
    }
    Ok(out)
}

/// Check whether a resource with `granted` capabilities may use `required`.
pub fn check(granted: &[String], required: &str) -> bool {
    granted.iter().any(|c| c == required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_capability_ok() {
        let c = parse_capabilities(&["filesystem.read".into()]).unwrap();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn unknown_capability_rejected() {
        assert!(parse_capabilities(&["godmode".into()]).is_err());
    }

    #[test]
    fn check_grant() {
        let g = vec!["network.http".into()];
        assert!(check(&g, "network.http"));
        assert!(!check(&g, "database.query"));
    }
}
