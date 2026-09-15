//! server.cfg → NormalizedServerConfig.
//!
//! Both `server.cfg` and `server.toml` (spec) converge here. The normalized model is what
//! runtime systems consume; the parser keeps the source-faithful AST for Aegis diffing.
//!
//! ponytail: `sv_licenseKey` is accepted as a *compat* convar and deliberately NOT validated
//! as an Aldivine license (spec: never treat a Cfx key as an Aldivine key). Same for the
//! legacy `mysql_connection_string` → compat hint only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ast::{Directive, Document, LineKind};
use crate::error::{CfgError, CfgResult};
use crate::parse::ParseOptions;
use crate::secrets::{EnvProvider, RedactedValue, SecretHandle, SecretProvider};

/// Convar visibility class (spec: set / setr / sets semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConvarScope {
    /// `set` — server-local only.
    Local,
    /// `setr` — replicated to clients per replication policy.
    Replicated,
    /// `sets` — public server metadata (must never contain secrets).
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Convar {
    pub scope: ConvarScope,
    pub value: String,
    /// 1-based line in the source document this came from.
    pub source_line: u32,
}

/// A convar whose value is held by reference to a secret, never inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretConvar {
    pub handle: SecretHandle,
    pub source_line: u32,
}

/// How a config change propagates at runtime (spec: "Config validation / hot reload").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReloadImpact {
    /// Safe to apply without touching anything runtime-owned.
    HotReloadSafe,
    /// Needs a resource reload to take effect.
    ResourceReloadRequired,
    /// Needs a full server restart.
    ServerRestartRequired,
    /// Cannot change once the process is running (e.g. bind endpoint).
    ImmutableRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AceRule {
    pub principal: String,
    pub object: String,
    pub allow: bool,
    pub source_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalRule {
    pub principal: String,
    pub parent: String,
    pub add: bool,
    pub source_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDirective {
    /// ensure | start | stop | restart
    pub action: String,
    pub name: String,
    pub source_line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointTransport {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointSpec {
    pub addr: String,
    pub transport: EndpointTransport,
    pub source_line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedServerConfig {
    pub hostname: Option<String>,
    pub project_name: Option<String>,
    pub project_desc: Option<String>,
    pub max_clients: Option<u32>,
    pub endpoint: Option<String>,
    pub endpoints: Vec<EndpointSpec>,
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub icon: Option<String>,
    pub game_build: Option<String>,
    pub public: bool,
    pub listing: bool,
    pub project_id: Option<String>,
    pub server_id: Option<String>,
    pub license_mode: Option<String>,
    pub aldivine_license_key: Option<SecretConvar>,
    /// Legacy Cfx key — compat-only marker; never validated as an Aldivine license.
    pub fivem_license_key: Option<String>,
    /// Legacy MySQL string — routed to ald-compat-db, never treated as PostgreSQL.
    pub legacy_mysql: Option<String>,

    pub convars: BTreeMap<String, Convar>,
    pub secrets: BTreeMap<String, SecretConvar>,
    pub ace: Vec<AceRule>,
    pub principals: Vec<PrincipalRule>,
    pub resources: Vec<ResourceDirective>,
    pub includes: Vec<String>,

    pub warnings: Vec<String>,
}

impl NormalizedServerConfig {
    /// Parse a root server.cfg body and normalize it, without resolving includes.
    /// `exec` lines are recorded in [`Self::includes`] and then ignored here —
    /// inlining is [`crate::IncludeResolver`]'s job. Callers that want the merged
    /// config must resolve first and pass the expanded document to
    /// [`NormalizedServerConfig::from_document`].
    pub fn from_root(source: &str, text: &str, opts: &ParseOptions) -> CfgResult<Self> {
        let root = crate::parse::parse_document(source, text, opts)?;
        let includes = collect_includes(&root);
        let mut cfg = NormalizedServerConfig::default();
        for (line, d) in root.directives() {
            match d.verb.as_str() {
                "exec" => continue,
                other => cfg.apply_directive(other, d, line)?,
            }
        }
        cfg.validate()?;
        cfg.includes = includes;
        Ok(cfg)
    }

    /// Dispatch one directive to the right applier.
    fn apply_directive(&mut self, verb: &str, d: &Directive, line: u32) -> CfgResult<()> {
        match verb {
            "set" | "setr" | "sets" => {
                // A convar whose name is a known typed directive (e.g. sv_maxclients)
                // must be routed to the typed path, otherwise `sv_maxclients 48` is
                // silently stored as an opaque string and never parsed.
                if let Some(k) = d.kv().map(|(k, _)| k) {
                    if is_typed_directive(k) {
                        return self.apply_typed(k, d, line);
                    }
                }
                self.apply_convar(d, line)
            }
            "set_secret" => self.apply_secret(d, line),
            "ensure" | "start" | "stop" | "restart" => self.apply_resource(d, line),
            "add_ace" | "remove_ace" => self.apply_ace(d, line),
            "add_principal" | "remove_principal" => self.apply_principal(d, line),
            "endpoint_add_tcp" | "endpoint_add_udp" => self.apply_endpoint(d, line),
            "exec" => Err(CfgError::DirectiveForbiddenInInclude { line, verb: "exec".into() }),
            known => self.apply_typed(known, d, line),
        }
    }

    /// Flatten an include-expanded [`Document`] into the normalized model.
    pub fn from_document(doc: &Document) -> CfgResult<Self> {
        let mut cfg = NormalizedServerConfig::default();
        for (line, d) in doc.directives() {
            cfg.apply_directive(&d.verb, d, line)?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    fn apply_typed(&mut self, verb: &str, d: &Directive, line: u32) -> CfgResult<()> {
        macro_rules! one {
            () => {
                // Both source shapes reach here: the bare form `sv_maxclients 48`
                // and the convar form `set sv_maxclients 48`. Read the VALUE in
                // either case — never the key.
                match d.verb.as_str() {
                    "set" | "setr" | "sets" => d.kv().map(|(_, v)| v),
                    _ => d.first_arg(),
                }
                .ok_or(CfgError::WrongArgCount {
                    line,
                    verb: d.verb.clone(),
                    expected: 1,
                    got: d.args.len(),
                })?
            };
        }
        match verb {
            "sv_hostname" => self.hostname = Some(one!().to_string()),
            "sv_projectName" => self.project_name = Some(one!().to_string()),
            "sv_projectDesc" => self.project_desc = Some(one!().to_string()),
            "sv_maxclients" => {
                let raw = one!();
                let n: u32 = raw.parse().map_err(|_| CfgError::InvalidValue {
                    line,
                    reason: format!("sv_maxclients must be an integer, got `{raw}`"),
                })?;
                self.max_clients = Some(n);
            }
            "sv_endpoint" => self.endpoint = Some(one!().to_string()),
            "sv_tags" => {
                self.tags = one!()
                    .split([',', ' '])
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "sv_locale" => self.locale = Some(one!().to_string()),
            "sv_icon" => self.icon = Some(one!().to_string()),
            "sv_gameBuild" => self.game_build = Some(one!().to_string()),
            "sv_public" => self.public = parse_bool(one!(), line, "sv_public")?,
            "sv_listing" => self.listing = parse_bool(one!(), line, "sv_listing")?,
            "ald_projectId" => self.project_id = Some(one!().to_string()),
            "ald_serverId" => self.server_id = Some(one!().to_string()),
            "license_mode" => self.license_mode = Some(one!().to_string()),
            "ald_licenseKey" => {
                let h = SecretHandle::parse(one!())?;
                self.aldivine_license_key = Some(SecretConvar { handle: h, source_line: line });
            }
            "sv_licenseKey" => {
                // Compat only. Never validated as an Aldivine license.
                self.fivem_license_key = Some(one!().to_string());
                self.warnings.push(format!(
                    "line {line}: sv_licenseKey is a FiveM compatibility marker; it is not an Aldivine license"
                ));
            }
            _ => {
                if let Some(v) = d.first_arg() {
                    self.convars.insert(
                        verb.to_string(),
                        Convar { scope: ConvarScope::Local, value: v.to_string(), source_line: line },
                    );
                }
            }
        }
        Ok(())
    }

    fn apply_convar(&mut self, d: &Directive, line: u32) -> CfgResult<()> {
        let (k, v) = d.kv().ok_or(CfgError::WrongArgCount {
            line,
            verb: d.verb.clone(),
            expected: 2,
            got: d.args.len(),
        })?;
        let scope = match d.verb.as_str() {
            "set" => ConvarScope::Local,
            "setr" => ConvarScope::Replicated,
            "sets" => ConvarScope::Public,
            _ => unreachable!(),
        };
        if let Some(existing) = self.convars.get(k) {
            if existing.scope != scope {
                return Err(CfgError::InvalidValue {
                    line,
                    reason: format!(
                        "convar `{k}` declared with conflicting scope (was {:?}, now {:?})",
                        existing.scope, scope
                    ),
                });
            }
        }
        self.convars.insert(k.to_string(), Convar { scope, value: v.to_string(), source_line: line });
        if k == "mysql_connection_string" {
            self.legacy_mysql = Some(v.to_string());
        }
        Ok(())
    }

    fn apply_secret(&mut self, d: &Directive, line: u32) -> CfgResult<()> {
        let (k, spec) = d.kv().ok_or(CfgError::WrongArgCount {
            line,
            verb: d.verb.clone(),
            expected: 2,
            got: d.args.len(),
        })?;
        let handle = SecretHandle::parse(spec)?;
        self.secrets.insert(k.to_string(), SecretConvar { handle, source_line: line });
        Ok(())
    }

    fn apply_resource(&mut self, d: &Directive, line: u32) -> CfgResult<()> {
        let name = d.first_arg().ok_or(CfgError::WrongArgCount {
            line,
            verb: d.verb.clone(),
            expected: 1,
            got: d.args.len(),
        })?;
        self.resources.push(ResourceDirective {
            action: d.verb.clone(),
            name: name.to_string(),
            source_line: line,
        });
        Ok(())
    }

    fn apply_ace(&mut self, d: &Directive, line: u32) -> CfgResult<()> {
        let (principal, object, granted) = match d.args.as_slice() {
            [p, o, a, ..] => (p.as_str(), o.as_str(), parse_grant(a.as_str(), line)?),
            _ => {
                return Err(CfgError::WrongArgCount {
                    line,
                    verb: d.verb.clone(),
                    expected: 3,
                    got: d.args.len(),
                })
            }
        };
        // remove_ace X allow == deny X.
        let allow = if d.verb == "add_ace" { granted } else { !granted };
        self.ace.push(AceRule { principal: principal.into(), object: object.into(), allow, source_line: line });
        Ok(())
    }

    fn apply_principal(&mut self, d: &Directive, line: u32) -> CfgResult<()> {
        let (principal, parent) = match d.args.as_slice() {
            [p, parent, ..] => (p.as_str(), parent.as_str()),
            _ => {
                return Err(CfgError::WrongArgCount {
                    line,
                    verb: d.verb.clone(),
                    expected: 2,
                    got: d.args.len(),
                })
            }
        };
        let add = d.verb == "add_principal";
        self.principals.push(PrincipalRule {
            principal: principal.into(),
            parent: parent.into(),
            add,
            source_line: line,
        });
        Ok(())
    }

    fn apply_endpoint(&mut self, d: &Directive, line: u32) -> CfgResult<()> {
        let addr = d.first_arg().ok_or(CfgError::WrongArgCount {
            line,
            verb: d.verb.clone(),
            expected: 1,
            got: d.args.len(),
        })?;
        let transport = if d.verb == "endpoint_add_tcp" { EndpointTransport::Tcp } else { EndpointTransport::Udp };
        self.endpoints.push(EndpointSpec { addr: addr.to_string(), transport, source_line: line });
        Ok(())
    }

    /// Cross-field validation. A config that parses but is unsafe is still rejected.
    pub fn validate(&self) -> CfgResult<()> {
        if let Some(n) = self.max_clients {
            if n == 0 || n > 1024 {
                return Err(CfgError::InvalidValue {
                    line: 0,
                    reason: format!("sv_maxclients out of range (1..=1024): {n}"),
                });
            }
        }

        // `sets` convars are public server metadata: secrets must never appear there.
        for (k, c) in &self.convars {
            if c.scope == ConvarScope::Public && looks_secret(k) {
                return Err(CfgError::InvalidValue {
                    line: c.source_line,
                    reason: format!(
                        "convar `{k}` looks secret but is declared `sets` (public metadata); use set_secret instead"
                    ),
                });
            }
        }

        // A name may not be both a secret and a replicated/public convar.
        for (k, s) in &self.secrets {
            if let Some(c) = self.convars.get(k) {
                if c.scope != ConvarScope::Local {
                    return Err(CfgError::InvalidValue {
                        line: c.source_line,
                        reason: format!("convar `{k}` has both a secret and a replicated/public value"),
                    });
                }
            }
            // An empty resolved secret must fail loudly, not become an empty string.
            if let Ok(v) = EnvProvider.resolve(&s.handle) {
                if v.is_empty() {
                    return Err(CfgError::EmptySecret { line: s.source_line });
                }
            }
        }

        if let Some(mode) = &self.license_mode {
            const MODES: &[&str] = &[
                "free", "aldivine", "paid", "subscription", "private", "developer", "beta", "invite_only",
            ];
            if !MODES.contains(&mode.as_str()) {
                return Err(CfgError::InvalidValue {
                    line: 0,
                    reason: format!("unknown license_mode `{mode}`"),
                });
            }
        }

        Ok(())
    }

    /// Late-resolve a named secret into a redacted wrapper.
    pub fn secret(&self, name: &str) -> CfgResult<RedactedValue> {
        let s = self.secrets.get(name).ok_or_else(|| CfgError::SecretProvider {
            handle: name.to_string(),
            reason: "no such secret convar".into(),
        })?;
        Ok(RedactedValue::new(EnvProvider.resolve(&s.handle)?))
    }

    /// The reload consequence of changing a setting (Aegis shows this before applying).
    pub fn reload_impact(&self, key: &str) -> ReloadImpact {
        match key {
            "sv_hostname" | "sv_projectName" | "sv_projectDesc" | "sv_tags" | "sv_locale" | "sv_icon"
            | "sv_listing" => ReloadImpact::HotReloadSafe,
            "sv_maxclients" | "sv_endpoint" | "endpoint_add_tcp" | "endpoint_add_udp" | "ald_serverId" => {
                ReloadImpact::ImmutableRuntime
            }
            "sv_public" | "license_mode" | "ald_licenseKey" => ReloadImpact::ServerRestartRequired,
            _ => ReloadImpact::ResourceReloadRequired,
        }
    }
}

/// The `exec` targets declared in a document, in order. `IncludeResolver` consumes
/// those lines when inlining, so this must be run on the *root* document (pre-expansion),
/// not on the flattened result.
fn collect_includes(doc: &Document) -> Vec<String> {
    doc.lines
        .iter()
        .filter_map(|l| match &l.kind {
            LineKind::Directive(d) if d.verb == "exec" => d.first_arg().map(str::to_string),
            _ => None,
        })
        .collect()
}

/// ACE directives use the FiveM-style `allow` / `deny` vocabulary rather than booleans.
fn parse_grant(raw: &str, line: u32) -> CfgResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "allow" => Ok(true),
        "deny" => Ok(false),
        other => Err(CfgError::InvalidValue {
            line,
            reason: format!("ace rule must be `allow` or `deny`, got `{other}`"),
        }),
    }
}

fn parse_bool(raw: &str, line: u32, ctx: &str) -> CfgResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(CfgError::InvalidValue {
            line,
            reason: format!("`{ctx}` expects a boolean, got `{other}`"),
        }),
    }
}

/// Directives that carry typed fields on [`NormalizedServerConfig`] rather than living as
/// opaque convars. `set sv_maxclients 48` must reach the typed path, not be stored as a string.
pub const TYPED_DIRECTIVES: &[&str] = &[
    "sv_hostname",
    "sv_projectName",
    "sv_projectDesc",
    "sv_maxclients",
    "sv_endpoint",
    "sv_tags",
    "sv_locale",
    "sv_icon",
    "sv_gameBuild",
    "sv_public",
    "sv_listing",
    "endpoint_add_tcp",
    "endpoint_add_udp",
    "ald_projectId",
    "ald_serverId",
    "ald_licenseKey",
    "license_mode",
];

pub fn is_typed_directive(name: &str) -> bool {
    TYPED_DIRECTIVES.contains(&name)
}

/// Heuristic for convar names that must never be public metadata.
fn looks_secret(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    const HINTS: &[&str] = &[
        "secret", "password", "passwd", "key", "token", "credential", "url", "dsn", "connection",
    ];
    HINTS.iter().any(|h| k.contains(h))
}

/// Names of convars whose plaintext must be redacted from logs/telemetry/audit.
pub fn is_secret_convar(name: &str) -> bool {
    looks_secret(name)
}

/// Render a convar value for logging: redacted if secret-looking, verbatim otherwise.
pub fn redact_convar(name: &str, value: &str) -> String {
    if is_secret_convar(name) {
        RedactedValue::redact(value)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::include_resolver::{IncludeResolver, InMemoryFs, Vfs};
    use crate::parse::{parse_document, ParseOptions};
    use crate::error::is_root_only;

    fn cfg_from(text: &str) -> CfgResult<NormalizedServerConfig> {
        let doc = parse_document("<test>", text, &ParseOptions::default())?;
        NormalizedServerConfig::from_document(&doc)
    }

    #[test]
    fn typed_directives_parsed() {
        let c = cfg_from(
            "sv_hostname \"Test\"\nsv_maxclients 64\nsv_tags \"rp, pvp\"\nsv_public true\nlicense_mode \"free\"\n",
        )
        .unwrap();
        assert_eq!(c.hostname.as_deref(), Some("Test"));
        assert_eq!(c.max_clients, Some(64));
        assert_eq!(c.tags, vec!["rp", "pvp"]);
        assert!(c.public);
        assert_eq!(c.license_mode.as_deref(), Some("free"));
    }

    #[test]
    fn maxclients_must_be_number() {
        let e = cfg_from("sv_maxclients lots").unwrap_err();
        assert!(matches!(e, CfgError::InvalidValue { .. }));
    }

    #[test]
    fn maxclients_range_enforced() {
        assert!(cfg_from("sv_maxclients 0").is_err());
        assert!(cfg_from("sv_maxclients 5000").is_err());
        assert!(cfg_from("sv_maxclients 48").is_ok());
    }

    #[test]
    fn set_setr_sets_scopes() {
        let c = cfg_from("set a 1\nsetr b 2\nsets c 3").unwrap();
        assert_eq!(c.convars["a"].scope, ConvarScope::Local);
        assert_eq!(c.convars["b"].scope, ConvarScope::Replicated);
        assert_eq!(c.convars["c"].scope, ConvarScope::Public);
    }

    #[test]
    fn conflicting_scope_rejected() {
        let e = cfg_from("set a 1\nsetr a 2").unwrap_err();
        assert!(matches!(e, CfgError::InvalidValue { .. }));
    }

    #[test]
    fn secret_convar_never_inline() {
        let c = cfg_from("set_secret database_url env:ALD_TEST_DB").unwrap();
        let s = c.secrets.get("database_url").expect("secret convar present");
        assert_eq!(s.handle.provider, "env");
        assert_eq!(s.handle.key, "ALD_TEST_DB");
        assert!(!c.convars.contains_key("database_url"));
    }

    #[test]
    fn secret_resolution_late_and_redacted() {
        std::env::set_var("ALD_TEST_DB", "postgres://secret");
        let c = cfg_from("set_secret database_url env:ALD_TEST_DB").unwrap();
        let v = c.secret("database_url").unwrap();
        assert_eq!(v.expose(), "postgres://secret");
        assert_eq!(format!("{v}"), "***");
        std::env::remove_var("ALD_TEST_DB");
    }

    #[test]
    fn secret_under_sets_rejected() {
        let e = cfg_from("sets database_url \"pg://x\"").unwrap_err();
        assert!(matches!(e, CfgError::InvalidValue { .. }));
    }

    #[test]
    fn empty_resolved_secret_rejected() {
        std::env::set_var("ALD_TEST_EMPTY", "");
        let e = cfg_from("set_secret k env:ALD_TEST_EMPTY").unwrap_err();
        assert!(matches!(e, CfgError::EmptySecret { .. }));
        std::env::remove_var("ALD_TEST_EMPTY");
    }

    #[test]
    fn ace_rules_normalized() {
        let c = cfg_from("add_ace group.admin command allow\nremove_ace group.admin command allow").unwrap();
        assert_eq!(c.ace.len(), 2);
        assert!(c.ace[0].allow);
        assert!(!c.ace[1].allow);
    }

    #[test]
    fn ace_needs_three_args() {
        let e = cfg_from("add_ace group.admin command").unwrap_err();
        assert!(matches!(e, CfgError::WrongArgCount { expected: 3, .. }));
    }

    #[test]
    fn ace_bool_must_be_valid() {
        let e = cfg_from("add_ace group.admin command maybe").unwrap_err();
        assert!(matches!(e, CfgError::InvalidValue { .. }));
    }

    #[test]
    fn principal_inheritance() {
        let c = cfg_from("add_principal group.admin group.superuser\nremove_principal group.mod group.admin").unwrap();
        assert_eq!(c.principals.len(), 2);
        assert!(c.principals[0].add);
        assert!(!c.principals[1].add);
    }

    #[test]
    fn resource_directives_preserve_order() {
        let c = cfg_from("ensure [system]\nstart my_resource\nstop other\nrestart chat").unwrap();
        let actions: Vec<&str> = c.resources.iter().map(|r| r.action.as_str()).collect();
        assert_eq!(actions, vec!["ensure", "start", "stop", "restart"]);
    }

    #[test]
    fn endpoints() {
        let c = cfg_from("endpoint_add_tcp 0.0.0.0:30120\nendpoint_add_udp 0.0.0.0:30120").unwrap();
        assert_eq!(c.endpoints.len(), 2);
        assert_eq!(c.endpoints[0].transport, EndpointTransport::Tcp);
        assert_eq!(c.endpoints[1].transport, EndpointTransport::Udp);
    }

    #[test]
    fn legacy_fivem_key_is_warning_not_license() {
        let c = cfg_from("sv_licenseKey cfxk_xxx").unwrap();
        assert!(c.fivem_license_key.is_some());
        assert!(c.aldivine_license_key.is_none());
        assert!(c.warnings.iter().any(|w| w.contains("not an Aldivine license")));
    }

    #[test]
    fn legacy_mysql_routed_to_compat() {
        let c = cfg_from("set mysql_connection_string \"mysql://old\"").unwrap();
        assert_eq!(c.legacy_mysql.as_deref(), Some("mysql://old"));
    }

    #[test]
    fn license_mode_validated() {
        assert!(cfg_from("license_mode free").is_ok());
        assert!(cfg_from("license_mode aldivine").is_ok());
        assert!(cfg_from("license_mode bogus").is_err());
    }

    #[test]
    fn exec_in_flat_doc_rejected() {
        let e = cfg_from("exec config/db.cfg").unwrap_err();
        assert!(matches!(e, CfgError::DirectiveForbiddenInInclude { .. }));
    }

    #[test]
    fn full_include_pipeline() {
        let mut fs = InMemoryFs::new();
        fs.insert(
            "server.cfg",
            "# main\nset sv_maxclients 48\nexec config/database.cfg\nensure [system]\n",
        );
        fs.insert(
            "config/database.cfg",
            "set_secret database_url env:ALD_TEST_DB2\nset database_driver \"postgres\"\n",
        );
        std::env::set_var("ALD_TEST_DB2", "postgres://real");
        let r = IncludeResolver::new(&fs);
        let doc = r.resolve("server.cfg").unwrap();
        let c = NormalizedServerConfig::from_document(&doc).unwrap();
        // Line 3 in the flattened file is `sv_maxclients 48` (line 1 is the comment).
        assert_eq!(c.max_clients, Some(48));
        assert_eq!(c.resources.len(), 1);
        assert_eq!(c.convars["database_driver"].value, "postgres");
        assert_eq!(c.secret("database_url").unwrap().expose(), "postgres://real");
        // The expanded document has its `exec` lines consumed by the resolver, so the
        // include list must come from the root document via `from_root`.
        assert!(c.includes.is_empty());
        std::env::remove_var("ALD_TEST_DB2");

        let root = "# main\nset sv_maxclients 48\nexec config/database.cfg\nensure [system]\n";
        let rc =
            NormalizedServerConfig::from_root("server.cfg", root, &ParseOptions::default())
                .unwrap();
        assert_eq!(rc.includes, vec!["config/database.cfg"]);
    }

    #[test]
    fn reload_impact_classes() {
        let c = NormalizedServerConfig::default();
        assert_eq!(c.reload_impact("sv_hostname"), ReloadImpact::HotReloadSafe);
        assert_eq!(c.reload_impact("sv_maxclients"), ReloadImpact::ImmutableRuntime);
        assert_eq!(c.reload_impact("sv_public"), ReloadImpact::ServerRestartRequired);
        assert_eq!(c.reload_impact("some_resource_cfg"), ReloadImpact::ResourceReloadRequired);
    }

    #[test]
    fn redaction_helpers() {
        assert_eq!(redact_convar("database_url", "pg://x"), "***");
        assert_eq!(redact_convar("sv_hostname", "My Server"), "My Server");
        assert!(is_secret_convar("API_TOKEN"));
    }

    #[test]
    fn empty_config_ok() {
        let c = cfg_from("").unwrap();
        assert!(c.convars.is_empty());
    }

    #[test]
    fn custom_directive_becomes_local_convar() {
        let c = cfg_from("ald_custom_thing 7").unwrap();
        assert_eq!(c.convars["ald_custom_thing"].value, "7");
    }

    #[test]
    fn is_root_only_export_works() {
        assert!(is_root_only("exec"));
    }
}
