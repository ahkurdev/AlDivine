use thiserror::Error;

pub type CfgResult<T> = Result<T, CfgError>;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CfgError {
    #[error("line {line}: unterminated string starting at column {col}")]
    UnterminatedString { line: u32, col: u32 },

    #[error("line {line}: directive `{verb}` expects {expected} argument(s), got {got}")]
    WrongArgCount { line: u32, verb: String, expected: usize, got: usize },

    #[error("line {line}: unknown directive `{verb}` (allowed: {allowed})")]
    UnknownDirective { line: u32, verb: String, allowed: String },

    #[error("line {line}: {reason}")]
    InvalidValue { line: u32, reason: String },

    #[error("line {line}: directive `{verb}` is not allowed inside an included file")]
    DirectiveForbiddenInInclude { line: u32, verb: String },

    #[error("line {line}: environment variable `{name}` is not set")]
    UnknownEnvVar { line: u32, name: String },

    #[error("include at line {line}: `{path}` is outside the allowed config root")]
    IncludeOutsideRoot { line: u32, path: String },

    #[error("include at line {line}: `{path}` — directory traversal (`..`) is not allowed")]
    IncludeTraversal { line: u32, path: String },

    #[error("include at line {line}: `{path}` not found")]
    IncludeMissing { line: u32, path: String },

    #[error("include depth limit ({limit}) exceeded at line {line}: `{path}`")]
    IncludeDepthLimit { line: u32, path: String, limit: usize },

    #[error("include cycle detected at line {line}: `{path}` (chain: {chain})")]
    IncludeCycle { line: u32, path: String, chain: String },

    #[error("secret directive at line {line} resolved to an empty value")]
    EmptySecret { line: u32 },

    #[error("secret provider error for `{handle}`: {reason}")]
    SecretProvider { handle: String, reason: String },
}

/// Verb whitelist. Unknown verbs are rejected rather than silently ignored, because a
/// typo in a security directive (e.g. `set_sekret`) must surface, not become a silent no-op.
pub const KNOWN_DIRECTIVES: &[&str] = &[
    "set",
    "setr",
    "sets",
    "set_secret",
    "exec",
    "ensure",
    "start",
    "stop",
    "restart",
    "sv_hostname",
    "sv_projectName",
    "sv_projectDesc",
    "sv_maxclients",
    "sv_endpoint",
    "endpoint_add_tcp",
    "endpoint_add_udp",
    "sv_tags",
    "sv_locale",
    "sv_icon",
    "sv_gameBuild",
    "sv_public",
    "sv_listing",
    "sv_licenseKey",
    "ald_projectId",
    "ald_serverId",
    "ald_licenseKey",
    "license_mode",
    "add_ace",
    "remove_ace",
    "add_principal",
    "remove_principal",
];

pub fn is_known(verb: &str) -> bool {
    KNOWN_DIRECTIVES.contains(&verb)
}

/// Directives that only make sense in the root document (never inside an included file).
pub const ROOT_ONLY_DIRECTIVES: &[&str] = &["exec"];

pub fn is_root_only(verb: &str) -> bool {
    ROOT_ONLY_DIRECTIVES.contains(&verb)
}
