//! Aldivine Secrets — central secret abstraction (spec: "Central secret abstraction").
//!
//! Invariants:
//! - a plaintext secret is never stored in config, logs, telemetry, audit or crash reports
//! - `set_secret` produces a [`SecretHandle`] (a reference, e.g. `env:VAR`), never the value
//! - resolution happens late, only where the value is actually needed, via a [`SecretProvider`]
//! - a [`RedactedValue`] wraps a resolved secret and can only be displayed as `***`

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{CfgError, CfgResult};

/// A reference to a secret, e.g. `env:ALDIVINE_DATABASE_URL`.
///
/// The handle itself is safe to log: it carries no secret material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretHandle {
    /// Provider name: `env`, `file`, `credman`, `vault`, `kms`.
    pub provider: String,
    /// Provider-specific key (env var name, file path, key id...).
    pub key: String,
}

impl SecretHandle {
    /// Parse `provider:key`. On a bare value (no provider prefix), the `env` provider is assumed
    /// and the whole string is the variable name — matching `set_secret x MY_VAR` convenience.
    pub fn parse(spec: &str) -> CfgResult<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(CfgError::EmptySecret { line: 0 });
        }
        match spec.split_once(':') {
            Some((provider, key)) if !provider.is_empty() => {
                if key.trim().is_empty() {
                    return Err(CfgError::EmptySecret { line: 0 });
                }
                let provider = provider.trim().to_ascii_lowercase();
                if !KNOWN_PROVIDERS.contains(&provider.as_str()) {
                    return Err(CfgError::SecretProvider {
                        handle: spec.to_string(),
                        reason: format!("unknown provider `{provider}`"),
                    });
                }
                Ok(SecretHandle { provider, key: key.trim().to_string() })
            }
            _ => Ok(SecretHandle { provider: "env".into(), key: spec.to_string() }),
        }
    }

    pub fn as_spec(&self) -> String {
        format!("{}:{}", self.provider, self.key)
    }
}

impl fmt::Display for SecretHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_spec())
    }
}

pub const KNOWN_PROVIDERS: &[&str] = &["env", "file", "credman", "vault", "kms"];

/// A secret provider resolves handles to values. Never store the returned value.
pub trait SecretProvider {
    fn name(&self) -> &'static str;
    fn resolve(&self, handle: &SecretHandle) -> CfgResult<String>;
}

/// Resolves from the process environment. The baseline provider — no external dependencies,
/// works for single-server operation (spec: Redis/external services must NOT be mandatory).
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvProvider;

impl SecretProvider for EnvProvider {
    fn name(&self) -> &'static str {
        "env"
    }

    fn resolve(&self, handle: &SecretHandle) -> CfgResult<String> {
        std::env::var(&handle.key).map_err(|_| CfgError::SecretProvider {
            handle: handle.as_spec(),
            reason: "environment variable not set".into(),
        })
    }
}

/// A resolved secret that cannot be displayed accidentally.
#[derive(Clone)]
pub struct RedactedValue {
    inner: String,
}

impl RedactedValue {
    pub fn new(value: String) -> Self {
        RedactedValue { inner: value }
    }

    /// Access the plaintext. Callers must not log the returned string.
    pub fn expose(&self) -> &str {
        &self.inner
    }

    /// Redact a string in place for logging: replaces the value with `***`.
    pub fn redact(s: &str) -> String {
        if s.is_empty() {
            return String::new();
        }
        "***".to_string()
    }
}

impl fmt::Debug for RedactedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedactedValue").field("value", &"***").finish()
    }
}

impl fmt::Display for RedactedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "***")
    }
}

impl PartialEq for RedactedValue {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
impl Eq for RedactedValue {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_explicit() {
        let h = SecretHandle::parse("env:ALDIVINE_DATABASE_URL").unwrap();
        assert_eq!(h.provider, "env");
        assert_eq!(h.key, "ALDIVINE_DATABASE_URL");
        assert_eq!(h.as_spec(), "env:ALDIVINE_DATABASE_URL");
    }

    #[test]
    fn parse_bare_defaults_to_env() {
        let h = SecretHandle::parse("MY_DB_VAR").unwrap();
        assert_eq!(h.provider, "env");
        assert_eq!(h.key, "MY_DB_VAR");
    }

    #[test]
    fn parse_unknown_provider_rejected() {
        let e = SecretHandle::parse("opensesame:k").unwrap_err();
        assert!(matches!(e, CfgError::SecretProvider { .. }));
    }

    #[test]
    fn parse_empty_rejected() {
        assert!(matches!(SecretHandle::parse("").unwrap_err(), CfgError::EmptySecret { .. }));
        assert!(matches!(SecretHandle::parse("   ").unwrap_err(), CfgError::EmptySecret { .. }));
    }

    #[test]
    fn parse_provider_only_rejected() {
        // `env:` has an empty key — falls through to env-with-colon name, which is not a
        // legitimate variable name; we reject it as malformed.
        let e = SecretHandle::parse("env:").unwrap_err();
        assert!(matches!(e, CfgError::EmptySecret { .. }));
    }

    #[test]
    fn handle_exposes_no_secret_value() {
        // A handle carries the *name* of a secret, never its value. Logging a handle is safe.
        std::env::set_var("ALD_SECRET_DISPLAY", "the-actual-secret-value");
        let h = SecretHandle::parse("env:ALD_SECRET_DISPLAY").unwrap();
        let displayed = format!("{h}");
        assert!(displayed.contains("ALD_SECRET_DISPLAY"));
        assert!(!displayed.contains("the-actual-secret-value"));
        std::env::remove_var("ALD_SECRET_DISPLAY");
    }

    #[test]
    fn env_provider_resolves() {
        std::env::set_var("ALD_SECRET_TEST", "hunter2");
        let h = SecretHandle::parse("env:ALD_SECRET_TEST").unwrap();
        assert_eq!(EnvProvider.resolve(&h).unwrap(), "hunter2");
        std::env::remove_var("ALD_SECRET_TEST");
    }

    #[test]
    fn env_provider_missing_is_error_not_empty() {
        let h = SecretHandle::parse("env:ALD_SECRET_DEFINITELY_MISSING").unwrap();
        let e = EnvProvider.resolve(&h).unwrap_err();
        assert!(matches!(e, CfgError::SecretProvider { .. }));
    }

    #[test]
    fn redacted_never_exposes_via_debug_or_display() {
        let v = RedactedValue::new("hunter2".into());
        assert_eq!(format!("{v}"), "***");
        assert_eq!(format!("{v:?}"), "RedactedValue { value: \"***\" }");
        assert_eq!(v.expose(), "hunter2");
    }

    #[test]
    fn redacted_equality_for_tests() {
        assert_eq!(RedactedValue::new("a".into()), RedactedValue::new("a".into()));
        assert_ne!(RedactedValue::new("a".into()), RedactedValue::new("b".into()));
    }

    #[test]
    fn all_spec_providers_recognized() {
        for p in KNOWN_PROVIDERS {
            assert!(SecretHandle::parse(&format!("{p}:somekey")).is_ok(), "{p} rejected");
        }
    }

    #[test]
    fn redact_helper() {
        assert_eq!(RedactedValue::redact("anything"), "***");
        assert_eq!(RedactedValue::redact(""), "");
    }
}
