use thiserror::Error;

/// Unified error type for Aldivine subsystems.
/// Crates extend with their own variants via `AldError::Other` or by mapping.
#[derive(Debug, Error)]
pub enum AldError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("resource error: {0}")]
    Resource(String),

    #[error("identity error: {0}")]
    Identity(String),

    #[error("entitlement provider unavailable")]
    EntitlementUnknown,

    #[error("rpc timeout")]
    RpcTimeout,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("other: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AldError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let e = AldError::Config("bad toml".into());
        assert_eq!(e.to_string(), "config error: bad toml");
        let t = AldError::RpcTimeout;
        assert_eq!(t.to_string(), "rpc timeout");
    }
}
