//! Aldivine server.cfg — AST-preserving parser, include system, convars, secrets.
//!
//! Design notes (spec: "server.cfg as first-class configuration"):
//! - tokens are quoted-string aware; never whitespace-split blindly
//! - comments, blank lines and ordering preserved so round-trip edits stay readable
//! - `exec` includes are resolved against a virtual FS trait, with traversal + cycle guards
//! - secrets never live in the AST as plaintext: `set_secret` resolves to a `SecretHandle`

pub mod ast;
pub mod error;
pub mod include_resolver;
pub mod parse;
pub mod secrets;
pub mod normalize;

pub use ast::{Directive, Document, Value};
pub use error::{CfgError, CfgResult};
pub use include_resolver::{IncludeResolver, InMemoryFs, Vfs};
pub use normalize::{NormalizedServerConfig, ReloadImpact};
pub use parse::parse_document;
pub use secrets::{SecretHandle, SecretProvider, EnvProvider, RedactedValue};
